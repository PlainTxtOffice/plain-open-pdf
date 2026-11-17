//! Flate encoding: a zlib stream (RFC 1950) of one DEFLATE block with the
//! fixed Huffman codes (RFC 1951), for the streams this crate writes.

use super::flate::{DISTANCE_BASE, DISTANCE_EXTRA, LENGTH_BASE, LENGTH_EXTRA};

/// How far back a match may reach, and how long it may be.
const WINDOW: usize = 32_768;
const MAX_MATCH: usize = 258;
const MIN_MATCH: usize = 3;
/// Earlier positions tried for each match; more finds longer matches.
const MAX_CHAIN: usize = 48;
const HASH_SIZE: usize = 1 << 15;
/// Marks the end of a chain of earlier positions.
const NONE: usize = usize::MAX;

/// Bits are packed from the low end of each byte.
struct BitWriter {
    bytes: Vec<u8>,
    pending: u64,
    count: u32,
}

impl BitWriter {
    fn bits(&mut self, value: u32, count: u32) {
        self.pending |= u64::from(value) << self.count;
        self.count += count;
        while self.count >= 8 {
            self.bytes.push(self.pending as u8);
            self.pending >>= 8;
            self.count -= 8;
        }
    }

    /// A Huffman code, which is sent from its high bit down.
    fn code(&mut self, code: u32, length: u32) {
        self.bits(code.reverse_bits() >> (32 - length), length);
    }

    /// A literal byte, a length symbol, or the end of the block, in the
    /// fixed code of RFC 1951 section 3.2.6.
    fn symbol(&mut self, symbol: u32) {
        match symbol {
            0..=143 => self.code(0x30 + symbol, 8),
            144..=255 => self.code(0x190 + symbol - 144, 9),
            256..=279 => self.code(symbol - 256, 7),
            _ => self.code(0xC0 + symbol - 280, 8),
        }
    }

    /// A copy of `length` bytes from `distance` bytes back.
    fn copy(&mut self, length: usize, distance: usize) {
        let index = LENGTH_BASE
            .iter()
            .rposition(|&base| usize::from(base) <= length)
            .unwrap_or(0);
        self.symbol(257 + index as u32);
        let extra = length - usize::from(LENGTH_BASE[index]);
        self.bits(extra as u32, LENGTH_EXTRA[index]);
        let index = DISTANCE_BASE
            .iter()
            .rposition(|&base| usize::from(base) <= distance)
            .unwrap_or(0);
        self.code(index as u32, 5);
        let extra = distance - usize::from(DISTANCE_BASE[index]);
        self.bits(extra as u32, DISTANCE_EXTRA[index]);
    }
}

fn hash(data: &[u8], position: usize) -> usize {
    let value = u32::from(data[position]) << 16
        | u32::from(data[position + 1]) << 8
        | u32::from(data[position + 2]);
    (value.wrapping_mul(0x9E37_79B1) >> 17) as usize % HASH_SIZE
}

/// Compress data into a zlib stream that `FlateDecode` reverses.
pub(crate) fn deflate_zlib(data: &[u8]) -> Vec<u8> {
    let mut writer = BitWriter {
        bytes: vec![0x78, 0x9C],
        pending: 0,
        count: 0,
    };
    // One final block with the fixed codes.
    writer.bits(1, 1);
    writer.bits(1, 2);
    // The latest position with each hash, and for each position the one
    // before it with the same hash.
    let mut heads = vec![NONE; HASH_SIZE];
    let mut earlier = vec![NONE; data.len()];
    let mut position = 0;
    while position < data.len() {
        let mut best = (0, 0);
        if position + MIN_MATCH <= data.len() {
            let limit = (data.len() - position).min(MAX_MATCH);
            let mut candidate = heads[hash(data, position)];
            for _ in 0..MAX_CHAIN {
                if candidate == NONE || position - candidate > WINDOW {
                    break;
                }
                let length = (0..limit)
                    .take_while(|&offset| data[candidate + offset] == data[position + offset])
                    .count();
                if length > best.0 {
                    best = (length, position - candidate);
                    if length == limit {
                        break;
                    }
                }
                candidate = earlier[candidate];
            }
        }
        let step = if best.0 >= MIN_MATCH {
            writer.copy(best.0, best.1);
            best.0
        } else {
            writer.symbol(u32::from(data[position]));
            1
        };
        let hashed = data.len().saturating_sub(MIN_MATCH - 1);
        let covered = earlier.iter_mut().enumerate().take(hashed);
        for (covered, earlier) in covered.skip(position).take(step) {
            let slot = hash(data, covered);
            *earlier = heads[slot];
            heads[slot] = covered;
        }
        position += step;
    }
    writer.symbol(256);
    if writer.count > 0 {
        writer.bits(0, 8 - writer.count);
    }
    let (mut low, mut high) = (1_u32, 0_u32);
    for &byte in data {
        low = (low + u32::from(byte)) % 65_521;
        high = (high + low) % 65_521;
    }
    writer.bytes.extend((high << 16 | low).to_be_bytes());
    writer.bytes
}

#[cfg(test)]
mod tests {
    use super::super::inflate_zlib;
    use super::*;

    #[test]
    fn compressed_data_inflates_to_the_original() {
        let mut samples: Vec<Vec<u8>> = vec![
            Vec::new(),
            b"a".to_vec(),
            b"abcabcabcabcabcabcabcabc".to_vec(),
            (0..=255).collect(),
            vec![0; 100_000],
            b"BT /F1 12 Tf 72 700 Td (Hello) Tj ET\n".repeat(500),
        ];
        // Bytes with no pattern, and long matches from far back.
        let mut state = 12_345_u32;
        let noise: Vec<u8> = (0..70_000)
            .map(|_| {
                state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                (state >> 16) as u8
            })
            .collect();
        samples.push([noise.as_slice(), &noise[..40_000]].concat());
        samples.push(noise);
        for sample in samples {
            let packed = deflate_zlib(&sample);
            assert_eq!(inflate_zlib(&packed, usize::MAX).unwrap(), sample);
        }
    }

    #[test]
    fn repeated_data_becomes_much_smaller() {
        let text = b"0 0 612 792 re f\n".repeat(1000);
        assert!(deflate_zlib(&text).len() * 20 < text.len());
        assert_eq!(deflate_zlib(b""), [0x78, 0x9C, 0x03, 0x00, 0, 0, 0, 1]);
    }
}
