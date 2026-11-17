//! The ciphers and hashes PDF encryption is built from.
//!
//! These read files that were encrypted with them, write files again with
//! the protection they had, and give new files AES-256 protection. RC4 and
//! MD5 are broken as security primitives and are only used to keep a file's
//! existing protection. Nothing here is hardened against timing attacks.

mod aes;
mod md5;
mod random;
mod rc4;
mod sha2;
mod tables;
#[cfg(test)]
mod tests;

pub(crate) use aes::{cbc_decrypt, cbc_encrypt};
pub(crate) use md5::md5;
pub(crate) use random::random_bytes;
pub(crate) use rc4::rc4;
pub(crate) use sha2::{sha256, sha384, sha512};

/// Append the padding MD5 and SHA-2 hash: a one bit, zeros, then the message
/// length in bits, so that the result is a whole number of blocks.
fn pad_message(data: &[u8], block: usize, length_bytes: usize, big_endian: bool) -> Vec<u8> {
    let mut message = data.to_vec();
    message.push(0x80);
    while message.len() % block != block - length_bytes {
        message.push(0);
    }
    let bits = (data.len() as u128) * 8;
    if big_endian {
        message.extend(&bits.to_be_bytes()[16 - length_bytes..]);
    } else {
        message.extend(&bits.to_le_bytes()[..length_bytes]);
    }
    message
}
