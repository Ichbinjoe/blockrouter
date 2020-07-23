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

use bytes::{Buf, BufMut, Bytes, BytesMut};

pub trait DirectBuf: Buf + std::convert::AsRef<[u8]> {
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
