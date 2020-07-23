/*
 *  Copyright (C) 2020  Joe Hirschfeld <j@ibj.io>
 *
 *  This program is free software: you can redistribute it and/or modify
 *  it under the terms of the GNU General Public License as published by
 *  the Free Software Foundation, either version 3 of the License, or
 *  (at your option) any later version.
 *
 *  This program is distributed in the hope that it will be useful,
 *  but WITHOUT ANY WARRANTY; without even the implied warranty of
 *  MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 *  GNU General Public License for more details.
 *
 *  You should have received a copy of the GNU General Public License
 *  along with this program.  If not, see <https://www.gnu.org/licenses/>.
 */

use std::alloc;
use std::cell::Cell;
use std::mem::{align_of, size_of, ManuallyDrop, MaybeUninit};

#[derive(Copy, Clone)]
struct FrameHeader {
    // The index of the next frame header
    next: usize,
    // The reference count of this frame
    is_live: bool,
}

// Note: Unions never call their destructors as it is impossible to tell which element is actually
// initialized within the union. When a frame is destructed, all elements within the frame also
// need dropped.
union RingElement<T> {
    // A frame header which is placed at the beginning of each Frame as a marker to the end of the
    // frame as well as a running reference count
    header: FrameHeader,
    // A frame element
    element: ManuallyDrop<T>,
}

pub struct FramedRing<T> {
    // This is generally a datastructure which does a lot of 'unsafe' stuff to be efficient
    ring: Cell<*mut MaybeUninit<RingElement<T>>>,
    // The base of the ring, which contains the index of the root FrameHeader
    base: Cell<usize>,
    // The head of the ring, which will contain the next element to be inserted.
    head: Cell<usize>,
    // ring_size = 2 pow ring_size_2
    ring_size_2: Cell<u8>,
}

