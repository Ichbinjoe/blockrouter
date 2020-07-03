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

use super::compress::Inflater;
use super::crypto::Cryptor;
use super::cursor;
use super::framer;
use super::mempool;
use super::parser;
use crate::zlib;

use std::iter::Iterator;

enum PacketDriverError {
    FramerError(framer::FrameError),
    CompressionSizeDecodeFail,
    SmallCompression,
    ZlibError(zlib::ZLibError),
}

impl From<framer::FrameError> for PacketDriverError {
    fn from(f: framer::FrameError) -> PacketDriverError {
        PacketDriverError::FramerError(f)
    }
}

enum InflaterError {
    CompressionSizeDecodeFail,
    SmallCompression,
    ZlibError(zlib::ZLibError),
}

impl From<zlib::ZLibError> for InflaterError {
    fn from(z: zlib::ZLibError) -> InflaterError {
        InflaterError::ZlibError(z)
    }
}

enum PacketizerError {
    Inflater(InflaterError),
    Framer(framer::FrameError),
}

impl From<InflaterError> for PacketizerError {
    fn from(z: InflaterError) -> PacketizerError {
        PacketizerError::Inflater(z)
    }
}

impl From<framer::FrameError> for PacketizerError {
    fn from(z: framer::FrameError) -> PacketizerError {
        PacketizerError::Framer(z)
    }
}

enum DataBacking<T: cursor::DirectBuf> {
    Cursor(cursor::Cursor),
    Multibytes(cursor::Multibytes<T>),
}

struct Packet<T: cursor::DirectBuf> {
    h: cursor::Multibytes<T>,
    d: DataBacking<T>,
}

struct InflateState {
    threshold: i32,
    inflater: Inflater,
}

struct PacketInflater {
    inflate: Option<InflateState>,
}

impl PacketInflater {
    fn inflate<'a, T: cursor::DirectBufMut, Alloc: mempool::BlockAllocator<'a, T>>(
        &mut self,
        frame: framer::Frame<T>,
        alloc: &'a Alloc,
    ) -> Result<Packet<T>, InflaterError> {
        if let Some(compress) = self.inflate.as_mut() {
            let indexed = frame.packet.cursor_indexed(frame.data_start);
            let result = parser::varint(indexed);
            match result {
                Ok((compressed_data, decompressed_size)) => {
                    if decompressed_size == 0 {
                        // No compression actually happened, just dump the packet back
                        let (data, cursor) = compressed_data.dissolve();
                        Ok(Packet {
                            h: data,
                            d: DataBacking::Cursor(cursor),
                        })
                    } else if decompressed_size < compress.threshold {
                        // This is an error, protocol dictates we should yeet the client at the
                        // other end for daring to send us such misformatted data
                        Err(InflaterError::SmallCompression)
                    } else {
                        // Segment the header from the data so that we can decompress the data
                        let (mut data, cursor) = compressed_data.dissolve();
                        let header = data.split_to(&cursor);

                        // frame.packet now contains the compressed data
                        let inflated = compress.inflater.process(data, alloc)?;

                        Ok(Packet {
                            h: header,
                            d: DataBacking::Multibytes(inflated),
                        })
                    }
                }
                _ => Err(InflaterError::CompressionSizeDecodeFail),
            }
        } else {
            // No compression, this packet can simply be passed along
            Ok(Packet {
                h: frame.packet,
                d: DataBacking::Cursor(frame.data_start),
            })
        }
    }
}

struct Packetizer<T: cursor::DirectBuf> {
    crypto: Cryptor,
    framer: framer::Framer<T>,
    inflater: PacketInflater,
}

struct PacketIterator<'a, T: cursor::DirectBuf> {
    f: framer::FrameIter<'a, T>,
    inflater: &'a mut PacketInflater,
}

impl<'a, T: cursor::DirectBufMut> PacketIterator<'a, T> {
    fn next<Allocator: mempool::BlockAllocator<'a, T>>(
        &mut self,
        alloc: &'a Allocator,
    ) -> Result<Packet<T>, PacketizerError> {
        let frame = self.f.next()?;
        let packet = self.inflater.inflate(frame, alloc)?;
        Ok(packet)
    }
}

impl<T: cursor::DirectBufMut> Packetizer<T> {
    fn process<'a>(&'a mut self, mut buf: T) -> PacketIterator<'a, T> {
        // Do the decryption
        // Why is this unsafe? This is telling the compiler to go fuck itself because we know the
        // data held within is initialized fine. This is an inplace operation on what should be
        // already trimmed data (from whatever handed us this buf to begin with)
        self.crypto.process(unsafe { buf.bytes_mut_assume_init() });

        let f;
        {
            f = self.framer.frame(buf);
        }

        PacketIterator {
            inflater: &mut self.inflater,
            f,
        }
    }
}
