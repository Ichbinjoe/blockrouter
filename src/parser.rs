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

#[derive(Debug, PartialEq)]
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
            if read & 0x80 == 0x00 {
                return Ok(($input, result));
            }

            i += 7;
            if i > $max_shift {
                return Err(nom::Err::Error(VarintParseFail::VarintExceededShift(
                    $max_shift,
                )));
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

#[cfg(test)]
mod test {
    use super::*;
    use ::bytes::BytesMut;
    use std::iter::FromIterator;

    macro_rules! to_buf {
        ($x: expr) => {
            BytesMut::from_iter($x.iter()).freeze()
        };
    }

    macro_rules! varint_test {
        ($m: ident, $r: expr, $b: expr) => {
            assert_eq!($m($b).unwrap(), (to_buf!([]), $r));
        };
    }

    #[test]
    fn varint_test() {
        varint_test!(varint, 0, to_buf!([0x00]));
        varint_test!(varint, 1, to_buf!([0x01]));
        varint_test!(varint, 2, to_buf!([0x02]));
        varint_test!(varint, 127, to_buf!([0x7f]));
        varint_test!(varint, 128, to_buf!([0x80, 0x01]));
        varint_test!(varint, 255, to_buf!([0xff, 0x01]));
        varint_test!(varint, 2147483647, to_buf!([0xff, 0xff, 0xff, 0xff, 0x07]));
        varint_test!(varint, -1, to_buf!([0xff, 0xff, 0xff, 0xff, 0x0f]));
        varint_test!(varint, -2147483648, to_buf!([0x80, 0x80, 0x80, 0x80, 0x08]));
    }

    #[test]
    fn varlong_test() {
        varint_test!(varlong, 0, to_buf!([0x00]));
        varint_test!(varlong, 1, to_buf!([0x01]));
        varint_test!(varlong, 2, to_buf!([0x02]));
        varint_test!(varlong, 127, to_buf!([0x7f]));
        varint_test!(varlong, 128, to_buf!([0x80, 0x01]));
        varint_test!(varlong, 255, to_buf!([0xff, 0x01]));
        varint_test!(varlong, 2147483647, to_buf!([0xff, 0xff, 0xff, 0xff, 0x07]));
        varint_test!(
            varlong,
            9223372036854775807,
            to_buf!([0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f])
        );
        varint_test!(
            varlong,
            -1,
            to_buf!([0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01])
        );
        varint_test!(
            varlong,
            -2147483648,
            to_buf!([0x80, 0x80, 0x80, 0x80, 0xf8, 0xff, 0xff, 0xff, 0xff, 0x01])
        );
        varint_test!(
            varlong,
            -9223372036854775808,
            to_buf!([0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x01])
        );
    }

    #[test]
    fn varint_blowout() {
        assert_eq!(
            varint(to_buf!([0x80, 0x80, 0x80, 0x80, 0x80])).unwrap_err(),
            nom::Err::Error(VarintParseFail::VarintExceededShift(32))
        );
    }

    #[test]
    fn varlong_blowout() {
        assert_eq!(
            varlong(to_buf!([
                0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80
            ]))
            .unwrap_err(),
            nom::Err::Error(VarintParseFail::VarintExceededShift(64))
        );
    }

    #[test]
    fn varint_short() {
        assert_eq!(
            varint(to_buf!([0x80, 0x80])).unwrap_err(),
            nom::Err::Incomplete(Needed::Unknown)
        );
        assert_eq!(
            varlong(to_buf!([0x80, 0x80])).unwrap_err(),
            nom::Err::Incomplete(Needed::Unknown)
        );
    }

    #[test]
    fn varint_non_term() {
        assert_eq!(varint(to_buf!([0x01, 0x02])).unwrap(), (to_buf!([0x02]), 1));
        assert_eq!(
            varlong(to_buf!([0x01, 0x02])).unwrap(),
            (to_buf!([0x02]), 1)
        );
    }
}
