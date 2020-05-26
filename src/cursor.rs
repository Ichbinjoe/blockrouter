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

use ::bytes::{Buf};
use std::collections::VecDeque;
use std::io::IoSlice;

pub trait SliceCursor: bytes::Buf + Clone{
    fn has_atleast(&self, len: usize) -> bool {
        self.remaining() > len
    }
}

pub trait SliceCursorMut: bytes::BufMut + SliceCursor {}

pub struct Multibytes {
    b: VecDeque<bytes::Bytes>,
}

#[derive(Clone, Copy, PartialOrd, PartialEq)]
pub struct Cursor {
    of: usize,
    i: usize,
}

impl Cursor {
    pub fn advance(&mut self, b: &Multibytes, i: usize) -> bool {
        self.i += i;
        self.true_up(b)
    }

    pub fn true_up(&mut self, b: &Multibytes) -> bool {
        loop {
            let r = match b.b.get(self.of) {
                Some(s) => s,
                None => {
                    // There is a special case where this cursor is valid, even if it doesn't point
                    // to a valid page - if we are pointing to the next not yet discovered page.
                    return self.of == b.b.len() && self.i == 0
                }
            };
            let len = r.len();
            if self.i >= len {
                self.i -= len;
                self.of += 1;
            } else {
                return true
            }
        }
    }

    pub fn remaining(&self, b: &Multibytes) -> usize {
        let blen =
            b.b.iter()
                .skip(self.of)
                .fold(0, |prev, next| prev + next.len());
        if blen <= self.i {
            0
        } else {
            blen - self.i
        }
    }

    pub fn has_atleast(&self, b: &Multibytes, len: usize) -> bool {
        let left = len + self.i;
        for buf in &b.b {
            let bl = buf.len();
            if bl > left {
                return true;
            }
        }
        return false;
    }

    pub fn bytes_vectored<'a>(&self, mb: &'a Multibytes, dst: &mut [IoSlice<'a>]) -> usize {
        let dstlen = dst.len();
        if dstlen < 1 {
            return 0;
        }

        let mut iter = mb.b.iter().skip(self.of);
        // The first element is special - we have to clip some items from the beginning for it to
        // work
        let first = match iter.next() {
            Some(s) => s,
            None => return 0,
        };

        dst[0] = IoSlice::new(&first[self.i..]);

        // Others can just be slammed in there, no problems
        let mut i = 1;
        while let Some(item) = iter.next() {
            dst[i] = IoSlice::new(&item[..]);
            i += 1;
            if i >= dstlen {
                break;
            }
        }
        return i;
    }

    pub fn run_off_end(&self, b: &Multibytes) -> usize {
        match b.b.get(self.of) {
            Some(p) => {
                // If this isn't the last page, then this will be in bounds (or it wasn't trued
                // up) and return 0. Otherwise, this will work properly.
               
                let len = p.len();
                if self.i > len {
                    self.i - len
                } else {
                    0
                }

            },
            None => {
                if self.i == 0 {
                    0
                } else {
                    panic!("cursor error - attempted to speculate run_off_end but referenced a page which did not exist making this impossible")
                }
            }
        }
    }
}

macro_rules! must_be_some {
    ($x:expr) => {
        match ($x) {
            Some(x) => x,
            None => panic!("wanted some, but got none. cursor error?")
        }
    }
}

impl Multibytes {
    pub fn new(b: VecDeque<bytes::Bytes>) -> Multibytes {
        Multibytes{b}
    }

    pub fn cursor(&self) -> Cursor {
        Cursor{of: 0, i: 0}
    }

    pub fn append(&mut self, b: bytes::Bytes) {
        self.b.push_back(b)
    }

    /// Before using this method, a Cursor should be 'trued up'
    pub fn partition_before(&mut self, c: &Cursor) -> Multibytes {
        // If our index into a buffer is 0, then we don't actually have to split it. We just have
        // to not carry it over
        let full_pages = match c.i {
            0 => c.of - 1,
            _ => c.of
        };

        let mut b = VecDeque::with_capacity(full_pages);
        for _i in 0..c.of - 1 {
            b.push_back(must_be_some!(self.b.pop_front()));
        }

        match self.b.front_mut() {
            Some(x) => {
                b.push_back(x.split_to(c.i));
            },
            None => {
                if c.i != 0 {
                    panic!("Cursor steps into a page which does not exist");
                }
                // Otherwise, we do nothing - this is an 'end of the line' cursor
            }
        }

        return Multibytes{b}
    }

    pub fn view<'a>(&'a self) -> MultibytesView<'a> {
        MultibytesView{
            b: self,
            c: self.cursor()
        }
    }
}

pub struct MultibytesView<'a> {
    b: &'a Multibytes,
    c: Cursor,
}

impl <'a> Buf for MultibytesView<'a> {
    fn remaining(&self) -> usize {
        self.c.remaining(self.b)
    }

    fn bytes(&self) -> &[u8] {
        match self.b.b.get(self.c.of) {
            Some(x) => {
                &x.bytes()[self.c.i..]
            }, None => {
                &[]
            }
        }
    }

    fn advance(&mut self, cnt: usize) {
        self.c.advance(self.b, cnt);
    }

    fn bytes_vectored<'b>(&'b self, dst: &mut [IoSlice<'b>]) -> usize {
        self.c.bytes_vectored(self.b, dst)
    }
}

impl <'a> SliceCursor for MultibytesView<'a> {
    fn has_atleast(&self, len: usize) -> bool {
        self.c.has_atleast(self.b, len)
    }
}

impl <'a> Clone for MultibytesView<'a> {
    fn clone(&self) -> Self {
        MultibytesView{
            b: self.b,
            c: self.c.clone()
        }
    }
}

impl <'a> MultibytesView<'a> {
    pub fn cursor(&self) -> Cursor {
        self.c
    }
}
