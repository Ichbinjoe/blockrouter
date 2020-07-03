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
use core::task::Poll;
use std::collections::VecDeque;
use tokio::io::AsyncRead;
use tokio::io::AsyncReadExt;
use tokio::io::Result;
use tokio::net::tcp::ReadHalf;
use tokio::prelude::*;
use tokio::runtime::Runtime;

trait BufferSource<T: cursor::DirectBufMut> {
    fn singlebuffer(&self) -> T;
    //fn buffers(n: usize, vec: &mut VecDeque<T>);
}

struct ConnectionSource<'a> {
    rh: ReadHalf<'a>,
}

pub enum ReadResult<T: cursor::DirectBufMut> {
    Data(T),
    EOF,
}

impl<'a> ConnectionSource<'a> {
    async fn read<T: cursor::DirectBufMut, BS: BufferSource<T>>(
        &mut self,
        alloc: &BS,
    ) -> io::Result<ReadResult<T>> {
        let mut buf = alloc.singlebuffer();

        let amount_read = self.rh.read_buf(&mut buf).await?;

        if amount_read == 0 {
            // The other side hung up... what do we do here? This is a close
            Ok(ReadResult::EOF)
        } else {
            buf.truncate(amount_read);
            Ok(ReadResult::Data(buf))
        }
    }
}
