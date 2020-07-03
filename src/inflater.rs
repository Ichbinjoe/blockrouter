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
use super::cursor;
use super::framer;
use super::mempool;
use super::parser;
use crate::zlib;

#[derive(Debug, PartialEq)]
pub enum InflaterError {
    CompressionSizeDecodeFail,
    SmallCompression,
    ZlibError(zlib::ZLibError),
}

impl From<zlib::ZLibError> for InflaterError {
    fn from(z: zlib::ZLibError) -> InflaterError {
        InflaterError::ZlibError(z)
    }
}

pub enum DataBacking<T: cursor::DirectBuf> {
    Cursor(cursor::Cursor),
    Multibytes(cursor::Multibytes<T>),
}

pub struct Packet<T: cursor::DirectBuf> {
    h: cursor::Multibytes<T>,
    d: DataBacking<T>,
}

struct InflateState {
    threshold: i32,
    inflater: Inflater,
}

pub struct PacketInflater {
    inflate: Option<InflateState>,
}

impl PacketInflater {
    pub fn new() -> PacketInflater {
        PacketInflater { inflate: None }
    }

    pub fn inflate<'a, T: cursor::DirectBufMut, Alloc: mempool::BlockAllocator<'a, T>>(
        &mut self,
        frame: framer::Frame<T>,
        alloc: &'a Alloc,
    ) -> Result<Packet<T>, InflaterError> {
        if let Some(compress) = &mut self.inflate {
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
                        // TODO: Constrain inflation to the size that was given us - this trusts
                        // user input :(
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

    pub fn start_compression(&mut self, threshold: i32) -> Result<(), zlib::ZLibError> {
        self.inflate = Some(InflateState {
            threshold: threshold,
            inflater: Inflater::inflate()?,
        });

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::iter::FromIterator;

    fn frame_of(s: Vec<u8>) -> framer::Frame<bytes::BytesMut> {
        let b = bytes::BytesMut::from_iter(s.iter());
        let mut vd = std::collections::VecDeque::new();
        vd.push_back(b);
        let mb = cursor::Multibytes::new(vd);
        let c = mb.cursor();
        framer::Frame {
            packet: mb,
            data_start: c,
        }
    }

    #[test]
    fn packetinflater_no_inflater() {
        let alloc = mempool::SystemMemPool { buf_size: 12 };
        let mut inflater = PacketInflater::new();
        let frame = frame_of(vec![0x1, 0x0]);
        let result = inflater.inflate(frame, &alloc).unwrap();
        if let DataBacking::Cursor(c) = result.d {
            assert_eq!(c.remaining(&result.h), 2);
        } else {
            panic!("non-cursor");
        }
    }

    #[test]
    fn packetinflater_too_small_0() {
        let alloc = mempool::SystemMemPool { buf_size: 12 };
        let mut inflater = PacketInflater::new();
        inflater.start_compression(64).unwrap();

        let frame = frame_of(vec![0x0, 0x3, 0x3]);
        let result = inflater.inflate(frame, &alloc).unwrap();
        if let DataBacking::Cursor(c) = result.d {
            assert_eq!(c.remaining(&result.h), 2);
        } else {
            panic!("non-cursor");
        }
    }

    #[test]
    fn packetinflater_small_compression() {
        let alloc = mempool::SystemMemPool { buf_size: 12 };
        let mut inflater = PacketInflater::new();
        inflater.start_compression(64).unwrap();

        let frame = frame_of(vec![0x1, 0x3, 0x3]);
        let result = inflater.inflate(frame, &alloc);
        if let Err(e) = result {
            assert_eq!(e, InflaterError::SmallCompression);
        } else {
            panic!("valid response");
        }
    }

    #[test]
    fn packetinflater_bad_varint() {
        let alloc = mempool::SystemMemPool { buf_size: 12 };
        let mut inflater = PacketInflater::new();
        inflater.start_compression(64).unwrap();

        let frame = frame_of(vec![0xff, 0xff, 0xff, 0xff, 0xff, 0x3, 0x3]);
        let result = inflater.inflate(frame, &alloc);
        if let Err(e) = result {
            assert_eq!(e, InflaterError::CompressionSizeDecodeFail);
        } else {
            panic!("valid response");
        }
    }

    use bytes::Buf;

    #[test]
    fn packetinflater_normal_compression() {
        let alloc = mempool::SystemMemPool { buf_size: 12 };
        let mut inflater = PacketInflater::new();
        inflater.start_compression(3).unwrap();

        // lol this isn't efficient
        let frame = frame_of(vec![0x4, 120, 156, 99, 100, 98, 102, 1, 0, 0, 24, 0, 11]);
        let result = inflater.inflate(frame, &alloc).unwrap();
        if let DataBacking::Multibytes(mb) = result.d {
            let mut view = mb.view();
            assert_eq!(view.get_u8(), 0x1);
            assert_eq!(view.get_u8(), 0x2);
            assert_eq!(view.get_u8(), 0x3);
            assert_eq!(view.get_u8(), 0x4);
            assert_eq!(view.remaining(), 0);
        } else {
            panic!("non-mb");
        }
    }

    /*
    #[test]
    fn packetizer_normal() {
        let mut packetizer = Packetizer::<bytes::BytesMut> {
            crypto: super::Cryptor::new_decrypt(),
            framer: super::framer::Framer::new(64, 16),
            inflater: PacketInflater { inflate: Cell::new(None) },
        };

        let alloc = mempool::SystemMemPool { buf_size: 12 };
        let buf = buf_of(vec![
            // Packet 1 has a length of 1, uncompressed.
            0x4, 0x1, 0x0, 0x1, 0x2,
            // turn compression on
            // Packet 2 is too small for compression, and is valid
            0x3, 0x0, 0x1, 0x2, // Packet 3 is compressed.
            13, 0x4, 120, 156, 99, 100, 98, 102, 1, 0, 0, 24, 0, 11,
        ]);

        let mut iter = packetizer.process(buf);
        {
            let packet = iter.next(&alloc).unwrap();
            if let DataBacking::Cursor(c) = packet.d {
                assert_eq!(c.remaining(&packet.h), 4);
            } else {
                panic!("unexpected db type");
            }
        }

        packetizer.start_compression(3).unwrap();

        let packet2 = iter.next(&alloc).unwrap();
        if let DataBacking::Cursor(c) = packet2.d {
            assert_eq!(c.remaining(&packet2.h), 2);
        } else {
            panic!("unexpected db type");
        }

        let packet3 = iter.next(&alloc).unwrap();
        if let DataBacking::Multibytes(mb) = packet3.d {
            let mut view = mb.view();
            assert_eq!(view.get_u8(), 0x1);
            assert_eq!(view.get_u8(), 0x2);
            assert_eq!(view.get_u8(), 0x3);
            assert_eq!(view.get_u8(), 0x4);
            assert_eq!(view.remaining(), 0);
        } else {
            panic!("unexpected db type");
        }
    }*/
}
