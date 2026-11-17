//! `LZWDecode`: variable-width codes, most significant bit first.

use super::FilterError;

const CLEAR_TABLE: usize = 256;
const END_OF_DATA: usize = 257;
const FIRST_ENTRY: usize = 258;
const MAX_ENTRIES: usize = 4096;

/// Decompress LZW data into at most `max_output` bytes.
///
/// `early_change` is the filter's `/EarlyChange` parameter: when true, the
/// default, codes grow one code earlier than the table strictly requires.
/// Data that stops without an end-of-data code yields what was decoded.
pub(crate) fn lzw_decode(
    data: &[u8],
    early_change: bool,
    max_output: usize,
) -> Result<Vec<u8>, FilterError> {
    // Each table entry is an earlier sequence plus one byte.
    let mut prefixes = [0u16; MAX_ENTRIES];
    let mut suffixes = [0u8; MAX_ENTRIES];
    let mut next = FIRST_ENTRY;
    let mut width = 9;
    let mut previous: Option<usize> = None;

    let mut output = Vec::new();
    let mut sequence = Vec::new();
    let mut buffer = 0u32;
    let mut buffered = 0;
    let mut bytes = data.iter();
    loop {
        while buffered < width {
            let Some(&byte) = bytes.next() else {
                return Ok(output);
            };
            buffer = buffer << 8 | u32::from(byte);
            buffered += 8;
        }
        buffered -= width;
        let code = (buffer >> buffered) as usize & ((1 << width) - 1);

        match code {
            CLEAR_TABLE => {
                next = FIRST_ENTRY;
                width = 9;
                previous = None;
                continue;
            }
            END_OF_DATA => return Ok(output),
            _ => {}
        }
        // A code equal to the next entry names the sequence being defined:
        // the previous sequence followed by its own first byte.
        let known = match previous {
            Some(previous) if code == next => previous,
            _ if code < next => code,
            _ => return Err(FilterError::Corrupt),
        };
        sequence.clear();
        let mut entry = known;
        while entry >= FIRST_ENTRY {
            sequence.push(suffixes[entry]);
            entry = usize::from(prefixes[entry]);
        }
        let first = entry as u8;
        sequence.push(first);
        sequence.reverse();
        if code == next {
            sequence.push(first);
        }
        if output.len() + sequence.len() > max_output {
            return Err(FilterError::LimitExceeded);
        }
        output.extend_from_slice(&sequence);

        if let Some(previous) = previous
            && next < MAX_ENTRIES
        {
            prefixes[next] = previous as u16;
            suffixes[next] = first;
            next += 1;
        }
        previous = Some(code);
        width = match next + usize::from(early_change) {
            ..512 => 9,
            512..1024 => 10,
            1024..2048 => 11,
            _ => 12,
        };
    }
}
