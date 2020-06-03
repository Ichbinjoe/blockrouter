/*
 *  Copyright (C) 2020  Joe Hirschfeld <j@ibj.io>
 *
 *  This program is free software: you can redistribute it and/or modify
 *  it under the terms of the GNU General Public License as published by
 *  the Free Software Foundation, either version 3 of the License, or
 *  (at your option) any later version.
 *
 *  This program is distributed in the hope that it will be useful,
 *  but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 *  GNU General Public License for more details.
 *
 *  You should have received a copy of the GNU General Public License
 *  along with this program.  If not, see <https://www.gnu.org/licenses/>.
 */

extern crate crossbeam_queue;
extern crate crossbeam_utils;
extern crate memmap;

use crossbeam_queue::SegQueue;
use crossbeam_utils::Backoff;
use std::cell::RefCell;
use std::ops::Deref;
use std::ops::DerefMut;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

pub struct GlobalMemPoolSettings {
    pub buf_size: usize,
    pub tls_entries: usize,
    pub page_entries: usize,
    pub concurrent_allocation_limit: u64,
}

struct Page {
    pool: Arc<GlobalMemPool>,
    m: memmap::MmapMut,
}

#[derive(Clone, Copy)]
struct Slice {
    ptr: *mut u8,
    len: usize,
}

struct SliceLifecycle {
    page: Arc<Page>,
    d: Slice,
}

struct SliceRef {
    sl: SliceLifecycle,
}

impl Drop for SliceRef {
    fn drop(&mut self) {
        self.sl.page.pool.reclaim(
            // We basically end up performing a copy here, but we have special magic here because
            // refcounting and whatnot.
            // Its a bit wasteful, but what are you going to do 
            SliceLifecycle{
                page: self.sl.page.clone(),
                d: self.sl.d,
            });
    }
}

#[derive(Clone)]
pub struct Part {
    b: Rc<SliceRef>,
    data: Slice,
}

impl Part {
    fn new(parent: Rc<SliceRef>) -> Part {
        let d = parent.sl.d;
        Part { b: parent, data: d }
    }
}

impl Deref for Part {
    type Target = [u8];
    fn deref(&self) -> &Self::Target {
        unsafe { std::slice::from_raw_parts(self.data.ptr, self.data.len) }
    }
}

impl DerefMut for Part {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { std::slice::from_raw_parts_mut(self.data.ptr, self.data.len) }
    }
}

pub struct TLMemPool {
    cache: Vec<SliceLifecycle>,
}

pub struct GlobalMemPool {
    memory: SegQueue<SliceLifecycle>,
    gidx: usize,
    settings: GlobalMemPoolSettings,
    allocs: AtomicU64,
}

impl GlobalMemPool {
    /// Creates a new GlobalMemPool with the given settings
    /// Why is this unsafe? This function is _NOT_ reentrant, and will do bad things
    pub unsafe fn new(settings: GlobalMemPoolSettings) -> GlobalMemPool {
        let gidx = UTLMPI.allocators;
        UTLMPI.allocators += 1;
        GlobalMemPool {
            memory: SegQueue::new(),
            gidx,
            allocs: AtomicU64::new(0),
            settings,
        }
    }

    fn reclaim(&self, memory: SliceLifecycle) {
        // First, lets try to shove it on the end of our TLCache
        LUT.with(|tlmplut_rc| {
            let mut tlmplut = tlmplut_rc.borrow_mut();
            if let Some(tlmp_maybe) = tlmplut.lut.get_mut(self.gidx) {
                if let Some(tlmp) = tlmp_maybe {
                    if tlmp.cache.capacity() - tlmp.cache.len() > 0 {
                        tlmp.cache.push(memory);
                        return;
                    }
                }
            }

            // Pushing onto the local cache failed, just push to the global listing
            self.memory.push(memory);
        });
    }

