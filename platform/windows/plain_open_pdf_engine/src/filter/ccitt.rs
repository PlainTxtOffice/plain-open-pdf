//! `CCITTFaxDecode`: Group 3 and Group 4 fax data (ITU-T T.4 and T.6),
//! decoded to one bit for each pixel (ISO 32000-2 section 7.4.6).

use super::FilterError;
use std::collections::HashMap;

/// The codes of white runs up to 63 pixels (T.4 table 2), by run length.
const WHITE_TERMINATING: [&str; 64] = [
    "00110101", "000111", "0111", "1000", "1011", "1100", "1110", "1111", "10011", "10100",
    "00111", "01000", "001000", "000011", "110100", "110101", "101010", "101011", "0100111",
    "0001100", "0001000", "0010111", "0000011", "0000100", "0101000", "0101011", "0010011",
    "0100100", "0011000", "00000010", "00000011", "00011010", "00011011", "00010010", "00010011",
    "00010100", "00010101", "00010110", "00010111", "00101000", "00101001", "00101010", "00101011",
    "00101100", "00101101", "00000100", "00000101", "00001010", "00001011", "01010010", "01010011",
    "01010100", "01010101", "00100100", "00100101", "01011000", "01011001", "01011010", "01011011",
    "01001010", "01001011", "00110010", "00110011", "00110100",
];

/// The codes of black runs up to 63 pixels (T.4 table 2).
const BLACK_TERMINATING: [&str; 64] = [
    "0000110111",
    "010",
    "11",
    "10",
    "011",
    "0011",
    "0010",
    "00011",
    "000101",
    "000100",
    "0000100",
    "0000101",
    "0000111",
    "00000100",
    "00000111",
    "000011000",
    "0000010111",
    "0000011000",
    "0000001000",
    "00001100111",
    "00001101000",
    "00001101100",
    "00000110111",
    "00000101000",
    "00000010111",
    "00000011000",
    "000011001010",
    "000011001011",
    "000011001100",
    "000011001101",
    "000001101000",
    "000001101001",
    "000001101010",
    "000001101011",
    "000011010010",
    "000011010011",
    "000011010100",
    "000011010101",
    "000011010110",
    "000011010111",
    "000001101100",
    "000001101101",
    "000011011010",
    "000011011011",
    "000001010100",
    "000001010101",
    "000001010110",
    "000001010111",
    "000001100100",
    "000001100101",
    "000001010010",
    "000001010011",
    "000000100100",
    "000000110111",
    "000000111000",
    "000000100111",
    "000000101000",
    "000001011000",
    "000001011001",
    "000000101011",
    "000000101100",
    "000001011010",
    "000001100110",
    "000001100111",
];

/// The codes of white runs of 64 to 1728 pixels, in steps of 64 (T.4
/// table 3).
const WHITE_MAKEUP: [&str; 27] = [
    "11011",
    "10010",
    "010111",
    "0110111",
    "00110110",
    "00110111",
    "01100100",
    "01100101",
    "01101000",
    "01100111",
    "011001100",
    "011001101",
    "011010010",
    "011010011",
    "011010100",
    "011010101",
    "011010110",
    "011010111",
    "011011000",
    "011011001",
    "011011010",
    "011011011",
    "010011000",
    "010011001",
    "010011010",
    "011000",
    "010011011",
];

/// The codes of black runs of 64 to 1728 pixels (T.4 table 3).
const BLACK_MAKEUP: [&str; 27] = [
    "0000001111",
    "000011001000",
    "000011001001",
    "000001011011",
    "000000110011",
    "000000110100",
    "000000110101",
    "0000001101100",
    "0000001101101",
    "0000001001010",
    "0000001001011",
    "0000001001100",
    "0000001001101",
    "0000001110010",
    "0000001110011",
    "0000001110100",
    "0000001110101",
    "0000001110110",
    "0000001110111",
    "0000001010010",
    "0000001010011",
    "0000001010100",
    "0000001010101",
    "0000001011010",
    "0000001011011",
    "0000001100100",
    "0000001100101",
];

/// The codes of runs of 1792 to 2560 pixels of either color (T.4 table 3).
const EXTENDED_MAKEUP: [&str; 13] = [
    "00000001000",
    "00000001100",
    "00000001101",
    "000000010010",
    "000000010011",
    "000000010100",
    "000000010101",
    "000000010110",
    "000000010111",
    "000000011100",
    "000000011101",
    "000000011110",
    "000000011111",
];

/// The two-dimensional coding modes (T.4 table 4).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Pass,
    Horizontal,
    /// Vertical, with the offset of a1 from b1.
    Vertical(i32),
    /// An extension, such as uncompressed mode, which is not supported.
    Extension,
}

