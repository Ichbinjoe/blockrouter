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

use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::ops::IndexMut;

struct FragmentPool<T> {
    end: usize,
    pool: [MaybeUninit<T>; 64],
}

impl<T> FragmentPool<T> {
    fn new() -> FragmentPool<T> {
        FragmentPool {
            end: 0,
            pool: MaybeUninit::uninit_array(),
        }
    }

    fn lossy_push(&mut self, element: T) {
        if let Some(mem) = self.pool.get_mut(self.end) {
            *mem = MaybeUninit::new(element);
            self.end += 1;
        }
    }

    fn maybe_pop(&mut self) -> Option<T> {
        if self.end == 0 {
            None
        } else {
            self.end -= 1;
            // This is safe because we know that 1) 0 <= self.end <= self.pool.len() and 2) that
            // items with index <= self.end are all initialized.
            //
            // When we move this item out of this spot via pointer magic, self.end will already
            // have been decremented beyond the element, so the destructor assumes that there is
            // not a valid item in it
            unsafe {
                let element = self.pool.get_unchecked_mut(self.end).as_mut_ptr();
                Some(element.read())
            }
        }
    }
}

impl<T> Drop for FragmentPool<T> {
    fn drop(&mut self) {
        // We have to drop all 'initialized' elements.
        while self.end > 0 {
            self.end -= 1;
            // Safety - this is safe as self.end is always within valid range and all elements
            // under self.end are initialized.
            unsafe {
                std::ptr::drop_in_place(self.pool.get_unchecked_mut(self.end).as_mut_ptr());
            }
        }
    }
}

#[cfg(test)]
mod fragment_pool_tests {
    use super::*;
    use std::cell::Cell;

    #[derive(Debug)]
    struct DestructTracker {
        destructed: Cell<bool>,
    }

    #[derive(Debug)]
    struct Destructable<'a> {
        tracker: &'a DestructTracker,
    }

    impl<'a> Drop for Destructable<'a> {
        fn drop(&mut self) {
            self.tracker.destructed.set(true);
        }
    }

    #[test]
    fn putpop() {
        let tracker = DestructTracker {
            destructed: Cell::new(false),
        };
        let item = Destructable { tracker: &tracker };
        let mut pool = FragmentPool::<Destructable>::new();

        pool.lossy_push(item);
        assert_eq!(tracker.destructed.get(), false);
        let item2 = pool.maybe_pop().unwrap();
        assert_eq!(tracker.destructed.get(), false);
        std::mem::drop(item2);
        assert_eq!(tracker.destructed.get(), true);
    }

    #[test]
    fn putdrop() {
        let tracker = DestructTracker {
            destructed: Cell::new(false),
        };
        let item = Destructable { tracker: &tracker };
        let mut pool = FragmentPool::<Destructable>::new();

        pool.lossy_push(item);
        assert_eq!(tracker.destructed.get(), false);
        std::mem::drop(pool);
        assert_eq!(tracker.destructed.get(), true);
    }

    #[test]
    fn put_a_lot() {
        let mut trackers = Vec::<DestructTracker>::new();
        for _ in 0..64 {
            trackers.push(DestructTracker {
                destructed: Cell::new(false),
            });
        }

        let extra_tracker = DestructTracker {
            destructed: Cell::new(false),
        };
        let mut pool = FragmentPool::<Destructable>::new();

        for i in 0..64 {
            let item = Destructable {
                tracker: trackers.get(i).unwrap(),
            };
            pool.lossy_push(item);
        }

        for i in 0..64 {
            assert_eq!(trackers.get(i).unwrap().destructed.get(), false);
        }

        let extra_item = Destructable {
            tracker: &extra_tracker,
        };

        pool.lossy_push(extra_item);
        assert_eq!(extra_tracker.destructed.get(), true);
        for i in 0..64 {
            std::mem::drop(pool.maybe_pop().unwrap());
            assert_eq!(trackers.get(63 - i).unwrap().destructed.get(), true);
        }
    }

    #[test]
    fn empty_pop() {
        let mut pool = FragmentPool::<Destructable>::new();
        pool.maybe_pop()
            .expect_none("popped something when there was nothing to pop");
    }
}

//// Generic container for a single logical 'packet'.
//pub struct Packet<T> {

//}
