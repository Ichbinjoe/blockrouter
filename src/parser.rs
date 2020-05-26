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
use nom::*;

pub enum VarintParseFail {
    VarintExceededShift(usize),
}

macro_rules! varint_decode {
    ($input:expr, $max_shift:expr, $typ:ty) => {{
        let mut i = 0;
        let mut result: $typ = 0;
        loop {
            if !$input.has_atleast(1) {
                return Err(nom::Err::Incomplete(Needed::Unknown));
            }
            let read = $input.get_u8();
            result |= ((read & 0x7f) as $typ) << i;
            i += 7;
            if i >= $max_shift {
                return Err(nom::Err::Error(VarintParseFail::VarintExceededShift(
                    $max_shift,
                )));
            }
            if result & 0x80 == 0x00 {
                return Ok(($input, result));
            }
        }
    }};
}

pub fn varint<T: cursor::SliceCursor>(mut b: T) -> IResult<T, i32, VarintParseFail> {
    varint_decode!(b, 32, i32)
}

pub fn varlong<T: cursor::SliceCursor>(mut b: T) -> IResult<T, i64, VarintParseFail> {
    varint_decode!(b, 64, i64);
}
