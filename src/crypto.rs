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

use std::pin::Pin;

use crate::mbedtls::{AesCryptCfb8, CryptMode};

pub struct Cryptor {
    c: Option<Pin<Box<AesCryptCfb8>>>,
    mode: CryptMode,
}

impl Cryptor {
    pub fn new_encrypt() -> Cryptor {
        Cryptor{
            c: None,
            mode: CryptMode::Encrypt,
        }
    }
    
    pub fn new_decrypt() -> Cryptor {
        Cryptor{
            c: None,
            mode: CryptMode::Decrypt,
        }
    }

    pub fn process(&mut self, data: &mut [u8]) {
        if let Some(c) = &mut self.c {
            c.process(data, self.mode);
        }
    }

    pub fn start_crypto(&mut self, key: [u8; 16]) {
        self.c = Some(AesCryptCfb8::new(key));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypt() {
        let key: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
        let mut msg: [u8; 7] = [0, 1, 2, 3, 4, 5, 6];

        let mut c = Cryptor::new_encrypt();
        c.start_crypto(key);
        c.process(&mut msg);

        assert_eq!(msg, [0x0a, 0x22, 0xf7, 0x96, 0xe1, 0xb9, 0x3e]);
    }
    
    #[test]
    fn decrypt() {
        let key: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
        let mut msg: [u8; 7] = [0x0a, 0x22, 0xf7, 0x96, 0xe1, 0xb9, 0x3e];

        let mut c = Cryptor::new_decrypt();
        c.start_crypto(key);
        c.process(&mut msg);

        assert_eq!(msg, [0, 1, 2, 3, 4, 5, 6]);
    }
    
    #[test]
    fn passthrough() {
        let mut msg: [u8; 7] = [0, 1, 2, 3, 4, 5, 6];

        let mut c = Cryptor::new_decrypt();
        c.process(&mut msg);

        assert_eq!(msg, [0, 1, 2, 3, 4, 5, 6]);
    }
}
