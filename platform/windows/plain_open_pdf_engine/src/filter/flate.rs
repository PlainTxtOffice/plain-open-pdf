//! zlib (RFC 1950) and DEFLATE (RFC 1951) decompression for `FlateDecode`.

use super::FilterError;

pub(super) const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
pub(super) const LENGTH_EXTRA: [u32; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
pub(super) const DISTANCE_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
pub(super) const DISTANCE_EXTRA: [u32; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
/// Order in which a dynamic block stores its code-length code lengths.
const CODE_LENGTH_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

/// Reads bits least-significant first, as DEFLATE packs them.
struct BitReader<'a> {
    data: &'a [u8],
    position: usize,
    buffer: u32,
    count: u32,
}

impl BitReader<'_> {
    /// Read up to 16 bits.
    fn bits(&mut self, need: u32) -> Result<u32, FilterError> {
        while self.count < need {
            let byte = *self.data.get(self.position).ok_or(FilterError::Corrupt)?;
            self.position += 1;
            self.buffer |= u32::from(byte) << self.count;
            self.count += 8;
        }
        let value = self.buffer & ((1 << need) - 1);
        self.buffer >>= need;
        self.count -= need;
        Ok(value)
    }

    /// Drop the unread bits of the current byte; fewer than eight are held.
    fn align(&mut self) {
        self.buffer = 0;
        self.count = 0;
    }
}

/// A canonical Huffman code: how many codes have each length, and the
/// symbols ordered by code.
struct Huffman {
    counts: [u16; 16],
    symbols: Vec<u16>,
}

impl Huffman {
    /// Build a code from one length (0 through 15) per symbol; 0 is unused.
    fn new(lengths: &[u8]) -> Result<Self, FilterError> {
        let mut counts = [0u16; 16];
        for &length in lengths {
            counts[usize::from(length)] += 1;
        }
        counts[0] = 0;
        let mut unused_codes = 1i32;
        for &count in &counts[1..] {
            unused_codes = unused_codes * 2 - i32::from(count);
            if unused_codes < 0 {
                return Err(FilterError::Corrupt);
            }
        }
        let mut offsets = [0u16; 16];
        for length in 1..15 {
            offsets[length + 1] = offsets[length] + counts[length];
        }
        let used = lengths.iter().filter(|&&length| length != 0).count();
        let mut symbols = vec![0u16; used];
        for (symbol, &length) in lengths.iter().enumerate() {
            if length != 0 {
                let offset = &mut offsets[usize::from(length)];
                symbols[usize::from(*offset)] = symbol as u16;
                *offset += 1;
            }
        }
        Ok(Self { counts, symbols })
    }

