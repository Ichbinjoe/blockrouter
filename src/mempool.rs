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
use std::sync::atomic::{AtomicU64, Ordering};

pub struct GlobalMemPoolSettings {
    pub buf_size: usize,
    pub page_entries: usize,
    pub concurrent_allocation_limit: u64,
}

struct Page {
    m: memmap::MmapMut,
}

#[derive(Clone, Copy)]
struct Slice {
    ptr: *mut u8,
    len: usize,
}

#[derive(Clone)]
pub struct Part<'a> {
    global_mempool: &'a GlobalMemPool,
    parent_slice: *mut u8,
    data: Slice,
}

impl<'a> Drop for Part<'a> {
    fn drop(&mut self) {
        unsafe {
            let refcount = self.parent_slice.offset(self.global_mempool.realsize) as *mut u32;
            if *refcount == 1 {
                self.global_mempool.reclaim(self.parent_slice);
            }
            *refcount -= 1;
        }
    }
}

impl<'a> Deref for Part<'a> {
    type Target = [u8];
    fn deref(&self) -> &Self::Target {
        unsafe { std::slice::from_raw_parts(self.data.ptr, self.data.len) }
    }
}

impl<'a> DerefMut for Part<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { std::slice::from_raw_parts_mut(self.data.ptr, self.data.len) }
    }
}

pub struct TLMemPool {
    cache: Vec<*mut u8>,
}

pub struct GlobalMemPool {
    memory: SegQueue<*mut u8>,
    lk: &'static std::thread::LocalKey<RefCell<TLMemPool>>,
    settings: GlobalMemPoolSettings,
    realsize: isize,
    allocs: AtomicU64,
}

impl GlobalMemPool {
    /// Creates a new GlobalMemPool with the given settings
    /// Why is this unsafe? This function is _NOT_ reentrant, and will do bad things
    pub unsafe fn new(
        global_tlmp_ref: &'static std::thread::LocalKey<RefCell<TLMemPool>>,
        settings: GlobalMemPoolSettings,
    ) -> GlobalMemPool {
        GlobalMemPool {
            memory: SegQueue::new(),
            lk: global_tlmp_ref,
            allocs: AtomicU64::new(0),
            realsize: 1 << settings.buf_size - std::mem::size_of::<u32>(),
            settings,
        }
    }

    fn reclaim(&self, memory: *mut u8) {
        self.lk.with(|tlmp_rc| {
            unsafe {
                let tlmp = tlmp_rc.as_ptr();
                let cache = &mut (*tlmp).cache;
                if cache.capacity() - cache.len() > 0 {
                    cache.push(memory);
                    return;
                }
            }

            // Pushing onto the local cache failed, just push to the global listing
            self.memory.push(memory);
        });
    }

    /// Allocates a new Part
    pub fn allocate(&self) -> Part {
        let slice = self
            .lk
            .with(|tlmp| unsafe { (*tlmp.as_ptr()).cache.pop() })
            .unwrap_or_else(|| self.allocate_global());

        // There is a special sentienl at the tail end of every slice which acts as
        // the refcount value
        unsafe {
            let refcount_ptr = slice.offset(self.realsize as isize) as *mut u32;
            *refcount_ptr = 1;
        }
        
        Part{
            global_mempool: self,
            parent_slice: slice,
            data: Slice {
                ptr: slice,
                len: self.realsize as usize,
            },
        }
    }

