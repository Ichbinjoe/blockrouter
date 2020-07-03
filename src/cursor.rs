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

use ::bytes::{Buf, BufMut, Bytes, BytesMut};
use std::collections::VecDeque;
use std::io::IoSlice;

pub trait DirectBuf: bytes::Buf + std::convert::AsRef<[u8]> {
    fn split_to(&mut self, at: usize) -> Self;
    fn truncate(&mut self, len: usize);
}

impl DirectBuf for Bytes {
    fn truncate(&mut self, len: usize) {
        self.truncate(len)
    }

    fn split_to(&mut self, at: usize) -> Self {
        self.split_to(at)
    }
}

pub trait DirectBufMut: bytes::BufMut + DirectBuf + std::convert::AsMut<[u8]> {
    unsafe fn bytes_mut_assume_init(&mut self) -> &mut [u8];
}

impl DirectBuf for BytesMut {
    fn truncate(&mut self, len: usize) {
        self.truncate(len)
    }

    fn split_to(&mut self, at: usize) -> Self {
        self.split_to(at)
    }
}

impl DirectBufMut for BytesMut {
    unsafe fn bytes_mut_assume_init(&mut self) -> &mut [u8] {
        // look, if you thought this was safe you came to the wrong place
        std::mem::transmute(self.bytes_mut())
    }
}

pub trait SliceCursor: bytes::Buf {
    fn has_atleast(&self, len: usize) -> bool {
        self.remaining() >= len
    }
}

impl SliceCursor for Bytes {}

pub trait SliceCursorMut: BufMut + SliceCursor {}

impl SliceCursor for BytesMut {}

#[derive(Debug)]
pub struct Multibytes<T: DirectBuf> {
    pub(crate) b: VecDeque<T>,
}

#[derive(Clone, Copy, Debug, PartialOrd, PartialEq)]
pub struct Cursor {
    of: usize,
    i: usize,
}

impl Cursor {
    pub fn advance<T: DirectBuf>(&mut self, b: &Multibytes<T>, i: usize) -> bool {
        self.i += i;
        self.true_up(b)
    }

    pub fn true_up<T: DirectBuf>(&mut self, b: &Multibytes<T>) -> bool {
        loop {
            let r = match b.b.get(self.of) {
                Some(s) => s,
                None => {
                    // There is a special case where this cursor is valid, even if it doesn't point
                    // to a valid page - if we are pointing to the next not yet discovered page.
                    return self.of == b.b.len() && self.i == 0;
                }
            };
            let len = r.remaining();
            if self.i >= len {
                self.i -= len;
                self.of += 1;
            } else {
                return true;
            }
        }
    }

    pub fn remaining<T: DirectBuf>(&self, b: &Multibytes<T>) -> usize {
        let blen =
            b.b.iter()
                .skip(self.of)
                .fold(0, |prev, next| prev + next.remaining());
        if blen <= self.i {
            0
        } else {
            blen - self.i
        }
    }

    pub fn has_atleast<T: DirectBuf>(&self, b: &Multibytes<T>, len: usize) -> bool {
        let mut left = len + self.i;
        for buf in b.b.iter().skip(self.of) {
            let bl = buf.remaining();
            if bl >= left {
                return true;
            }
            left -= bl;
        }
        return left == 0;
    }

    pub fn bytes_vectored<'a, T: DirectBuf>(
        &self,
        mb: &'a Multibytes<T>,
        dst: &mut [IoSlice<'a>],
    ) -> usize {
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

        dst[0] = IoSlice::new(&first.as_ref()[self.i..]);

        // Others can just be slammed in there, no problems
        let mut i = 1;
        while let Some(item) = iter.next() {
            if item.remaining() == 0 {
                continue;
            }

            dst[i] = IoSlice::new(&item.as_ref()[..]);
            i += 1;
            if i >= dstlen {
                break;
            }
        }
        return i;
    }

    pub fn run_off_end<T: DirectBuf>(&self, b: &Multibytes<T>) -> usize {
        match b.b.get(self.of) {
            Some(p) => {
                // If this isn't the last page, then this will be in bounds (or it wasn't trued
                // up) and return 0. Otherwise, this will work properly.

                let len = p.remaining();
                if self.i > len {
                    self.i - len
                } else {
                    0
                }
            }
            None => {
                // This works. Why? Because when we run off the end, we are relegated to a new of,
                // which is imaginary
                self.i
            }
        }
    }
}

