//! SHA-256, SHA-384, and SHA-512 (FIPS 180-4).

use super::pad_message;
use super::tables::{SHA256_INITIAL, SHA256_ROUNDS, SHA384_INITIAL, SHA512_INITIAL, SHA512_ROUNDS};

pub(crate) fn sha256(data: &[u8]) -> [u8; 32] {
    let mut state = SHA256_INITIAL;
    for block in pad_message(data, 64, 8, true).as_chunks::<64>().0 {
        let mut words = [0u32; 64];
        for (word, bytes) in words.iter_mut().zip(block.as_chunks::<4>().0) {
            *word = u32::from_be_bytes(*bytes);
        }
        for i in 16..64 {
            let x = words[i - 15];
            let y = words[i - 2];
            let small0 = x.rotate_right(7) ^ x.rotate_right(18) ^ (x >> 3);
            let small1 = y.rotate_right(17) ^ y.rotate_right(19) ^ (y >> 10);
            words[i] = words[i - 16]
                .wrapping_add(small0)
                .wrapping_add(words[i - 7])
                .wrapping_add(small1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for i in 0..64 {
            let big1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choose = (e & f) ^ (!e & g);
            let first = h
                .wrapping_add(big1)
                .wrapping_add(choose)
                .wrapping_add(SHA256_ROUNDS[i])
                .wrapping_add(words[i]);
            let big0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let second = big0.wrapping_add(majority);
            (a, b, c, d, e, f, g, h) = (
                first.wrapping_add(second),
                a,
                b,
                c,
                d.wrapping_add(first),
                e,
                f,
                g,
            );
        }
        for (total, value) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *total = total.wrapping_add(value);
        }
    }
    let mut digest = [0u8; 32];
    for (bytes, word) in digest.as_chunks_mut::<4>().0.iter_mut().zip(state) {
        *bytes = word.to_be_bytes();
    }
    digest
}

pub(crate) fn sha512(data: &[u8]) -> [u8; 64] {
    sha512_from(SHA512_INITIAL, data)
}

/// SHA-384 is SHA-512 from a different initial state, cut to 48 bytes.
pub(crate) fn sha384(data: &[u8]) -> [u8; 48] {
    let digest = sha512_from(SHA384_INITIAL, data);
    std::array::from_fn(|index| digest[index])
}

fn sha512_from(mut state: [u64; 8], data: &[u8]) -> [u8; 64] {
    for block in pad_message(data, 128, 16, true).as_chunks::<128>().0 {
        let mut words = [0u64; 80];
        for (word, bytes) in words.iter_mut().zip(block.as_chunks::<8>().0) {
            *word = u64::from_be_bytes(*bytes);
        }
        for i in 16..80 {
            let x = words[i - 15];
            let y = words[i - 2];
            let small0 = x.rotate_right(1) ^ x.rotate_right(8) ^ (x >> 7);
            let small1 = y.rotate_right(19) ^ y.rotate_right(61) ^ (y >> 6);
            words[i] = words[i - 16]
                .wrapping_add(small0)
                .wrapping_add(words[i - 7])
                .wrapping_add(small1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for i in 0..80 {
            let big1 = e.rotate_right(14) ^ e.rotate_right(18) ^ e.rotate_right(41);
            let choose = (e & f) ^ (!e & g);
            let first = h
                .wrapping_add(big1)
                .wrapping_add(choose)
                .wrapping_add(SHA512_ROUNDS[i])
                .wrapping_add(words[i]);
            let big0 = a.rotate_right(28) ^ a.rotate_right(34) ^ a.rotate_right(39);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let second = big0.wrapping_add(majority);
            (a, b, c, d, e, f, g, h) = (
                first.wrapping_add(second),
                a,
                b,
                c,
                d.wrapping_add(first),
                e,
                f,
                g,
            );
        }
        for (total, value) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *total = total.wrapping_add(value);
        }
    }
    let mut digest = [0u8; 64];
    for (bytes, word) in digest.as_chunks_mut::<8>().0.iter_mut().zip(state) {
        *bytes = word.to_be_bytes();
    }
    digest
}
