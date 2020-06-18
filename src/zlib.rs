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
#![feature(new_uninit)]

extern crate libc;
use libc::*;

use std::mem::{size_of, MaybeUninit};

static ZLIB_MAJ_VERSION: &str = "1";

#[repr(C)]
pub struct ZStream {
    pub next_in: *const c_uchar,
    pub avail_in: c_uint,
    pub total_in: size_t,

    pub next_out: *mut c_uchar,
    pub avail_out: c_uint,
    pub total_out: size_t,

    pub msg: *const c_char,
    internal_state: *mut c_void,

    // We do janky stuff here because we want to just zero initialize this struct, and rust gets
    // mad if we do this with function pointers.
    alloc_fn: *const c_void, //extern fn(*mut c_void, c_uint, c_uint) -> *mut c_void,
    free_fn: *const c_void,  //extern fn(*mut c_void, *mut c_void),

    opaque: *mut c_void,

    data_type: c_int,

    pub adler: c_ulong,
    reserved: c_ulong,
}

#[link(name = "z", kind = "static")]
extern "C" {
    fn deflateInit_(
        strm: *mut ZStream,
        level: c_int,
        version: *const c_char,
        stream_size: c_int,
    ) -> c_int;
    fn inflateInit_(strm: *mut ZStream, version: *const c_char, stream_size: c_int) -> c_int;

    fn deflate(strm: *mut ZStream, flush: c_int) -> c_int;
    fn deflateEnd(strm: *mut ZStream) -> c_int;
    fn inflate(strm: *mut ZStream, flush: c_int) -> c_int;
    fn inflateEnd(strm: *mut ZStream) -> c_int;

    fn deflateReset(strm: *mut ZStream);
    fn inflateReset(sterm: *mut ZStream);
}

#[repr(i32)]
#[derive(Debug)]
pub enum ZLibError {
    Errno = -1,
    StreamError = -2,
    DataError = -3,
    MemError = -4,
    BufError = -5,
    VersionError = -6,
}
impl ZLibError {
    fn lookup(i: i32) -> Option<ZLibError> {
        match i {
            -1 => Some(ZLibError::Errno),
            -2 => Some(ZLibError::StreamError),
            -3 => Some(ZLibError::DataError),
            -4 => Some(ZLibError::MemError),
            -5 => Some(ZLibError::BufError),
            -6 => Some(ZLibError::VersionError),
            _ => None,
        }
    }
}

#[repr(i32)]
pub enum FlushMode {
    NoFlush = 0,
    PartialFlush = 1,
    SyncFlush = 2,
    FullFlush = 3,
    Finish = 4,
    Block = 5,
    Trees = 6,
}

pub trait ZlibOperator {
    fn reset(&mut self);
    fn process(&mut self, flush: FlushMode) -> Option<ZLibError>;
    fn strm(&self) -> &ZStream;
    fn strm_mut(&mut self) -> &mut ZStream;
}

pub struct Inflate {
    pub strm: Box<ZStream>,
}

impl Drop for Inflate {
    fn drop(&mut self) {
        unsafe {
            inflateEnd(self.strm.as_mut());
        }
    }
}

impl Inflate {
    pub fn new() -> Result<Inflate, ZLibError> {
        let mut i = Inflate {
            strm: unsafe { Box::<ZStream>::new_zeroed().assume_init() },
        };

        let errno = unsafe {
            inflateInit_(
                i.strm.as_mut(),
                ZLIB_MAJ_VERSION.as_ptr() as *const i8,
                size_of::<ZStream>() as i32,
            )
        };

        if let Some(e) = ZLibError::lookup(errno) {
            return Err(e);
        }

        Ok(i)
    }
}

impl ZlibOperator for Inflate {
    fn reset(&mut self) {
        unsafe { inflateReset(self.strm.as_mut()) }
    }

    fn process(&mut self, flush: FlushMode) -> Option<ZLibError> {
        ZLibError::lookup(unsafe { inflate(self.strm.as_mut(), flush as i32) })
    }

    fn strm(&self) -> &ZStream {
        &self.strm
    }

    fn strm_mut(&mut self) -> &mut ZStream {
        &mut self.strm
    }
}

pub struct Deflate {
    pub strm: Box<ZStream>,
}

impl Drop for Deflate {
    fn drop(&mut self) {
        unsafe {
            deflateEnd(self.strm.as_mut());
        }
    }
}

impl Deflate {
    pub fn new(level: i32) -> Result<Deflate, ZLibError> {
        let mut i = Deflate {
            strm: unsafe { Box::new_zeroed().assume_init() },
        };

        let errno = unsafe {
            deflateInit_(
                i.strm.as_mut(),
                level,
                ZLIB_MAJ_VERSION.as_ptr() as *const i8,
                size_of::<ZStream>() as i32,
            )
        };

        if let Some(e) = ZLibError::lookup(errno) {
            return Err(e);
        }

        Ok(i)
    }
}

impl ZlibOperator for Deflate {
    fn reset(&mut self) {
        unsafe { deflateReset(self.strm.as_mut()) }
    }

    fn process(&mut self, flush: FlushMode) -> Option<ZLibError> {
        ZLibError::lookup(unsafe { deflate(self.strm.as_mut(), flush as i32) })
    }

    fn strm(&self) -> &ZStream {
        &self.strm
    }

    fn strm_mut(&mut self) -> &mut ZStream {
        &mut self.strm
    }
}