macro_rules! must_be_some {
    ($x:expr) => {
        match ($x) {
            Some(x) => x,
            None => panic!("wanted some, but got none. cursor error?"),
        }
    };
}

impl<T: DirectBuf> Multibytes<T> {
    pub fn new(b: VecDeque<T>) -> Multibytes<T> {
        Multibytes { b }
    }

    pub fn cursor(&self) -> Cursor {
        Cursor { of: 0, i: 0 }
    }

    pub fn append(&mut self, b: T) {
        self.b.push_back(b)
    }

    /// Before using this method, a Cursor should be 'trued up'
    pub fn split_to(&mut self, c: &Cursor) -> Self {
        // If our index into a buffer is 0, then we don't actually have to split it. We just have
        // to not carry it over
        let full_pages = match c.i {
            0 => {
                if c.of == 0 {
                    // this is a special case - the correct answer is to just give back a MB which
                    // is empty
                    return Multibytes { b: VecDeque::new() };
                }
                c.of - 1
            }
            _ => c.of,
        };

        let mut b = VecDeque::with_capacity(full_pages);

        for _i in 0..c.of {
            b.push_back(must_be_some!(self.b.pop_front()));
        }

        if c.i > 0 {
            match self.b.front_mut() {
                Some(x) => {
                    b.push_back(x.split_to(c.i));
                }
                None => {
                    if c.i != 0 {
                        panic!("Cursor steps into a page which does not exist");
                    }
                    // Otherwise, we do nothing - this is an 'end of the line' cursor
                }
            }
        }

        return Multibytes { b };
    }

    pub fn view<'a>(&'a self) -> MultibytesView<'a, T> {
        MultibytesView {
            b: self,
            c: self.cursor(),
        }
    }

    pub fn cursor_view<'a>(&'a self, c: Cursor) -> MultibytesView<'a, T> {
        MultibytesView { b: self, c }
    }

    pub fn indexed<'a>(self) -> IndexedMultibytes<T> {
        IndexedMultibytes {
            b: self,
            c: Cursor { of: 0, i: 0 },
        }
    }

    pub fn cursor_indexed<'a>(self, c: Cursor) -> IndexedMultibytes<T> {
        IndexedMultibytes { b: self, c }
    }
}

pub struct IndexedMultibytes<T: DirectBuf> {
    b: Multibytes<T>,
    c: Cursor,
}

impl<T: DirectBuf> Buf for IndexedMultibytes<T> {
    fn remaining(&self) -> usize {
        self.c.remaining(&self.b)
    }

    fn bytes(&self) -> &[u8] {
        match self.b.b.get(self.c.of) {
            Some(x) => &x.bytes()[self.c.i..],
            None => &[],
        }
    }

    fn advance(&mut self, cnt: usize) {
        self.c.advance(&self.b, cnt);
    }

    fn bytes_vectored<'b>(&'b self, dst: &mut [IoSlice<'b>]) -> usize {
        self.c.bytes_vectored(&self.b, dst)
    }
}

impl<T: DirectBuf> SliceCursor for IndexedMultibytes<T> {
    fn has_atleast(&self, len: usize) -> bool {
        self.c.has_atleast(&self.b, len)
    }
}

impl<T: DirectBuf> IndexedMultibytes<T> {
    pub fn cursor(&self) -> Cursor {
        self.c
    }

    pub fn dissolve(self) -> (Multibytes<T>, Cursor) {
        (self.b, self.c)
    }
}

pub struct MultibytesView<'a, T: DirectBuf> {
    b: &'a Multibytes<T>,
    c: Cursor,
}

impl<'a, T: DirectBuf> Buf for MultibytesView<'a, T> {
    fn remaining(&self) -> usize {
        self.c.remaining(self.b)
    }

    fn bytes(&self) -> &[u8] {
        match self.b.b.get(self.c.of) {
            Some(x) => &x.bytes()[self.c.i..],
            None => &[],
        }
    }