    fn allocate_global(&self) -> *mut u8 {
        let backoff = Backoff::new();
        loop {
            match self.memory.pop() {
                Ok(slice) => return slice,
                Err(_) => {
                    // Try to allocate
                    let previous_allocs = self.allocs.fetch_add(1, Ordering::AcqRel);
                    if previous_allocs <= self.settings.concurrent_allocation_limit - 1 {
                        // perform a new allocation
                        // TODO: This should fail more.... gracefully? Blowing up the program isn't
                        // exactly... nice?
                        let mm = memmap::MmapMut::map_anon(
                            self.settings.page_entries << self.settings.buf_size,
                        )
                        .unwrap();

                        let page = Box::into_raw(Box::new(Page {
                            m: mm,
                        }));

                        // Now you may asking, woah there cowboy. Thats some pretty unsafe bullshit
                        // you are pulling here. And I would agree. Unfortuantely the rust compiler
                        // has lost to the will of me - this should work, as the slice will be
                        // static in memory no matter where the structures move (as is intended).
                        let base_ptr =
                            unsafe { page.as_ref().unwrap() }.m.deref().as_ptr() as *mut u8;

                        for itr in 1..self.settings.page_entries {
                            let ptr = unsafe { base_ptr.add(itr << self.settings.buf_size) };
                            self.memory.push(ptr);
                        }

                        self.allocs.fetch_sub(1, Ordering::Release);

                        return base_ptr;
                    } else {
                        // We are already allocating maximum pages, back off
                        self.allocs.fetch_sub(1, Ordering::Release);

                        backoff.spin();
                        backoff.snooze();
                    }
                    continue;
                }
            }
        }
    }
}

macro_rules! global_mempool_tlmp {
    ($label: ident, $cap: expr) => {
        thread_local! {
            static $label: RefCell<TLMemPool> = RefCell::new(TLMemPool{cache: Vec::with_capacity($cap)});
        }
    };
}

#[cfg(test)]
mod tests {
    extern crate test;

    use super::*;
    use test::Bencher;

    global_mempool_tlmp!(smoke_test_pool, 64);
    #[test]
    fn smoke_test() {
        let allocator = unsafe {
            GlobalMemPool::new(&smoke_test_pool, GlobalMemPoolSettings {
                buf_size: 12,
                concurrent_allocation_limit: 1,
                page_entries: 64,
            })
        };

        for i in 0..10000 {
            let mut buffer = GlobalMemPool::allocate(&allocator);
            buffer[0] = i as u8;
        }
    }

    global_mempool_tlmp!(bench_simple_tl_hot_pool, 64);
    #[bench]
    fn bench_simple_tl_hot(b: &mut Bencher) {
        let allocator = unsafe {
            GlobalMemPool::new(&bench_simple_tl_hot_pool, GlobalMemPoolSettings {
                buf_size: 12,
                concurrent_allocation_limit: 1,
                page_entries: 64,
            })
        };

        for _i in 0..10000 {
            let buffer = GlobalMemPool::allocate(&allocator);
            test::black_box(buffer);
        }
        b.iter(|| {
            for _i in 0..10000 {
                let buffer = GlobalMemPool::allocate(&allocator);
                test::black_box(buffer);
            }
        })
    }

    global_mempool_tlmp!(bench_simple_tl_cold_pool, 0);
    #[bench]
    fn bench_simple_tl_cold(b: &mut Bencher) {
        let allocator = unsafe {
            GlobalMemPool::new(&bench_simple_tl_cold_pool, GlobalMemPoolSettings {
                buf_size: 12,
                concurrent_allocation_limit: 1,
                page_entries: 64,
            })
        };
        for _i in 0..10000 {
            let buffer = GlobalMemPool::allocate(&allocator);
            test::black_box(buffer);
        }

        b.iter(|| {
            for _i in 0..10000 {
                let buffer = GlobalMemPool::allocate(&allocator);
                test::black_box(buffer);
            }
        })
    }

    use std::alloc::{alloc, dealloc, Layout};
    #[bench]
    fn system_malloc(b: &mut Bencher) {
        unsafe {
            let layout = Layout::from_size_align_unchecked(4096, 12);
            for _i in 0..10000 {
                let ptr = alloc(layout);
                test::black_box(ptr);
                dealloc(ptr, layout);
            }
            b.iter(|| {
                for _i in 0..10000 {
                    let ptr = alloc(layout);
                    test::black_box(ptr);
                    dealloc(ptr, layout);
                }
            })
        }
    }
}
