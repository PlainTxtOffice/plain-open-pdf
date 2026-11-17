//! `RunLengthDecode`: literal runs and repeated bytes.

use super::FilterError;

/// Decode run-length data into at most `max_output` bytes.
///
/// A length byte below 128 copies the next `length + 1` bytes; one above 128
/// repeats the next byte `257 - length` times; 128 ends the data. Data that
/// stops mid-run yields what was decoded.
pub(crate) fn run_length_decode(data: &[u8], max_output: usize) -> Result<Vec<u8>, FilterError> {
    let mut output = Vec::new();
    let mut rest = data;
    while let Some((&length, tail)) = rest.split_first() {
        let length = usize::from(length);
        let consumed = match length {
            128 => break,
            ..128 => {
                let run = &tail[..tail.len().min(length + 1)];
                output.extend_from_slice(run);
                run.len()
            }
            _ => {
                let Some(&byte) = tail.first() else {
                    break;
                };
                output.resize(output.len() + 257 - length, byte);
                1
            }
        };
        if output.len() > max_output {
            return Err(FilterError::LimitExceeded);
        }
        rest = &tail[consumed..];
    }
    Ok(output)
}
