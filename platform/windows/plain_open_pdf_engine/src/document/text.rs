//! Text strings: the encodings of human-readable strings outside page
//! content (ISO 32000-2 section 7.9.2.2).

/// Code points of PDFDocEncoding bytes 0x18 through 0x1F.
const LOW_SPECIALS: [char; 8] = [
    '\u{02d8}', '\u{02c7}', '\u{02c6}', '\u{02d9}', '\u{02dd}', '\u{02db}', '\u{02da}', '\u{02dc}',
];

/// Code points of PDFDocEncoding bytes 0x80 through 0xA0.
const HIGH_SPECIALS: [char; 33] = [
    '\u{2022}', '\u{2020}', '\u{2021}', '\u{2026}', '\u{2014}', '\u{2013}', '\u{0192}', '\u{2044}',
    '\u{2039}', '\u{203a}', '\u{2212}', '\u{2030}', '\u{201e}', '\u{201c}', '\u{201d}', '\u{2018}',
    '\u{2019}', '\u{201a}', '\u{2122}', '\u{fb01}', '\u{fb02}', '\u{0141}', '\u{0152}', '\u{0160}',
    '\u{0178}', '\u{017d}', '\u{0131}', '\u{0142}', '\u{0153}', '\u{0161}', '\u{017e}', '\u{fffd}',
    '\u{20ac}',
];

/// Decode a text string to Unicode.
///
/// A string that starts with the byte order mark FE FF is UTF-16BE, one that
/// starts with EF BB BF is UTF-8, and any other is PDFDocEncoding. Bytes that
/// the encoding leaves undefined, and malformed sequences, become U+FFFD.
pub fn decode_text_string(bytes: &[u8]) -> String {
    if let Some(units) = bytes.strip_prefix(&[0xfe, 0xff]) {
        let units = units
            .chunks(2)
            .map(|unit| u16::from_be_bytes([unit[0], *unit.get(1).unwrap_or(&0)]));
        return char::decode_utf16(units)
            .map(|unit| unit.unwrap_or(char::REPLACEMENT_CHARACTER))
            .collect();
    }
    if let Some(text) = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]) {
        return String::from_utf8_lossy(text).into_owned();
    }
    bytes
        .iter()
        .map(|&byte| match byte {
            0x18..=0x1f => LOW_SPECIALS[usize::from(byte - 0x18)],
            0x80..=0xa0 => HIGH_SPECIALS[usize::from(byte - 0x80)],
            0x7f | 0xad => char::REPLACEMENT_CHARACTER,
            _ => char::from(byte),
        })
        .collect()
}

/// Encode text as a PDF text string: as it is when it is printable ASCII,
/// and otherwise as UTF-16BE behind a byte order mark.
pub fn encode_text_string(text: &str) -> Vec<u8> {
    if text.bytes().all(|byte| matches!(byte, 0x20..=0x7E)) {
        return text.as_bytes().to_vec();
    }
    let mut bytes = vec![0xFE, 0xFF];
    for unit in text.encode_utf16() {
        bytes.extend(unit.to_be_bytes());
    }
    bytes
}