const MODES: [(&str, Mode); 9] = [
    ("0001", Mode::Pass),
    ("001", Mode::Horizontal),
    ("1", Mode::Vertical(0)),
    ("011", Mode::Vertical(1)),
    ("000011", Mode::Vertical(2)),
    ("0000011", Mode::Vertical(3)),
    ("010", Mode::Vertical(-1)),
    ("000010", Mode::Vertical(-2)),
    ("0000010", Mode::Vertical(-3)),
];

/// Codes by length and value.
type Codes<T> = HashMap<(u32, u32), T>;

fn code(text: &str) -> (u32, u32) {
    let value = text
        .bytes()
        .fold(0, |value, bit| value << 1 | u32::from(bit - b'0'));
    (text.len() as u32, value)
}

/// The run lengths each code of one color stands for.
fn run_codes(terminating: &[&str; 64], makeup: &[&str; 27]) -> Codes<usize> {
    let mut codes = HashMap::new();
    for (length, text) in terminating.iter().enumerate() {
        codes.insert(code(text), length);
    }
    for (index, text) in makeup.iter().enumerate() {
        codes.insert(code(text), (index + 1) * 64);
    }
    for (index, text) in EXTENDED_MAKEUP.iter().enumerate() {
        codes.insert(code(text), 1792 + index * 64);
    }
    codes
}

/// How a fax stream is coded, from its decode parameters.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FaxParameters {
    /// Below 0, all two-dimensional (Group 4); 0, all one-dimensional;
    /// above 0, mixed, with a tag bit before each line.
    pub k: i64,
    pub byte_align: bool,
    pub columns: usize,
    /// The number of rows, or 0 when the data ends them.
    pub rows: usize,
    pub end_of_block: bool,
    pub black_is_1: bool,
}

impl Default for FaxParameters {
    fn default() -> Self {
        Self {
            k: 0,
            byte_align: false,
            columns: 1728,
            rows: 0,
            end_of_block: true,
            black_is_1: false,
        }
    }
}

struct BitReader<'a> {
    data: &'a [u8],
    position: usize,
}

impl BitReader<'_> {
    fn ended(&self) -> bool {
        self.position >= self.data.len() * 8
    }

    /// The next `count` bits, as a number; bits past the end are zeros.
    fn peek(&self, count: u32) -> u32 {
        let mut value = 0;
        for offset in 0..count as usize {
            let at = self.position + offset;
            let bit = self
                .data
                .get(at / 8)
                .map_or(0, |byte| byte >> (7 - at % 8) & 1);
            value = value << 1 | u32::from(bit);
        }
        value
    }

    fn skip(&mut self, count: u32) {
        self.position += count as usize;
    }

    fn align(&mut self) {
        self.position = self.position.div_ceil(8) * 8;
    }

    /// Read the code a table knows, trying lengths from 1 to 13 bits.
    fn read<T: Copy>(&mut self, codes: &Codes<T>) -> Option<T> {
        for length in 1..=13 {
            if let Some(&value) = codes.get(&(length, self.peek(length))) {
                self.skip(length);
                return Some(value);
            }
        }
        None
    }

    /// Move past an end-of-line code, with any zero fill before it, and
    /// return whether there was one.
    fn end_of_line(&mut self) -> bool {
        let mut zeros = 0;
        while zeros < 4096 && self.peek(1) == 0 && !self.ended() {
            zeros += 1;
            self.position += 1;
        }
        if zeros >= 11 && self.peek(1) == 1 {
            self.skip(1);
            return true;
        }
        self.position -= zeros;
        false
    }
}

/// Decode fax data to rows of one bit for each pixel, padded to whole
/// bytes, with 0 for black unless `black_is_1`.
pub(crate) fn ccitt_decode(
    data: &[u8],
    parameters: FaxParameters,
    max_output: usize,
) -> Result<Vec<u8>, FilterError> {
    let columns = parameters.columns;
    if columns == 0 || columns > 1 << 20 {
        return Err(FilterError::Corrupt);
    }
    let row_bytes = columns.div_ceil(8);
    let white = run_codes(&WHITE_TERMINATING, &WHITE_MAKEUP);
    let black = run_codes(&BLACK_TERMINATING, &BLACK_MAKEUP);
    let mut modes: Codes<Mode> = MODES
        .iter()
        .map(|&(text, mode)| (code(text), mode))
        .collect();
    modes.insert(code("0000001"), Mode::Extension);
    let mut bits = BitReader { data, position: 0 };
    let mut output = Vec::new();
    // Each line as the positions where its color changes, starting white.
    let mut reference: Vec<usize> = Vec::new();
    let mut rows = 0;
    while parameters.rows == 0 || rows < parameters.rows {
        if parameters.byte_align {
            bits.align();
        }
        // End-of-line codes may come before a line; several in a row end
        // the data.
        let mut ends = 0;
        while bits.end_of_line() {
            ends += 1;
        }
        if bits.ended() || (parameters.end_of_block && ends >= 2) {
            break;
        }
        let two_dimensional = match parameters.k {
            k if k < 0 => true,
            0 => false,
            _ => {
                let tag = bits.peek(1);
                bits.skip(1);
                tag == 0
            }
        };
        let line = if two_dimensional {
            two_dimensional_line(&mut bits, &reference, columns, &white, &black, &modes)
        } else {
            one_dimensional_line(&mut bits, columns, &white, &black)
        };
        let Some(line) = line.map(|line| normalize(line, columns)) else {
            break;
        };
        if output.len() + row_bytes > max_output {
            return Err(FilterError::LimitExceeded);
        }
        output.extend(pack(&line, columns, parameters.black_is_1));
        reference = line;
        rows += 1;
    }
    // Rows the data does not reach are white.
    let blank = pack(&[], columns, parameters.black_is_1);
    while rows < parameters.rows {
        if output.len() + row_bytes > max_output {
            return Err(FilterError::LimitExceeded);
        }
        output.extend(&blank);
        rows += 1;
    }
    Ok(output)
}