    fn decode(&self, bits: &mut BitReader<'_>) -> Result<u16, FilterError> {
        let mut code = 0i32;
        let mut first = 0i32;
        let mut index = 0i32;
        for &count in &self.counts[1..] {
            code |= bits.bits(1)? as i32;
            let count = i32::from(count);
            if code - count < first {
                return self
                    .symbols
                    .get((index + code - first) as usize)
                    .copied()
                    .ok_or(FilterError::Corrupt);
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        Err(FilterError::Corrupt)
    }
}

/// Decompress a zlib stream into at most `max_output` bytes.
///
/// The trailing Adler-32 checksum is not verified, because damaged checksums
/// are common in files whose data is otherwise intact.
pub(crate) fn inflate_zlib(data: &[u8], max_output: usize) -> Result<Vec<u8>, FilterError> {
    let [method, flags, body @ ..] = data else {
        return Err(FilterError::Corrupt);
    };
    if method & 0x0f != 8 {
        return Err(FilterError::Corrupt);
    }
    if flags & 0x20 != 0 {
        // A preset dictionary is not something a PDF can supply.
        return Err(FilterError::Corrupt);
    }
    let mut bits = BitReader {
        data: body,
        position: 0,
        buffer: 0,
        count: 0,
    };
    let mut output = Vec::new();
    loop {
        let last = bits.bits(1)? == 1;
        match bits.bits(2)? {
            0 => stored_block(&mut bits, &mut output, max_output)?,
            1 => {
                let mut lengths = [8u8; 288];
                lengths[144..256].fill(9);
                lengths[256..280].fill(7);
                let literals = Huffman::new(&lengths)?;
                let distances = Huffman::new(&[5; 30])?;
                compressed_block(&mut bits, &mut output, max_output, &literals, &distances)?;
            }
            2 => {
                let (literals, distances) = dynamic_codes(&mut bits)?;
                compressed_block(&mut bits, &mut output, max_output, &literals, &distances)?;
            }
            _ => return Err(FilterError::Corrupt),
        }
        if last {
            return Ok(output);
        }
    }
}

fn stored_block(
    bits: &mut BitReader<'_>,
    output: &mut Vec<u8>,
    max_output: usize,
) -> Result<(), FilterError> {
    bits.align();
    let header = bits
        .data
        .get(bits.position..bits.position + 4)
        .ok_or(FilterError::Corrupt)?;
    let length = u16::from_le_bytes([header[0], header[1]]);
    if length != !u16::from_le_bytes([header[2], header[3]]) {
        return Err(FilterError::Corrupt);
    }
    let start = bits.position + 4;
    let end = start + usize::from(length);
    let bytes = bits.data.get(start..end).ok_or(FilterError::Corrupt)?;
    if output.len() + bytes.len() > max_output {
        return Err(FilterError::LimitExceeded);
    }
    output.extend_from_slice(bytes);
    bits.position = end;
    Ok(())
}

/// Read the literal/length and distance codes that open a dynamic block.
fn dynamic_codes(bits: &mut BitReader<'_>) -> Result<(Huffman, Huffman), FilterError> {
    let literal_count = bits.bits(5)? as usize + 257;
    let distance_count = bits.bits(5)? as usize + 1;
    let code_length_count = bits.bits(4)? as usize + 4;
    if literal_count > 286 || distance_count > 30 {
        return Err(FilterError::Corrupt);
    }
    let mut code_lengths = [0u8; 19];
    for &position in CODE_LENGTH_ORDER.iter().take(code_length_count) {
        code_lengths[position] = bits.bits(3)? as u8;
    }
    let code_length_code = Huffman::new(&code_lengths)?;

    let total = literal_count + distance_count;
    let mut lengths = [0u8; 286 + 30];
    let mut index = 0;
    while index < total {
        let symbol = code_length_code.decode(bits)?;
        let (value, repeat) = match symbol {
            0..=15 => (symbol as u8, 1),
            16 => {
                let previous = *lengths
                    .get(index.wrapping_sub(1))
                    .ok_or(FilterError::Corrupt)?;
                (previous, 3 + bits.bits(2)? as usize)
            }
            17 => (0, 3 + bits.bits(3)? as usize),
            _ => (0, 11 + bits.bits(7)? as usize),
        };
        if index + repeat > total {
            return Err(FilterError::Corrupt);
        }
        lengths[index..index + repeat].fill(value);
        index += repeat;
    }
    if lengths[256] == 0 {
        // Without an end-of-block code the block could never finish.
        return Err(FilterError::Corrupt);
    }
    Ok((
        Huffman::new(&lengths[..literal_count])?,
        Huffman::new(&lengths[literal_count..total])?,
    ))
}

fn compressed_block(
    bits: &mut BitReader<'_>,
    output: &mut Vec<u8>,
    max_output: usize,
    literals: &Huffman,
    distances: &Huffman,
) -> Result<(), FilterError> {
    loop {
        let symbol = usize::from(literals.decode(bits)?);
        match symbol {
            0..=255 => {
                if output.len() >= max_output {
                    return Err(FilterError::LimitExceeded);
                }
                output.push(symbol as u8);
            }
            256 => return Ok(()),
            _ => {
                let index = symbol - 257;
                let base = *LENGTH_BASE.get(index).ok_or(FilterError::Corrupt)?;
                let length = usize::from(base) + bits.bits(LENGTH_EXTRA[index])? as usize;
                let index = usize::from(distances.decode(bits)?);
                let base = *DISTANCE_BASE.get(index).ok_or(FilterError::Corrupt)?;
                let distance = usize::from(base) + bits.bits(DISTANCE_EXTRA[index])? as usize;
                if distance > output.len() {
                    return Err(FilterError::Corrupt);
                }
                if output.len() + length > max_output {
                    return Err(FilterError::LimitExceeded);
                }
                for _ in 0..length {
                    output.push(output[output.len() - distance]);
                }
            }
        }
    }
}