impl<T> FramedRing<T> {
    pub fn frame<'ring>(&'ring mut self) -> RingFrameMut<'ring, T> {
        // Why does this take a mut? Because this action is only valid if there are no other frames
        // which exist for this ring.
        let start = self.head.get();

        self.append_to_ring(RingElement {
            header: FrameHeader {
                next: start + 1,
                is_live: true,
            },
        });

        RingFrameMut {
            f: RingFrame {
                ring: self,
                start,
                live_at: start + 1,
            },
        }
    }

    fn append_to_ring(&self, re: RingElement<T>) {
        let len = 1 << self.ring_size_2.get();
        // len is always a power of 2 (see above definition) - this is a cheap way of performing a
        // % operation.
        let mut mask = len - 1;

        let old_head = self.head.get();

        // Progress the head!
        self.head.set(old_head + 1);

        let base_i = self.base.get() & mask;
        let head_i = self.head.get() & mask;

        if base_i == head_i {
            unsafe {
                // We have run out of space, double the size and copy stuff over in a way that
                // isn't stupid

                // TODO: Potential overrun. This should probably be fixed by performing memory
                // accounting somewhere
                self.ring_size_2.update(|v| v + 1);

                let new_buffer_layout = alloc::Layout::from_size_align_unchecked(
                    size_of::<RingElement<T>>() << self.ring_size_2.get(),
                    align_of::<RingElement<T>>(),
                );
                let new_buffer: *mut MaybeUninit<RingElement<T>> =
                    std::mem::transmute(alloc::alloc(new_buffer_layout));

                let new_len = 1 << self.ring_size_2.get();

                // We now do 2 separate copies to transfer the data into the new expanded memory
                // space without messing up any indexes. Since this is always a doubling of size
                // along a power of 2, we always know our 3 critical points (and that there isn't a
                // potential 4th point that we need to calculate). Our two copies will be from base
                // to the end of the old array, then from the start of the new array to head. Since
                // this condition is only hit when the array is completely full, all of this will
                // be valid.

                // The pivot point is the end of array / start of array transition. This is the
                // point which our memcpys will pivot around.
                let pivot_point = self.head.get() & (!mask);

                // After this point, mask now refers to the mask of the new buffer
                mask = new_len - 1;

                // Base -> Pivot copy
                std::ptr::copy_nonoverlapping(
                    self.ring.get().add(base_i),
                    new_buffer.add(self.base.get() & mask),
                    pivot_point - self.base.get(),
                );

                // Pivot -> Head copy
                std::ptr::copy_nonoverlapping(
                    self.ring.get(),
                    new_buffer.add(pivot_point & mask),
                    self.head.get() - pivot_point,
                );

                let old_buffer_layout = alloc::Layout::from_size_align_unchecked(
                    size_of::<RingElement<T>>() << (self.ring_size_2.get() - 1),
                    align_of::<RingElement<T>>(),
                );

                // Deallocate the old buffer
                std::alloc::dealloc(std::mem::transmute(self.ring.get()), old_buffer_layout);

                // Move the new buffer to replace the old buffer
                self.ring.set(new_buffer);
            }
        }

        unsafe {
            self.ring
                .get()
                .add(old_head & mask)
                .write(MaybeUninit::new(re));
        }
    }

    pub fn try_promote<'ring>(
        &'ring mut self,
        frame: RingFrame<'ring, T>,
    ) -> Option<RingFrameMut<'ring, T>> {
        unsafe {
            let header = self.get(frame.start).header;
            if header.next != self.head.get() {
                None
            } else {
                Some(RingFrameMut {
                    f: RingFrame {
                        ring: self,
                        start: frame.start,
                        live_at: frame.live_at,
                    },
                })
            }
        }
    }

    pub fn promote<'ring>(&'ring mut self, frame: RingFrame<'ring, T>) -> RingFrameMut<'ring, T> {
        self.try_promote(frame).unwrap()
    }

    fn mask(&self) -> usize {
        (1 << self.ring_size_2.get()) - 1
    }

    unsafe fn get<'a>(&'a self, i: usize) -> &'a RingElement<T> {
        self.get_masked(i & self.mask())
    }

    unsafe fn get_mut<'a>(&'a self, i: usize) -> &'a mut RingElement<T> {
        self.get_masked_mut(i & self.mask())
    }

    unsafe fn get_masked<'a>(&'a self, i: usize) -> &'a RingElement<T> {
        std::mem::transmute(&*self.ring.get().add(i))
    }

    unsafe fn get_masked_mut<'a>(&'a self, i: usize) -> &'a mut RingElement<T> {
        std::mem::transmute(&mut *self.ring.get().add(i))
    }
}

impl<T> Drop for FramedRing<T> {
    fn drop(&mut self) {
        // If we can drop, that means all child frames have been dropped and we just need to free
        // the buffer.

        unsafe {
            let layout = alloc::Layout::from_size_align_unchecked(
                size_of::<RingElement<T>>() << self.ring_size_2.get(),
                align_of::<RingElement<T>>(),
            );

            alloc::dealloc(std::mem::transmute(self.ring.get()), layout);
        }
    }
}

pub struct RingFrameMut<'ring, T> {
    f: RingFrame<'ring, T>,
}

pub struct RingFrame<'ring, T> {
    ring: &'ring FramedRing<T>,
    start: usize,
    live_at: usize,
}

impl<'ring, T> Drop for RingFrame<'ring, T> {
    fn drop(&mut self) {
        // Drop all of our contents, then attempt to progress base as far as we can.  We can
        // progress this all the way up until base == head, in which case this was the last frame
        // in the ring (not that it matters to us, but interesting to know).

        unsafe {
            // This mask is only valid over while the ring doesn't change size.
            let mask = self.ring.mask();
            let mut header = &mut self.ring.get_masked_mut(self.start & mask).header;
            if std::mem::needs_drop::<T>() {
                for i in self.live_at + 1..header.next {
                    ManuallyDrop::drop(&mut self.ring.get_masked_mut(i & mask).element);
                }
            }

            if self.ring.head.get() == header.next {
                // Special case for the head of the line - just roll the head back to the header
                // index
                self.ring.head.set(self.start);
                return;
            }

            // We are the base!
            if self.ring.base.get() == self.start {
                // Okay, so now we need to figure out a new base by skipping around the buffer
                // until we hit either the end of the ring or a frame which is still in use.
                let header_idx = self.start;
                let mut working_header = header;
                loop {
                    let next_header_index = header_idx + working_header.next;
                    if next_header_index >= self.ring.head.get() {
                        if next_header_index > self.ring.head.get() {
                            // This is a memory corruption issue
                            panic!("trail of headers does not lead to the head");
                        }
                        // Exit - we are done here as the ring is now empty.
                        self.ring.base.set(self.ring.head.get());
                        return;
                    }

                    working_header = &mut self.ring.get_masked_mut(header_idx & mask).header;
                    if working_header.is_live {
                        // Exit - this frame is still being used, and is now the new base
                        self.ring.base.set(header_idx);
                        return;
                    }
                }
            } else {
                // Decrement the header, as we no longer are using this frame but can't 'reclaim'
                // the space until the base frame is dropped.
                header.is_live = false;
            }
        }
    }
}

