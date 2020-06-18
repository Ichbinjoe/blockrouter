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

use super::cursor;
use super::mempool;
use super::zlib;

use std::collections::VecDeque;

use bytes::Buf;

pub struct MbZlibOp<
    'g,
    Op: zlib::ZlibOperator,
    T: cursor::DirectBufMut,
    Allocator: mempool::BlockAllocator<'g, T>,
> {
    z: Op,
    allocator: &'g Allocator,
    pd: std::marker::PhantomData<T>,
}

impl<'g, T: cursor::DirectBufMut, Allocator: mempool::BlockAllocator<'g, T>>
    MbZlibOp<'g, zlib::Deflate, T, Allocator>
{
    pub fn deflate(level: i32, allocator: &'g Allocator) -> Result<Self, zlib::ZLibError> {
        let deflate = zlib::Deflate::new(level)?;
        Ok(MbZlibOp {
            z: deflate,
            allocator,
            pd: std::marker::PhantomData,
        })
    }
}

impl<'g, T: cursor::DirectBufMut, Allocator: mempool::BlockAllocator<'g, T>>
    MbZlibOp<'g, zlib::Inflate, T, Allocator>
{
    pub fn inflate(allocator: &'g Allocator) -> Result<Self, zlib::ZLibError> {
        let inflate = zlib::Inflate::new()?;
        Ok(MbZlibOp {
            z: inflate,
            allocator,
            pd: std::marker::PhantomData,
        })
    }
}

impl<
        'g,
        Op: zlib::ZlibOperator,
        T: cursor::DirectBufMut,
        Allocator: mempool::BlockAllocator<'g, T>,
    > MbZlibOp<'g, Op, T, Allocator>
{
    unsafe fn set_in(&mut self, buf: &T) {
        let b = buf.bytes();
        self.z.strm_mut().next_in = b.as_ptr().clone();
        self.z.strm_mut().avail_in = b.len() as u32;
    }

    unsafe fn set_out(&mut self, buf: &mut T) {
        let b = buf.bytes();
        self.z.strm_mut().next_out = b.as_ptr().clone() as *mut u8;
        self.z.strm_mut().avail_out = b.len() as u32;
    }

    pub fn process(
        &mut self,
        mut b: cursor::Multibytes<T>,
    ) -> Result<cursor::Multibytes<T>, zlib::ZLibError> {
        let mut buf_in = match b.b.pop_front() {
            Some(x) => x,
            None => return Ok(b), // Nothing to do, abort!
        };

        let mut buf_out = self.allocator.allocate();

        // Set up the zlib shit
        // This is unsafe because we need to keep the buffers in frame without dropping them while
        // we are doing zlib operations
        unsafe {
            self.set_in(&buf_in);
            self.set_out(&mut buf_out);
        }

        let mut vd = VecDeque::new();

        loop {
            if let Some(err) = self.z.process(zlib::FlushMode::SyncFlush) {
                return Err(err);
            }

            if self.z.strm().avail_in == 0 {
                // Try to pop again
                if let Some(new_buf_in) = b.b.pop_front() {
                    buf_in = new_buf_in;
                    unsafe {
                        self.set_in(&buf_in);
                    }
                } else {
                    break;
                }
            }

            if self.z.strm().avail_out == 0 {
                let old_buf = std::mem::replace(&mut buf_out, self.allocator.allocate());
                unsafe {
                    self.set_out(&mut buf_out);
                }

                vd.push_back(old_buf);
            }
        }

        let trail_size = buf_out.remaining() as u32 - self.z.strm().avail_out;

        if trail_size > 0 {
            buf_out.truncate(trail_size as usize);
            vd.push_back(buf_out);
        }

        Ok(cursor::Multibytes::new(vd))
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::mempool::BlockAllocator;
    use bytes::Buf;

    global_mempool_tlmp!(bidirectional_smoke_test_tlmp, 16);

    #[test]
    fn bidirectional_smoke_test() {
        let alloc = mempool::GlobalMemPool::new(
            &bidirectional_smoke_test_tlmp,
            mempool::GlobalMemPoolSettings {
                buf_size: 8,
                page_entries: 128,
                concurrent_allocation_limit: 1,
            },
        );

        let mut deflate = MbZlibOp::deflate(5, &alloc).expect("could not init deflate");
        let mut inflate = MbZlibOp::inflate(&alloc).expect("could not init inflate");

        let mut buffer = alloc.allocate();
        for i in 0..buffer.remaining() {
            buffer[i] = (i % 16) as u8;
        }

        let mut vd = VecDeque::new();
        vd.push_back(buffer);
        let mb = cursor::Multibytes::new(vd);

        let compressed = deflate.process(mb).expect("could not deflate");
        assert_eq!(28, compressed.cursor().remaining(&compressed));
        let reinflated = inflate.process(compressed).expect("could not inflate");
        let mut v = reinflated.view();
        for i in 0..252 {
            assert_eq!(i % 16 as u8, v.get_u8());
        }
    }

    extern crate test;
    use test::Bencher;
    global_mempool_tlmp!(bench_deflate_inflate_cycle_tlmp, 16);
    #[bench]
    fn bench_deflate_inflate_cycle(b: &mut Bencher) {
        let alloc = mempool::GlobalMemPool::new(
            &bench_deflate_inflate_cycle_tlmp,
            mempool::GlobalMemPoolSettings {
                buf_size: 8,
                page_entries: 128,
                concurrent_allocation_limit: 1,
            },
        );

        let mut deflate = MbZlibOp::deflate(1, &alloc).expect("could not init deflate");
        let mut inflate = MbZlibOp::inflate(&alloc).expect("could not init inflate");

        let mut buffer = alloc.allocate();
        for i in 0..buffer.remaining() {
            buffer[i] = (i % 16) as u8;
        }

        let mut vd = VecDeque::new();
        vd.push_back(buffer);
        let mut mb = Some(cursor::Multibytes::new(vd));
        // There has to be a better way to do this...
        b.iter(|| {
            for _i in 0..1000 {
                let compressed = deflate
                    .process(mb.take().unwrap())
                    .expect("could not deflate");
                mb = Some(inflate.process(compressed).expect("could not inflate"));
            }
        });
    }
}
