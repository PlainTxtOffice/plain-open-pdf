//! ToUnicode CMaps: from character codes to Unicode text
//! (ISO 32000-2 section 9.10.3).

use crate::{ParseError, PdfObject, parse_content};
use std::collections::BTreeMap;

/// A range of codes that map to consecutive or listed values.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CodeRange {
    length: usize,
    low: u32,
    high: u32,
    target: RangeTarget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RangeTarget {
    /// UTF-16 units for the lowest code; the last unit counts up from there.
    Base(Vec<u16>),
    /// One value for each code of the range, in order.
    Each(Vec<String>),
}

/// The mapping a font's `/ToUnicode` stream defines.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToUnicodeMap {
    /// Code length in bytes, lowest code, and highest code of each
    /// codespace range.
    codespaces: Vec<(usize, u32, u32)>,
    /// Text by code length and code.
    singles: BTreeMap<(usize, u32), String>,
    ranges: Vec<CodeRange>,
}

/// A code's bytes as a big-endian number; codes are at most four bytes.
fn code_value(bytes: &[u8]) -> Option<u32> {
    (1..=4).contains(&bytes.len()).then(|| {
        bytes
            .iter()
            .fold(0, |value, &byte| value << 8 | u32::from(byte))
    })
}

fn utf16_units(bytes: &[u8]) -> Vec<u16> {
    let units = bytes.as_chunks::<2>().0;
    units.iter().map(|unit| u16::from_be_bytes(*unit)).collect()
}

fn text(units: &[u16]) -> String {
    char::decode_utf16(units.iter().copied())
        .map(|unit| unit.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

fn string(object: &PdfObject) -> Option<&[u8]> {
    match object {
        PdfObject::String(bytes) => Some(bytes),
        _ => None,
    }
}

impl ToUnicodeMap {
    /// Read the codespace ranges and the `bfchar` and `bfrange` sections of
    /// a CMap; everything else in the file is ignored, as are entries that
    /// are not well formed.
    ///
    /// # Errors
    ///
    /// Returns a located [`ParseError`] when the CMap's syntax cannot be read.
    pub fn parse(bytes: &[u8]) -> Result<Self, ParseError> {
        let mut map = Self::default();
        for operation in parse_content(bytes)? {
            let operands = &operation.operands;
            match operation.operator.as_slice() {
                b"endcodespacerange" => {
                    for pair in operands.as_chunks::<2>().0 {
                        if let (Some(low), Some(high)) = (string(&pair[0]), string(&pair[1]))
                            && low.len() == high.len()
                            && let (Some(low_code), Some(high)) =
                                (code_value(low), code_value(high))
                        {
                            map.codespaces.push((low.len(), low_code, high));
                        }
                    }
                }
                b"endbfchar" => {
                    for pair in operands.as_chunks::<2>().0 {
                        if let (Some(code), Some(target)) = (string(&pair[0]), string(&pair[1]))
                            && let Some(value) = code_value(code)
                        {
                            let target = text(&utf16_units(target));
                            map.singles.insert((code.len(), value), target);
                        }
                    }
                }
                b"endbfrange" => {
                    for triple in operands.as_chunks::<3>().0 {
                        let (Some(low), Some(high)) = (string(&triple[0]), string(&triple[1]))
                        else {
                            continue;
                        };
                        let target = match &triple[2] {
                            PdfObject::String(base) => RangeTarget::Base(utf16_units(base)),
                            PdfObject::Array(values) => RangeTarget::Each(
                                values
                                    .iter()
                                    .map(|value| {
                                        text(&utf16_units(string(value).unwrap_or_default()))
                                    })
                                    .collect(),
                            ),
                            _ => continue,
                        };
                        if low.len() == high.len()
                            && let (Some(low_code), Some(high)) =
                                (code_value(low), code_value(high))
                        {
                            map.ranges.push(CodeRange {
                                length: low.len(),
                                low: low_code,
                                high,
                                target,
                            });
                        }
                    }
                }
                _ => {}
            }
        }
        map.codespaces.sort_unstable();
        Ok(map)
    }

    /// The text one character code maps to, if the CMap defines it.
    pub fn lookup(&self, code: &[u8]) -> Option<String> {
        let value = code_value(code)?;
        if let Some(text) = self.singles.get(&(code.len(), value)) {
            return Some(text.clone());
        }
        let range = self.ranges.iter().find(|range| {
            range.length == code.len() && (range.low..=range.high).contains(&value)
        })?;
        let offset = value - range.low;
        match &range.target {
            RangeTarget::Each(values) => values.get(offset as usize).cloned(),
            RangeTarget::Base(units) => {
                let mut units = units.clone();
                let last = units.last_mut()?;
                *last = last.wrapping_add(offset as u16);
                Some(text(&units))
            }
        }
    }

    /// Decode a string of character codes shown with this font.
    ///
    /// Codes are split by the codespace ranges, shortest first; without
    /// any, each byte is a code. A code with no mapping, or bytes outside
    /// every codespace, become U+FFFD.
    pub fn decode(&self, bytes: &[u8]) -> String {
        let mut decoded = String::new();
        let mut rest = bytes;
        while !rest.is_empty() {
            let (code, tail) = rest.split_at(self.code_length(rest));
            match self.lookup(code) {
                Some(text) => decoded.push_str(&text),
                None => decoded.push(char::REPLACEMENT_CHARACTER),
            }
            rest = tail;
        }
        decoded
    }

    /// How many bytes the character code at the start of `rest` has: the
    /// length of the shortest codespace range that holds it, or 1.
    pub(crate) fn code_length(&self, rest: &[u8]) -> usize {
        self.codespaces
            .iter()
            .find(|&&(length, low, high)| {
                rest.get(..length)
                    .and_then(code_value)
                    .is_some_and(|value| (low..=high).contains(&value))
            })
            .map_or(1, |codespace| codespace.0)
    }

    /// The number of codes and code ranges the CMap defines.
    pub fn len(&self) -> usize {
        self.singles.len() + self.ranges.len()
    }

    /// Whether the CMap defines no mapping at all.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