pub struct RingFrameIter<'a, T> {
    ring: &'a FramedRing<T>,
    i: usize,
    end: usize,
}

impl<'a, T> Iterator for RingFrameIter<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        if self.i >= self.end {
            None
        } else {
            unsafe {
                let item = &self.ring.get(self.i).element;
                self.i += 1;
                Some(item)
            }
        }
    }
}

pub struct RingFrameIntoIter<'a, T> {
    f: RingFrame<'a, T>,
    end: usize,
}

impl<'a, T> Iterator for RingFrameIntoIter<'a, T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        if self.f.live_at >= self.end {
            None
        } else {
            unsafe {
                // This is basically what happens in self.f.ring.get(), but without doing memory
                // transmutation because we actually don't want to do it here.
                let element = (&*self.f.ring.ring.get().add(self.f.live_at))
                    .read()
                    .element;
                self.f.live_at += 1;
                Some(ManuallyDrop::into_inner(element))
            }
        }
    }
}

impl<'ring, T> RingFrame<'ring, T> {
    fn header<'a>(&'a self) -> &'a FrameHeader {
        unsafe { &self.ring.get(self.start).header }
    }

    pub fn len(&self) -> usize {
        self.header().next - self.start - 1
    }

    pub fn get(&self, index: usize) -> Option<&T> {
        let i = self.start + index + 1;
        if i >= self.header().next {
            None
        } else {
            Some(unsafe { &self.ring.get(i).element })
        }
    }

    pub unsafe fn get_unchecked(&self, index: usize) -> &T {
        &self.ring.get(self.start + index + 1).element
    }

    pub fn iter<'a>(&'a self) -> RingFrameIter<'a, T> {
        RingFrameIter {
            ring: self.ring,
            i: self.start + 1,
            end: self.header().next,
        }
    }
}

impl<'ring, T> IntoIterator for RingFrame<'ring, T> {
    type Item = T;
    type IntoIter = RingFrameIntoIter<'ring, T>;

    fn into_iter(self) -> Self::IntoIter {
        unsafe {
            let header = self.ring.get(self.start).header;
            RingFrameIntoIter {
                f: self,
                end: header.next,
            }
        }
    }
}

impl<'ring, T> RingFrameMut<'ring, T> {
    pub fn next(self) -> (RingFrame<'ring, T>, RingFrameMut<'ring, T>) {
        // We need to produce a new frame header for the new frame
        let head = self.f.ring.head.get();
        self.f.ring.append_to_ring(RingElement {
            header: FrameHeader {
                next: head + 1,
                is_live: true,
            },
        });

        let ring = self.f.ring;

        (
            self.f,
            RingFrameMut {
                f: RingFrame {
                    ring,
                    start: head,
                    live_at: head + 1,
                },
            },
        )
    }

    pub fn append(&self, element: T) {
        unsafe {
            self.f.ring.append_to_ring(RingElement {
                element: ManuallyDrop::new(element),
            });

            let mut header = self.f.ring.get_mut(self.f.start).header;
            header.next += 1;
        }
    }

    pub fn inner<'a>(&'a self) -> &'a RingFrame<'ring, T> {
        &self.f
    }
}
