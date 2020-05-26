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

use ::bytes::Bytes;

use std::collections::VecDeque;

use super::cursor;
use super::parser;

pub struct Frame {
    packet: cursor::Multibytes,
    data_start: cursor::Cursor,
}

pub struct FrameIter<'a> {
    framer: &'a mut Framer,
}

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

impl<'a> FrameIter<'a> {
    fn next(&'a mut self) -> Result<Frame, FrameError> {
        match &mut self.framer.state {
            FramerState::WaitingForHeader => {
                // Attempt to decode a header
                let header_view = self.framer.ring.view();
                match parser::varint(header_view) {
                    Ok((view, len)) => {
                        if len < 0 || len as usize > self.framer.max_frame_size {
                            return Err(FrameError::DecodeError);
                        }

                        let data_start = view.cursor();
                        let mut data_end = data_start.clone();
                        let valid = data_end.advance(&self.framer.ring, len as usize);

                        // If this is valid, then we can split the framer ring and spit out a
                        // frame
                        if valid {
                            return Ok(Frame {
                                packet: self.framer.ring.partition_before(&data_end),
                                // This cursor is still valid - it will always be less than
                                // data_end
                                data_start: data_start,
                            });
                        // the state right now is WaitingForHeader, which is correct for
                        // whenever this gets called again
                        } else {
                            // doesn't look like we have all the data quite yet, set our state
                            // and exit
                            self.framer.state =
                                FramerState::WaitingForTailingData(TailingDataState {
                                    data_start: data_start,
                                    data_end: data_end,
                                });

                            return Err(FrameError::WaitingForData(
                                data_end.run_off_end(&self.framer.ring),
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

                let valid = state.data_end.true_up(&self.framer.ring);
                if valid {
                    let f = Frame {
                        packet: self.framer.ring.partition_before(&state.data_end),
                        // This cursor is still valid - it will always be less than
                        // data_end
                        data_start: state.data_start,
                    };
                    self.framer.state = FramerState::WaitingForHeader;
                    return Ok(f);
                } else {
                    return Err(FrameError::WaitingForData(
                        state.data_end.run_off_end(&self.framer.ring),
                    ));
                }
            }
        }
    }
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

pub struct Framer {
    pub max_frame_size: usize,
    ring: cursor::Multibytes,
    state: FramerState,
}

impl Framer {
    fn new(max_frame_size: usize, buffer_size: usize) -> Framer {
        Framer {
            max_frame_size,
            ring: cursor::Multibytes::new(VecDeque::with_capacity(buffer_size)),
            state: FramerState::WaitingForHeader,
        }
    }

    fn frame<'a>(&'a mut self, b: Bytes) -> FrameIter<'a> {
        self.ring.append(b);
        FrameIter { framer: self }
    }
}