/// A run length: makeup codes, then a terminating code under 64.
fn run(bits: &mut BitReader<'_>, codes: &Codes<usize>) -> Option<usize> {
    let mut total = 0;
    loop {
        let length = bits.read(codes)?;
        total += length;
        if length < 64 {
            return Some(total);
        }
    }
}

/// One line coded as alternating runs, white first.
fn one_dimensional_line(
    bits: &mut BitReader<'_>,
    columns: usize,
    white: &Codes<usize>,
    black: &Codes<usize>,
) -> Option<Vec<usize>> {
    let mut changes = Vec::new();
    let mut position = 0;
    let mut is_black = false;
    while position < columns {
        position = (position + run(bits, if is_black { black } else { white })?).min(columns);
        changes.push(position);
        is_black = !is_black;
    }
    Some(changes)
}

/// One line coded against the line before it.
fn two_dimensional_line(
    bits: &mut BitReader<'_>,
    reference: &[usize],
    columns: usize,
    white: &Codes<usize>,
    black: &Codes<usize>,
    modes: &Codes<Mode>,
) -> Option<Vec<usize>> {
    let mut changes: Vec<usize> = Vec::new();
    // a0 starts just before the line, on white.
    let mut start: Option<usize> = None;
    let mut is_black = false;
    loop {
        let from = start.unwrap_or(0);
        if start.is_some_and(|start| start >= columns) {
            break;
        }
        // b1: the first change on the reference line past a0 to the color
        // opposite a0's, which is at an even index for a change to black.
        // b2: the change after it.
        let wanted = usize::from(is_black);
        let found = reference.iter().enumerate().position(|(index, &place)| {
            index % 2 == wanted && start.is_none_or(|start| place > start)
        });
        let b1 = found
            .and_then(|index| reference.get(index))
            .copied()
            .unwrap_or(columns);
        let b2 = found
            .and_then(|index| reference.get(index + 1))
            .copied()
            .unwrap_or(columns);
        match bits.read(modes)? {
            Mode::Pass => start = Some(b2),
            Mode::Horizontal => {
                let (first, second) = if is_black {
                    (black, white)
                } else {
                    (white, black)
                };
                let a1 = (from + run(bits, first)?).min(columns);
                let a2 = (a1 + run(bits, second)?).min(columns);
                changes.extend([a1, a2]);
                start = Some(a2);
            }
            Mode::Vertical(offset) => {
                let a1 = usize::try_from(b1 as i64 + i64::from(offset))
                    .ok()?
                    .min(columns);
                if a1 < from && start.is_some() {
                    return None;
                }
                changes.push(a1);
                start = Some(a1);
                is_black = !is_black;
            }
            Mode::Extension => return None,
        }
    }
    Some(changes)
}

/// Keep only the real changes of a line: two at one place cancel, as a run
/// of no pixels leaves them, and changes at the line's end close it.
fn normalize(changes: Vec<usize>, columns: usize) -> Vec<usize> {
    let mut kept: Vec<usize> = Vec::with_capacity(changes.len());
    for change in changes {
        if kept.last() == Some(&change) {
            kept.pop();
        } else {
            kept.push(change);
        }
    }
    kept.retain(|&change| change < columns);
    kept
}

/// Pack a line, given where its color changes, into bytes.
fn pack(changes: &[usize], columns: usize, black_is_1: bool) -> Vec<u8> {
    let mut bytes = vec![0_u8; columns.div_ceil(8)];
    let mut is_black = false;
    let mut position = 0;
    for &change in changes.iter().chain([&columns]) {
        let end = change.min(columns);
        if is_black == black_is_1 {
            // A run whose pixels are one bits.
            for pixel in position..end {
                bytes[pixel / 8] |= 0x80 >> (pixel % 8);
            }
        }
        position = position.max(end);
        is_black = !is_black;
    }
    bytes
}
