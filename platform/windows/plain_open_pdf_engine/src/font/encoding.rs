//! Simple font encodings: from the one-byte character codes of a Type 1,
//! TrueType, or Type 3 font to Unicode (ISO 32000-2 sections 9.6.5 and
//! 9.10.2).

use super::tables::{LATIN, SYMBOL, ZAPF_DINGBATS};
use crate::PdfObject;

/// Glyph names in common use that the Annex D character sets lack.
const OTHER_NAMES: &[(&str, char)] = &[
    ("dotlessj", '\u{237}'),
    ("ff", '\u{fb00}'),
    ("ffi", '\u{fb03}'),
    ("ffl", '\u{fb04}'),
    ("nbspace", '\u{a0}'),
    ("sfthyphen", '\u{ad}'),
];

/// Find a row of a table sorted by name.
fn find<'a, T>(table: &'a [T], name: &str, key: impl Fn(&T) -> &str) -> Option<&'a T> {
    let index = table.binary_search_by(|row| key(row).cmp(name)).ok()?;
    table.get(index)
}

/// The character a glyph name of the known character sets stands for.
fn named_character(name: &str) -> Option<char> {
    find(LATIN, name, |row| row.0)
        .map(|row| row.1)
        .or_else(|| find(SYMBOL, name, |row| row.0).map(|row| row.1))
        .or_else(|| find(ZAPF_DINGBATS, name, |row| row.0).map(|row| row.1))
        .or_else(|| find(OTHER_NAMES, name, |row| row.0).map(|row| row.1))
}

fn scalar(digits: &str) -> Option<char> {
    if !digits.bytes().all(|digit| digit.is_ascii_hexdigit()) {
        return None;
    }
    char::from_u32(u32::from_str_radix(digits, 16).ok()?)
}

/// The text of a name that spells out code points: `uni` followed by groups
/// of four hexadecimal digits, or `u` followed by four to six.
fn coded_text(name: &str) -> Option<String> {
    if let Some(digits) = name.strip_prefix("uni")
        && !digits.is_empty()
        && digits.len().is_multiple_of(4)
        && digits.is_ascii()
    {
        return (0..digits.len())
            .step_by(4)
            .map(|start| scalar(digits.get(start..start + 4)?))
            .collect();
    }
    let digits = name.strip_prefix('u')?;
    (4..=6)
        .contains(&digits.len())
        .then(|| scalar(digits))?
        .map(String::from)
}

/// The text a glyph name stands for, if the name is a known one.
///
/// Known names are those of the standard Latin, Symbol, and ZapfDingbats
/// character sets, a few ligatures, and names that spell out code points
/// (`uni00E9`, `u1F600`). A suffix that starts with a period is ignored, and
/// the parts of a name joined by underscores are read one by one, so `f_i`
/// is "fi".
pub fn glyph_text(name: &[u8]) -> Option<String> {
    let name = std::str::from_utf8(name).ok()?;
    let base = name.split('.').next().filter(|base| !base.is_empty())?;
    base.split('_')
        .map(|part| {
            named_character(part)
                .map(String::from)
                .or_else(|| coded_text(part))
        })
        .collect()
}

/// The text of each of a simple font's 256 character codes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimpleEncoding {
    /// Indexed by code; a code without a character has empty text.
    codes: Vec<String>,
    /// The glyph name of each code; empty for a code without a glyph.
    names: Vec<String>,
}

impl SimpleEncoding {
    fn empty() -> Self {
        Self {
            codes: vec![String::new(); 256],
            names: vec![String::new(); 256],
        }
    }

    /// Fill an encoding from table rows; code 0 marks a row it lacks.
    fn from_table<T>(table: &[T], entry: impl Fn(&T) -> (&str, char, u8)) -> Self {
        let mut encoding = Self::empty();
        for row in table {
            let (name, character, code) = entry(row);
            if code != 0 {
                encoding.set(code, name, character);
            }
        }
        encoding
    }

    fn set(&mut self, code: u8, name: &str, character: char) {
        let code = usize::from(code);
        if let (Some(text), Some(glyph)) = (self.codes.get_mut(code), self.names.get_mut(code)) {
            *text = character.to_string();
            *glyph = name.to_owned();
        }
    }

    /// `StandardEncoding`, the built-in encoding of Latin-text Type 1 fonts.
    pub fn standard() -> Self {
        Self::from_table(LATIN, |row| (row.0, row.1, row.2))
    }

    /// `MacRomanEncoding`.
    pub fn mac_roman() -> Self {
        let mut encoding = Self::from_table(LATIN, |row| (row.0, row.1, row.3));
        // The space is also encoded as 312 (Annex D.2, footnote 6).
        encoding.set(0xCA, "space", ' ');
        encoding
    }