    fn advance(&mut self, cnt: usize) {
        self.c.advance(self.b, cnt);
    }

    fn bytes_vectored<'b>(&'b self, dst: &mut [IoSlice<'b>]) -> usize {
        self.c.bytes_vectored(self.b, dst)
    }
}

impl<'a, T: DirectBuf> SliceCursor for MultibytesView<'a, T> {
    fn has_atleast(&self, len: usize) -> bool {
        self.c.has_atleast(self.b, len)
    }
}

impl<'a, T: DirectBuf> Clone for MultibytesView<'a, T> {
    fn clone(&self) -> Self {
        MultibytesView {
            b: self.b,
            c: self.c.clone(),
        }
    }
}

impl<'a, T: DirectBuf> MultibytesView<'a, T> {
    pub fn cursor(&self) -> Cursor {
        self.c
    }
}

#[cfg(test)]
mod tests {
    mod a {
        use super::super::*;
        use bytes::{BufMut, BytesMut};

        #[test]
        fn slice_cursor_has_atleast() {
            let mut b = BytesMut::new();
            b.reserve(4);
            b.put_u32(4);
            assert!(b.has_atleast(3));
            assert!(!b.has_atleast(5));
        }
    }

    use super::*;
    use std::iter::FromIterator;

    fn make_test_mb() -> Multibytes<bytes::Bytes> {
        let slices = vec![
            vec![1, 2, 3, 4],
            vec![5, 6],
            vec![],
            vec![7, 8, 9],
            vec![10],
        ];
        Multibytes {
            b: VecDeque::from_iter(
                slices
                    .iter()
                    .map(|s| bytes::BytesMut::from_iter(s.iter()).freeze()),
            ),
        }
    }

    #[test]
    fn cursor_advance() {
        let mb = make_test_mb();
        let mut cursor = mb.cursor();

        assert!(cursor.advance(&mb, 3));
        assert!(cursor.advance(&mb, 7));
        assert!(!cursor.advance(&mb, 1));
    }

    #[test]
    fn cursor_remaining() {
        let mb = make_test_mb();
        let mut cursor = mb.cursor();

        assert_eq!(cursor.remaining(&mb), 10);
        cursor.advance(&mb, 3);
        assert_eq!(cursor.remaining(&mb), 7);
        cursor.advance(&mb, 7);
        assert_eq!(cursor.remaining(&mb), 0);
        cursor.advance(&mb, 1);
        assert_eq!(cursor.remaining(&mb), 0);
    }

    #[test]
    fn cursor_has_atleast() {
        let mb = make_test_mb();
        let mut cursor = mb.cursor();

        assert!(cursor.has_atleast(&mb, 0));
        assert!(cursor.has_atleast(&mb, 3));
        assert!(cursor.has_atleast(&mb, 9));
        assert!(cursor.has_atleast(&mb, 10));
        assert!(!cursor.has_atleast(&mb, 11));
        cursor.advance(&mb, 3);
        assert!(cursor.has_atleast(&mb, 7));
        cursor.advance(&mb, 7);
        assert!(!cursor.has_atleast(&mb, 1));
        assert!(cursor.has_atleast(&mb, 0));
        cursor.advance(&mb, 1);
        assert!(!cursor.has_atleast(&mb, 1));
        assert!(!cursor.has_atleast(&mb, 0));
    }

