//! `ASCIIHexDecode` and `ASCII85Decode`: binary data written as text.

use super::FilterError;

/// PDF whitespace, which both encodings ignore.
fn is_whitespace(byte: u8) -> bool {
    matches!(byte, 0 | 9 | 10 | 12 | 13 | 32)
}

/// Decode pairs of hexadecimal digits up to `>`; an odd final digit is the
/// high half of a last byte.
pub(crate) fn ascii_hex_decode(data: &[u8]) -> Result<Vec<u8>, FilterError> {
    let mut output = Vec::with_capacity(data.len() / 2);
    let mut high = None;
    for &byte in data {
        if byte == b'>' {
            break;
        }
        if is_whitespace(byte) {
            continue;
        }
        let digit = (byte as char).to_digit(16).ok_or(FilterError::Corrupt)? as u8;
        match high.take() {
            Some(high) => output.push(high << 4 | digit),
            None => high = Some(digit),
        }
    }
    output.extend(high.map(|high| high << 4));
    Ok(output)
}

/// Decode base-85 groups: five characters from `!` through `u` make four
/// bytes, `z` stands for four zero bytes, and `~>` ends the data.
pub(crate) fn ascii85_decode(data: &[u8]) -> Result<Vec<u8>, FilterError> {
    let mut output = Vec::with_capacity(data.len() / 5 * 4);
    let mut group = [0u8; 5];
    let mut filled = 0;
    for &byte in data {
        match byte {
            b'~' => break,
            _ if is_whitespace(byte) => {}
            b'z' if filled == 0 => output.extend([0; 4]),
            b'!'..=b'u' => {
                group[filled] = byte - b'!';
                filled += 1;
                if filled == group.len() {
                    output.extend(group_value(group)?.to_be_bytes());
                    filled = 0;
                }
            }
            _ => return Err(FilterError::Corrupt),
        }
    }
    // A final group of n characters holds n - 1 bytes; it is completed with
    // the highest digit so that truncating the result rounds correctly.
    if filled > 1 {
        group[filled..].fill(84);
        output.extend(&group_value(group)?.to_be_bytes()[..filled - 1]);
    }
    Ok(output)
}

fn group_value(group: [u8; 5]) -> Result<u32, FilterError> {
    let value = group
        .iter()
        .fold(0u64, |value, &digit| value * 85 + u64::from(digit));
    u32::try_from(value).map_err(|_| FilterError::Corrupt)
}
