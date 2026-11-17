//! MD5 (RFC 1321).

use super::pad_message;
use super::tables::MD5_SINES;

/// Left-rotation amounts: four per round, repeated across its 16 steps.
const SHIFTS: [u32; 16] = [7, 12, 17, 22, 5, 9, 14, 20, 4, 11, 16, 23, 6, 10, 15, 21];

pub(crate) fn md5(data: &[u8]) -> [u8; 16] {
    let mut state = [0x6745_2301u32, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476];
    for block in pad_message(data, 64, 8, false).as_chunks::<64>().0 {
        let mut words = [0u32; 16];
        for (word, bytes) in words.iter_mut().zip(block.as_chunks::<4>().0) {
            *word = u32::from_le_bytes(*bytes);
        }
        let [mut a, mut b, mut c, mut d] = state;
        for step in 0..64 {
            let (mixed, word) = match step / 16 {
                0 => ((b & c) | (!b & d), step),
                1 => ((d & b) | (!d & c), (5 * step + 1) % 16),
                2 => (b ^ c ^ d, (3 * step + 5) % 16),
                _ => (c ^ (b | !d), (7 * step) % 16),
            };
            let sum = a
                .wrapping_add(mixed)
                .wrapping_add(MD5_SINES[step])
                .wrapping_add(words[word]);
            let shift = SHIFTS[step / 16 * 4 + step % 4];
            (a, b, c, d) = (d, b.wrapping_add(sum.rotate_left(shift)), b, c);
        }
        for (total, value) in state.iter_mut().zip([a, b, c, d]) {
            *total = total.wrapping_add(value);
        }
    }
    let mut digest = [0u8; 16];
    for (bytes, word) in digest.as_chunks_mut::<4>().0.iter_mut().zip(state) {
        *bytes = word.to_le_bytes();
    }
    digest
}
