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

use std::collections::VecDeque;

use super::cursor;
use super::parser;

#[derive(Debug)]
pub struct Frame<T: cursor::DirectBuf> {
    pub packet: cursor::Multibytes<T>,
    pub data_start: cursor::Cursor,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum FrameError {
    /// We are waiting for a size header. It should be finished in a few bytes, but since we don't
    /// know, we won't hint a size.
    WaitingForHeader,
    /// We are waiting on the rest of packet data. usize is the amount of data we expect to finish
    /// off this packet.
    WaitingForData(usize),
    /// This should be considered fatal - something we didn't expect happened.
    DecodeError,
}

struct TailingDataState {
    data_start: cursor::Cursor,
    data_end: cursor::Cursor,
}

enum FramerState {
    /// Offset into first buffer in the ring which the Varint would start
    WaitingForHeader,
    WaitingForTailingData(TailingDataState),
}

pub struct Framer<T: cursor::DirectBuf> {
    pub max_frame_size: usize,
    ring: cursor::Multibytes<T>,
    state: FramerState,
}

impl<T: cursor::DirectBuf> Framer<T> {
    pub fn new(max_frame_size: usize, buffer_size: usize) -> Self {
        Framer {
            max_frame_size,
            ring: cursor::Multibytes::new(VecDeque::with_capacity(buffer_size)),
            state: FramerState::WaitingForHeader,
        }
    }

    pub fn push_buffer(&mut self, b: T) {
        self.ring.append(b);
    }

    pub fn frame(&mut self) -> Result<Frame<T>, FrameError> {
        match &mut self.state {
            FramerState::WaitingForHeader => {
                // Attempt to decode a header
                let header_view = self.ring.view();
                match parser::varint(header_view) {
                    Ok((view, len)) => {
                        if len < 0 || len as usize > self.max_frame_size {
                            return Err(FrameError::DecodeError);
                        }

                        let data_start = view.cursor();
                        let mut data_end = data_start.clone();
                        let valid = data_end.advance(&self.ring, len as usize);

                        // If this is valid, then we can split the framer ring and spit out a
                        // frame
                        if valid {
                            return Ok(Frame {
                                packet: self.ring.split_to(&data_end),
                                // This cursor is still valid - it will always be less than
                                // data_end
                                data_start,
                            });
                        // the state right now is WaitingForHeader, which is correct for
                        // whenever this gets called again
                        } else {
                            // doesn't look like we have all the data quite yet, set our state
                            // and exit
                            self.state = FramerState::WaitingForTailingData(TailingDataState {
                                data_start,
                                data_end,
                            });

                            return Err(FrameError::WaitingForData(
                                data_end.run_off_end(&self.ring),
                            ));
                        }
                    }
                    Err(nom::Err::Incomplete(_)) => {
                        // We don't have enough, no progression.
                        return Err(FrameError::WaitingForHeader);
                    }
                    Err(nom::Err::Error(_)) | Err(nom::Err::Failure(_)) => {
                        // The parser probably overran - whatever is on the other end of this
                        // sent us bad data. Fatal the framer
                        return Err(FrameError::DecodeError);
                    }
                }
            }
            FramerState::WaitingForTailingData(state) => {
                // We already have a header, but need to wait for the rest of the data to come
                // in

                let valid = state.data_end.true_up(&self.ring);
                if valid {
                    let f = Frame {
                        packet: self.ring.split_to(&state.data_end),
                        // This cursor is still valid - it will always be less than
                        // data_end
                        data_start: state.data_start,
                    };
                    self.state = FramerState::WaitingForHeader;
                    return Ok(f);
                } else {
                    return Err(FrameError::WaitingForData(
                        state.data_end.run_off_end(&self.ring),
                    ));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::iter::FromIterator;

    macro_rules! to_buf {
        ($x: expr) => {
            bytes::BytesMut::from_iter($x.iter()).freeze()
        };
    }

    fn varint_len(mut v: usize) -> usize {
        let mut i = 1;
        loop {
            v >>= 7;
            if v == 0 {
                return i;
            } else {
                i += 1;
            }
        }
    }

    macro_rules! validate_frame {
        ($frame: expr, $len: expr) => {
            let f = $frame;
            let c = f.packet.cursor();
            assert_eq!(c.remaining(&f.packet), varint_len($len) + $len);
        };
    }

    #[test]
    fn max_frame_size() {
        let mut f = Framer::new(128, 1);
        // Prefix length of 129
        let b = to_buf!([0x80, 0x02]);
        f.push_buffer(b);
        assert_eq!(f.frame().unwrap_err(), FrameError::DecodeError);
    }

    #[test]
    fn invalid_varint() {
        let mut f = Framer::new(128, 1);
        // Invalid varint should result in an error
        let b = to_buf!([0x80, 0x80, 0x80, 0x80, 0x80, 0x02]);
        f.push_buffer(b);
        assert_eq!(f.frame().unwrap_err(), FrameError::DecodeError);
    }

    #[test]
    fn single_frame() {
        let mut f = Framer::new(128, 1);
        let b = to_buf!([0x3, 0x0, 0x1, 0x2]);
        f.push_buffer(b);

        let packet1 = f.frame().unwrap();
        validate_frame!(&packet1, 3);
        let eof = f.frame();
        assert_eq!(eof.unwrap_err(), FrameError::WaitingForHeader);
    }

    #[test]
    fn single_frame_multi_invoke() {
        let mut f = Framer::new(128, 1);
        f.push_buffer(to_buf!([0x3]));
        assert_eq!(f.frame().unwrap_err(), FrameError::WaitingForData(3));

        f.push_buffer(to_buf!([0x0]));
        assert_eq!(f.frame().unwrap_err(), FrameError::WaitingForData(2));

        f.push_buffer(to_buf!([0x1]));
        assert_eq!(f.frame().unwrap_err(), FrameError::WaitingForData(1));

        f.push_buffer(to_buf!([0x2]));
        validate_frame!(f.frame().unwrap(), 3);
        assert_eq!(f.frame().unwrap_err(), FrameError::WaitingForHeader);
    }

    #[test]
    fn multi_frame_single_invoke() {
        let mut f = Framer::new(128, 1);
        let b = to_buf!([0x3, 0x0, 0x1, 0x2, 0x2, 0x0, 0x1]);
        f.push_buffer(b);
        validate_frame!(f.frame().unwrap(), 3);
        validate_frame!(f.frame().unwrap(), 2);
        assert_eq!(f.frame().unwrap_err(), FrameError::WaitingForHeader);
    }

    #[test]
    fn odd_partition() {
        let mut f = Framer::new(128, 1);
        f.push_buffer(to_buf!([0x3, 0x0, 0x1]));
        assert_eq!(f.frame().unwrap_err(), FrameError::WaitingForData(1));

        f.push_buffer(to_buf!([0x2, 0x2, 0x0, 0x1, 0x4, 0x0]));
        validate_frame!(f.frame().unwrap(), 3);
        validate_frame!(f.frame().unwrap(), 2);
        assert_eq!(f.frame().unwrap_err(), FrameError::WaitingForData(3));
    }
}