    #[test]
    fn cursor_bytes_vectored() {
        let mb = make_test_mb();
        let mut cursor = mb.cursor();

        let mut io = vec![
            IoSlice::new(&[]),
            IoSlice::new(&[]),
            IoSlice::new(&[]),
            IoSlice::new(&[]),
        ];

        // by definition we must return 0
        assert_eq!(cursor.bytes_vectored(&mb, &mut []), 0);

        assert_eq!(cursor.bytes_vectored(&mb, &mut io), 4);
        assert_eq!(io[0].to_vec(), vec![1, 2, 3, 4]);
        assert_eq!(io[1].to_vec(), vec![5, 6]);
        assert_eq!(io[2].to_vec(), vec![7, 8, 9]);
        assert_eq!(io[3].to_vec(), vec![10]);

        cursor.advance(&mb, 3);
        assert_eq!(cursor.bytes_vectored(&mb, &mut io), 4);
        assert_eq!(io[0].to_vec(), vec![4]);
        assert_eq!(io[1].to_vec(), vec![5, 6]);
        assert_eq!(io[2].to_vec(), vec![7, 8, 9]);
        assert_eq!(io[3].to_vec(), vec![10]);

        cursor.advance(&mb, 2);
        assert_eq!(cursor.bytes_vectored(&mb, &mut io), 3);
        assert_eq!(io[0].to_vec(), vec![6]);
        assert_eq!(io[1].to_vec(), vec![7, 8, 9]);
        assert_eq!(io[2].to_vec(), vec![10]);

        cursor.advance(&mb, 5);
        assert_eq!(cursor.bytes_vectored(&mb, &mut io), 0);
        // We shouldn't touch the slice if we report 0
        assert_eq!(io[0].to_vec(), vec![6]);
        assert_eq!(io[1].to_vec(), vec![7, 8, 9]);
        assert_eq!(io[2].to_vec(), vec![10]);

        cursor.advance(&mb, 1);
        assert_eq!(cursor.bytes_vectored(&mb, &mut io), 0);
        assert_eq!(cursor.bytes_vectored(&mb, &mut []), 0);
    }

    #[test]
    fn cursor_run_off_end() {
        let mut mb = make_test_mb();
        let mut cursor = mb.cursor();

        for _ in 0..10 {
            cursor.advance(&mb, 1);
            assert_eq!(cursor.run_off_end(&mb), 0);
        }

        cursor.advance(&mb, 1);
        assert_eq!(cursor.run_off_end(&mb), 1);
        mb.append(bytes::BytesMut::from_iter(vec![11, 12].iter()).freeze());
        assert_eq!(cursor.run_off_end(&mb), 0);
        cursor.advance(&mb, 101);
        assert_eq!(cursor.run_off_end(&mb), 100);
    }

    #[test]
    fn multibytes_split_to() {
        let mut mb = make_test_mb();
        let mut cursor = mb.cursor();

        let mb_empty = mb.split_to(&cursor);
        assert!(mb_empty.b.len() == 0);

        cursor.advance(&mb, 1);
        let mb_1 = mb.split_to(&cursor);
        assert_eq!(mb.b.len(), 5);
        assert_eq!(mb_1.b.len(), 1);
        assert_eq!(mb.b[0].bytes(), [2, 3, 4]);
        assert_eq!(mb_1.b[0].bytes(), [1]);
        // run with ASAN / valgrind to ensure bytes didn't mess up
        drop(mb_1);

        cursor = mb.cursor();
        cursor.advance(&mb, 3);

        let mb_2 = mb.split_to(&cursor);
        assert_eq!(mb.b.len(), 4);
        assert_eq!(mb_2.b.len(), 1);
        assert_eq!(mb.b[0].bytes(), [5, 6]);
        assert_eq!(mb_2.b[0].bytes(), [2, 3, 4]);
        // run with ASAN / valgrind to ensure bytes didn't mess up
        drop(mb_2);

        cursor = mb.cursor();
        cursor.advance(&mb, 3);

        let mb_3 = mb.split_to(&cursor);
        assert_eq!(mb.b.len(), 2);
        assert_eq!(mb_3.b.len(), 3);
        assert_eq!(mb.b[0].bytes(), [8, 9]);
        assert_eq!(mb_3.b[0].bytes(), [5, 6]);
        assert_eq!(mb_3.b[1].bytes(), []);
        assert_eq!(mb_3.b[2].bytes(), [7]);
        // run with ASAN / valgrind to ensure bytes didn't mess up
        drop(mb_3);

        cursor = mb.cursor();
        cursor.advance(&mb, 3);

        let mb_4 = mb.split_to(&cursor);
        assert_eq!(mb.b.len(), 0);
        assert_eq!(mb_4.b.len(), 2);
        assert_eq!(mb_4.b[0].bytes(), [8, 9]);
        assert_eq!(mb_4.b[1].bytes(), [10]);
        // run with ASAN / valgrind to ensure bytes didn't mess up
        drop(mb_4);
    }
}
