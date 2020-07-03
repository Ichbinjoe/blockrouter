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

#![feature(test)]
#![feature(new_uninit)]

extern crate bytes;
extern crate nom;
extern crate tokio;

#[macro_use]
pub mod mempool;

pub mod compress;
pub mod crypto;
pub mod cursor;
pub mod framer;
pub mod mbedtls;
pub mod packet;
pub mod parser;
pub mod socket;
pub mod zlib;

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}