    /// `WinAnsiEncoding`.
    pub fn win_ansi() -> Self {
        let mut encoding = Self::from_table(LATIN, |row| (row.0, row.1, row.4));
        // The space and the hyphen have a second code, and every unused
        // code above 40 is a bullet (Annex D.2, footnotes 3, 5, and 6).
        encoding.set(0xA0, "space", ' ');
        encoding.set(0xAD, "hyphen", '-');
        for code in 0x21..=0xFF {
            if encoding.text(code).is_none() {
                encoding.set(code, "bullet", '\u{2022}');
            }
        }
        encoding
    }

    /// The built-in encoding of the Symbol font.
    pub fn symbol() -> Self {
        Self::from_table(SYMBOL, |row| (row.0, row.1, row.2))
    }

    /// The built-in encoding of the ZapfDingbats font.
    pub fn zapf_dingbats() -> Self {
        Self::from_table(ZAPF_DINGBATS, |row| (row.0, row.1, row.2))
    }

    /// A predefined encoding by its PDF name. `MacExpertEncoding` is not
    /// supported.
    pub fn named(name: &[u8]) -> Option<Self> {
        match name {
            b"StandardEncoding" => Some(Self::standard()),
            b"MacRomanEncoding" => Some(Self::mac_roman()),
            b"WinAnsiEncoding" => Some(Self::win_ansi()),
            _ => None,
        }
    }

    /// The encoding a font starts from, before any `/Differences`.
    ///
    /// `base_encoding` is the font's `/Encoding` name or the `/BaseEncoding`
    /// of its encoding dictionary. Without a supported one, the font's
    /// built-in encoding applies: Symbol and ZapfDingbats have their own, a
    /// Type 3 font has none, and any other font is taken to use
    /// `StandardEncoding`; a font program's own encoding is applied where
    /// its glyphs are looked up.
    pub fn for_font(subtype: &[u8], base_font: &[u8], base_encoding: Option<&[u8]>) -> Self {
        if let Some(encoding) = base_encoding.and_then(Self::named) {
            return encoding;
        }
        // A subset font's name starts with six letters and a plus sign.
        let name = match base_font.get(6) {
            Some(b'+') => base_font.get(7..).unwrap_or_default(),
            _ => base_font,
        };
        if name.starts_with(b"Symbol") {
            Self::symbol()
        } else if name.starts_with(b"ZapfDingbats") {
            Self::zapf_dingbats()
        } else if subtype == b"Type3" {
            Self::empty()
        } else {
            Self::standard()
        }
    }

    /// Apply a `/Differences` array: each number sets the code of the glyph
    /// name after it, and each further name takes the next code. A code
    /// given an unknown glyph name has no text.
    pub fn apply_differences(&mut self, differences: &[PdfObject]) {
        let mut next = None;
        for item in differences {
            match item {
                PdfObject::Integer(code) => next = usize::try_from(*code).ok(),
                PdfObject::Name(name) => {
                    if let Some(code) = next {
                        if let Some(text) = self.codes.get_mut(code) {
                            *text = glyph_text(name).unwrap_or_default();
                        }
                        if let Some(glyph) = self.names.get_mut(code) {
                            *glyph = String::from_utf8_lossy(name).into_owned();
                        }
                        next = Some(code + 1);
                    }
                }
                _ => {}
            }
        }
    }

    /// The glyph name of one character code, if the encoding has one.
    pub fn glyph_name(&self, code: u8) -> Option<&str> {
        self.names
            .get(usize::from(code))
            .map(String::as_str)
            .filter(|name| !name.is_empty())
    }

    /// The text of one character code, if the encoding has it.
    pub fn text(&self, code: u8) -> Option<&str> {
        self.codes
            .get(usize::from(code))
            .map(String::as_str)
            .filter(|text| !text.is_empty())
    }

    /// Decode a string shown with the font. A code the encoding lacks
    /// becomes U+FFFD.
    pub fn decode(&self, bytes: &[u8]) -> String {
        let mut decoded = String::new();
        for &code in bytes {
            match self.text(code) {
                Some(text) => decoded.push_str(text),
                None => decoded.push(char::REPLACEMENT_CHARACTER),
            }
        }
        decoded
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_are_sorted_by_name() {
        assert!(LATIN.is_sorted_by(|a, b| a.0 < b.0));
        assert!(SYMBOL.is_sorted_by(|a, b| a.0 < b.0));
        assert!(ZAPF_DINGBATS.is_sorted_by(|a, b| a.0 < b.0));
        assert!(OTHER_NAMES.is_sorted_by(|a, b| a.0 < b.0));
    }

    #[test]
    fn no_encoding_uses_a_code_twice() {
        fn distinct(codes: impl Iterator<Item = u8>) -> bool {
            let mut seen = [false; 256];
            codes
                .filter(|&code| code != 0)
                .all(|code| !std::mem::replace(&mut seen[usize::from(code)], true))
        }
        assert!(distinct(LATIN.iter().map(|row| row.2)));
        assert!(distinct(LATIN.iter().map(|row| row.3)));
        assert!(distinct(LATIN.iter().map(|row| row.4)));
        assert!(distinct(SYMBOL.iter().map(|row| row.2)));
        assert!(distinct(ZAPF_DINGBATS.iter().map(|row| row.2)));
    }
}