    /// Allocates a new Part
    pub fn allocate(self_rc: &Arc<GlobalMemPool>) -> Part {
        let slice_lifecycle = LUT
            .with(|tlmplut_rc| {
                tlmplut_rc
                    .borrow_mut()
                    .lut
                    .get_mut(self_rc.gidx)
                    .and_then(|tlmp_maybe: &mut Option<TLMemPool>| tlmp_maybe.as_mut())
                    .and_then(|tlmp: &mut TLMemPool| tlmp.cache.pop())
            })
            .unwrap_or_else(|| GlobalMemPool::allocate_global(self_rc));

        Part::new(Rc::new(SliceRef {
            sl: slice_lifecycle,
        }))
    }

    /// Installs a local cache-lookup entry in the global thread-local LUT - this isn't required,
    /// but the memory allocator will generally have higher performance on this thread if there are
    /// repeated allocations & deallocations
    pub fn install_tl_cache(&self) {
        LUT.with(|tlmplut_rc| {
            let mut tlmplut = tlmplut_rc.borrow_mut();
            // first, fill in None options if necessary
            let initial_size = tlmplut.lut.len();
            // this is slightly inefficient, but idk who cares
            for _ in initial_size..self.gidx + 1 {
                tlmplut.lut.push(None);
            }

            tlmplut.lut[self.gidx].get_or_insert(TLMemPool {
                cache: Vec::with_capacity(self.settings.tls_entries),
            });
        });
    }

    fn allocate_global(self_rc: &Arc<GlobalMemPool>) -> SliceLifecycle {
        let backoff = Backoff::new();
        let slf = self_rc.as_ref();
        loop {
            match slf.memory.pop() {
                Ok(slice) => return slice,
                Err(_) => {
                    // Try to allocate
                    let previous_allocs = slf.allocs.fetch_add(1, Ordering::AcqRel);
                    if previous_allocs < slf.settings.concurrent_allocation_limit - 1 {
                        // perform a new allocation
                        // TODO: This should fail more.... gracefully? Blowing up the program isn't
                        // exactly... nice?
                        let mm = memmap::MmapMut::map_anon(
                            slf.settings.page_entries << slf.settings.buf_size,
                        )
                        .unwrap();

                        let page = Arc::new(Page {
                            pool: self_rc.clone(),
                            m: mm,
                        });

                        // Now you may asking, woah there cowboy. Thats some pretty unsafe bullshit
                        // you are pulling here. And I would agree. Unfortuantely the rust compiler
                        // has lost to the will of me - this should work, as the slice will be
                        // static in memory no matter where the structures move (as is intended).
                        let base_ptr = page.m.deref().as_ptr() as *mut u8;

                        let entry_size = 1 << slf.settings.buf_size;
                        let first_slice = SliceLifecycle {
                            page: page.clone(),
                            d: Slice {
                                ptr: base_ptr.clone(),
                                len: entry_size,
                            },
                        };

                        for itr in 1..slf.settings.page_entries {
                            let slice = SliceLifecycle {
                                page: page.clone(),
                                d: Slice {
                                    ptr: unsafe { base_ptr.add(itr << slf.settings.buf_size) },
                                    len: entry_size,
                                },
                            };

                            slf.memory.push(slice);
                        }

                        slf.allocs.fetch_sub(1, Ordering::Release);

                        return first_slice;
                    } else {
                        // We are already allocating maximum pages, back off
                        slf.allocs.fetch_sub(1, Ordering::Release);

                        backoff.spin();
                        backoff.snooze();
                    }
                    continue;
                }
            }
        }
    }
}

struct TLMPLUT {
    lut: Vec<Option<TLMemPool>>,
}

thread_local! {
    static LUT: RefCell<TLMPLUT> = RefCell::new(TLMPLUT{lut: Vec::new()});
}

struct UniversalTLMPInfo {
    allocators: usize,
}

static mut UTLMPI: UniversalTLMPInfo = UniversalTLMPInfo { allocators: 0 };


