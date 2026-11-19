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

/// The longest code DEFLATE allows, in bits.
const MAX_CODE_BITS: u32 = 15;
/// How many input bits index the lookup table of a code.
const LOOKUP_BITS: u32 = 10;
const LOOKUP_SIZE: usize = 1 << LOOKUP_BITS;
/// A lookup entry keeps the code's length in this many low bits and the
/// symbol above them.
const ENTRY_LENGTH_BITS: u32 = 4;
/// The most symbols a code has: the fixed literal/length code.
const MAX_SYMBOLS: usize = 288;
/// How many bytes one refill of the bit buffer loads away from the end of
/// the data.
const REFILL_BYTES: usize = 6;

/// Reads bits least-significant first, as DEFLATE packs them.
///
/// `buffer` holds the `count` bits that were loaded and not yet used, the
/// next one lowest; its higher bits are zero. `position` is the next byte to
/// load, so it runs ahead of the bits already used.
struct BitReader<'a> {
    data: &'a [u8],
    position: usize,
    buffer: u64,
    count: u32,
}

impl BitReader<'_> {
    /// Load more bytes into a buffer that holds fewer than 16 bits.
    ///
    /// Away from the end of the data this loads `REFILL_BYTES` bytes at once,
    /// which always fit above 15 bits. In the last bytes it loads one byte at
    /// a time. Afterwards the buffer holds at least 48 bits, or every bit
    /// that is left.
    fn refill(&mut self) {
        debug_assert!(self.count < 16);
        let rest = self.data.get(self.position..).unwrap_or_default();
        if let Some(word) = rest.first_chunk::<8>() {
            let loaded = u64::from_le_bytes(*word) & ((1 << (REFILL_BYTES * 8)) - 1);
            self.buffer |= loaded << self.count;
            self.position += REFILL_BYTES;
            self.count += REFILL_BYTES as u32 * 8;
        } else {
            for &byte in rest {
                if self.count > 56 {
                    break;
                }
                self.buffer |= u64::from(byte) << self.count;
                self.position += 1;
                self.count += 8;
            }
        }
    }

    /// Read up to 16 bits; fail when the data ends before them.
    fn bits(&mut self, need: u32) -> Result<u32, FilterError> {
        if self.count < need {
            self.refill();
            if self.count < need {
                return Err(FilterError::Corrupt);
            }
        }
        let value = (self.buffer & ((1 << need) - 1)) as u32;
        self.drop_bits(need);
        Ok(value)
    }

    /// Mark the lowest `used` bits of the buffer as read.
    fn drop_bits(&mut self, used: u32) {
        self.buffer >>= used;
        self.count -= used;
    }

    /// Skip to the next byte boundary: drop the unread bits of the current
    /// byte and give back the whole bytes that were loaded ahead.
    fn align(&mut self) {
        self.position -= (self.count / 8) as usize;
        self.buffer = 0;
        self.count = 0;
    }
}

/// A canonical Huffman code (RFC 1951, section 3.2.2).
///
/// The codes of one length are consecutive numbers, given to the symbols in
/// rising order, and each length starts where the shorter ones end.
struct Huffman {
    /// Indexed by the next `LOOKUP_BITS` input bits: the symbol and the
    /// length of the code those bits start with, or 0 when that code is
    /// longer than `LOOKUP_BITS` bits or does not exist.
    lookup: [u16; LOOKUP_SIZE],
    /// How many codes have each length.
    counts: [u32; 16],
    /// The lowest code of each length.
    first_code: [u32; 16],
    /// Where the symbols of each length start in `symbols`.
    first_index: [usize; 16],
    /// The symbols in the order of their codes.
    symbols: [u16; MAX_SYMBOLS],
}

