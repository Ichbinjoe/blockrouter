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

extern crate libc;
use libc::*;

use std::pin::Pin;
use std::mem::MaybeUninit;

#[repr(C)]
pub struct MbedAesContext{
    nr: c_int,
    rk: *mut c_uint,
    buf: [c_uint; 68],
}

#[repr(i32)]
#[derive(Clone, Copy)]
pub enum CryptMode {
    Encrypt = 1,
    Decrypt = 0,
}

#[link(name = "mbedcrypto", kind="static")]
extern "C" {
    fn mbedtls_aes_init(ctx: *const MbedAesContext);
    fn mbedtls_aes_free(ctx: *const MbedAesContext);
    fn mbedtls_aes_setkey_enc(
        ctx: *const MbedAesContext,
        key: *const c_uchar,
        key_bits: c_uint,
    ) -> c_int;
    fn mbedtls_aes_crypt_cfb8(
        ctx: *const MbedAesContext,
        mode: c_int,
        length: size_t,
        iv: *const c_uchar, /*unsized char iv[16]*/
        input: *const c_uchar,
        output: *const c_uchar,
    ) -> c_int;
}

pub struct AesCryptCfb8 {
    ctx: MbedAesContext,
    iv: [c_uchar; 16],
    _pin: std::marker::PhantomPinned,
}

impl AesCryptCfb8 {
    /// Creates a new AesCryptCfb8 with the given key
    pub fn new(key: [c_uchar; 16]) -> Pin<Box<AesCryptCfb8>> {
        // SAFETY: idk looks safe to me
        unsafe {
            let b = Box::pin(
                AesCryptCfb8{
                    ctx: MaybeUninit::zeroed().assume_init(),
                    iv: key,
                    _pin: std::marker::PhantomPinned{},
                }
            );

            // Current implementation simply zeros this pointer - its already zeroed, so we don't
            // have to worry about it
            assert!(mbedtls_aes_setkey_enc(&b.ctx, b.iv.as_ptr(), 16 * 8) == 0);
            return b
        }
    }

    /// Performs an inplace encryption / decryption of the data given depending on the mode passed
    pub fn process(&self, data: &mut [u8], mode: CryptMode) {
        if data.len() == 0 {
            return;
        }
        
        unsafe {
            assert!(mbedtls_aes_crypt_cfb8(
                &self.ctx,
                mode as c_int,
                data.len(),
                self.iv.as_ptr(),
                data.as_ptr(),
                data.as_mut_ptr(),
            ) == 0);
        }
    }
}

impl Drop for AesCryptCfb8 {
    fn drop(&mut self) {
        unsafe {
            mbedtls_aes_free(&mut self.ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bindgen_test_layout_MbedAesContext() {
        assert_eq!(
            ::std::mem::size_of::<MbedAesContext>(),
            288usize,
            concat!("Size of: ", stringify!(MbedAesContext))
        );
        assert_eq!(
            ::std::mem::align_of::<MbedAesContext>(),
            8usize,
            concat!("Alignment of ", stringify!(MbedAesContext))
        );
        assert_eq!(
            unsafe { &(*(::std::ptr::null::<MbedAesContext>())).nr as *const _ as usize },
            0usize,
            concat!(
                "Offset of field: ",
                stringify!(MbedAesContext),
                "::",
                stringify!(nr)
            )
        );
        assert_eq!(
            unsafe { &(*(::std::ptr::null::<MbedAesContext>())).rk as *const _ as usize },
            8usize,
            concat!(
                "Offset of field: ",
                stringify!(MbedAesContext),
                "::",
                stringify!(rk)
            )
        );
        assert_eq!(
            unsafe { &(*(::std::ptr::null::<MbedAesContext>())).buf as *const _ as usize },
            16usize,
            concat!(
                "Offset of field: ",
                stringify!(MbedAesContext),
                "::",
                stringify!(buf)
            )
        );
    }
}