impl Huffman {
    /// Build a code from one length (0 through 15) per symbol; 0 is unused.
    ///
    /// Lengths that leave codes unused are accepted. Lengths that need more
    /// codes than there are make the stream corrupt.
    fn new(lengths: &[u8]) -> Result<Self, FilterError> {
        let mut counts = [0u32; 16];
        for &length in lengths {
            counts[usize::from(length)] += 1;
        }
        let mut first_code = [0u32; 16];
        let mut first_index = [0usize; 16];
        let mut code = 0;
        let mut index = 0;
        // Every code of `length` bits stands for 2^(15 - length) of the
        // 2^15 codes of full length.
        let mut full_codes_used = 0;
        for length in 1..16 {
            code <<= 1;
            first_code[length] = code;
            first_index[length] = index;
            code += counts[length];
            index += counts[length] as usize;
            full_codes_used += counts[length] << (MAX_CODE_BITS as usize - length);
        }
        if full_codes_used > 1 << MAX_CODE_BITS {
            return Err(FilterError::Corrupt);
        }

        let mut symbols = [0u16; MAX_SYMBOLS];
        let mut next_index = first_index;
        for (symbol, &length) in lengths.iter().enumerate() {
            if length != 0 {
                let slot = &mut next_index[usize::from(length)];
                symbols[*slot] = symbol as u16;
                *slot += 1;
            }
        }

        let mut lookup = [0u16; LOOKUP_SIZE];
        for length in 1..=LOOKUP_BITS {
            // The bits after a code can be anything, so each code fills one
            // entry for every value of the index bits it leaves over.
            let spare_values = 1usize << (LOOKUP_BITS - length);
            let start = first_index[length as usize];
            let end = start + counts[length as usize] as usize;
            let codes = first_code[length as usize]..;
            for (code, &symbol) in codes.zip(&symbols[start..end]) {
                // A code arrives with its highest bit first, so in the bit
                // buffer it reads backwards.
                let as_read = (code.reverse_bits() >> (32 - length)) as usize;
                let entry = symbol << ENTRY_LENGTH_BITS | length as u16;
                for spare in 0..spare_values {
                    lookup[spare << length | as_read] = entry;
                }
            }
        }
        Ok(Self {
            lookup,
            counts,
            first_code,
            first_index,
            symbols,
        })
    }

    /// Read one code and return its symbol.
    fn decode(&self, bits: &mut BitReader<'_>) -> Result<u16, FilterError> {
        if bits.count < MAX_CODE_BITS {
            bits.refill();
        }
        let entry = self.lookup[bits.buffer as usize & (LOOKUP_SIZE - 1)];
        let length = u32::from(entry) & ((1 << ENTRY_LENGTH_BITS) - 1);
        if length == 0 {
            return self.decode_long(bits);
        }
        // At the end of the data the buffer is padded with zero bits. A code
        // that needs some of them is cut off, not read.
        if length > bits.count {
            return Err(FilterError::Corrupt);
        }
        bits.drop_bits(length);
        Ok(entry >> ENTRY_LENGTH_BITS)
    }

    /// Read a code that the lookup table does not hold.
    ///
    /// No code of up to `LOOKUP_BITS` bits starts the buffer, so this tries
    /// each longer length in turn: the first bits belong to a code of that
    /// length when their number is one of the `counts` codes from
    /// `first_code` on. Lengths the data is too short for are not tried.
    fn decode_long(&self, bits: &mut BitReader<'_>) -> Result<u16, FilterError> {
        // The next 15 bits, with the one read first as the highest.
        let window = u32::from(((bits.buffer & 0x7fff) as u16).reverse_bits() >> 1);
        for length in LOOKUP_BITS + 1..=MAX_CODE_BITS.min(bits.count) {
            let code = window >> (MAX_CODE_BITS - length);
            let place = code.wrapping_sub(self.first_code[length as usize]);
            if place < self.counts[length as usize] {
                bits.drop_bits(length);
                let index = self.first_index[length as usize] + place as usize;
                return self.symbols.get(index).copied().ok_or(FilterError::Corrupt);
            }
        }
        Err(FilterError::Corrupt)
    }
}

/// The literal/length and distance codes of a fixed block (RFC 1951,
/// section 3.2.6).
fn fixed_codes() -> Result<(Huffman, Huffman), FilterError> {
    let mut lengths = [8u8; 288];
    lengths[144..256].fill(9);
    lengths[256..280].fill(7);
    Ok((Huffman::new(&lengths)?, Huffman::new(&[5; 30])?))
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
    // Most streams grow to at least twice their size, so the output starts
    // with that much room and skips the first rounds of growing. The room is
    // bounded by the data, which is already in memory, and by `max_output`.
    let mut output = Vec::with_capacity(body.len().saturating_mul(2).min(max_output));
    // The fixed codes are built once, by the first block that uses them.
    let mut fixed = None;
    loop {
        let last = bits.bits(1)? == 1;
        match bits.bits(2)? {
            0 => stored_block(&mut bits, &mut output, max_output)?,
            1 => {
                let (literals, distances) = match &fixed {
                    Some(codes) => codes,
                    None => fixed.insert(fixed_codes()?),
                };
                compressed_block(&mut bits, &mut output, max_output, literals, distances)?;
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
                // A match may run into the bytes it writes itself. Each
                // round copies from the start of the match up to the end of
                // the output, a whole number of repeats, so the pattern
                // stays whole and the next round can copy twice as much.
                let start = output.len() - distance;
                let mut remaining = length;
                while remaining > 0 {
                    let round = remaining.min(output.len() - start);
                    output.extend_from_within(start..start + round);
                    remaining -= round;
                }
            }
        }
    }
}
