//! Stream filters: decoding stored stream bytes into their original data.

mod ascii;
mod ccitt;
#[cfg(test)]
mod codec_tests;
mod dct;
mod deflate;
mod flate;
mod lzw;
mod run_length;
#[cfg(test)]
mod tests;

pub(crate) use ascii::{ascii_hex_decode, ascii85_decode};
pub(crate) use ccitt::{FaxParameters, ccitt_decode};
pub(crate) use dct::{dct_decode, dct_decode_reduced};
pub(crate) use deflate::deflate_zlib;
pub(crate) use flate::inflate_zlib;
pub(crate) use lzw::lzw_decode;
pub(crate) use run_length::run_length_decode;

/// Why stream data could not be decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FilterError {
    /// The encoded data is malformed.
    Corrupt,
    /// The decoded data would exceed the caller's size limit.
    LimitExceeded,
    /// The filter or one of its parameters is not implemented.
    Unsupported,
}

/// Parameters of a `FlateDecode` or `LZWDecode` predictor.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Predictor {
    /// The `/Predictor` value: 1 is none, 2 is TIFF, 10 and above are PNG.
    pub kind: i64,
    /// Samples per row.
    pub columns: usize,
    /// Color components per sample.
    pub colors: usize,
    /// Bits per color component.
    pub bits_per_component: usize,
}

/// Reverse the predictor applied before compression.
pub(crate) fn unpredict(data: Vec<u8>, predictor: Predictor) -> Result<Vec<u8>, FilterError> {
    match predictor.kind {
        ..=1 => Ok(data),
        2 => tiff_unpredict(data, predictor),
        _ => png_unpredict(&data, predictor),
    }
}

/// Reverse the TIFF predictor: each component was stored as its difference
/// from the same component of the sample to its left.
///
/// Components of 8 and 16 bits are supported; packed samples are not.
fn tiff_unpredict(mut data: Vec<u8>, predictor: Predictor) -> Result<Vec<u8>, FilterError> {
    let component_bytes = match predictor.bits_per_component {
        8 => 1,
        16 => 2,
        _ => return Err(FilterError::Unsupported),
    };
    let sample_bytes = predictor
        .colors
        .checked_mul(component_bytes)
        .ok_or(FilterError::Corrupt)?;
    let row_bytes = sample_bytes
        .checked_mul(predictor.columns)
        .ok_or(FilterError::Corrupt)?;
    if row_bytes == 0 {
        return Err(FilterError::Corrupt);
    }
    for row in data.chunks_mut(row_bytes) {
        for index in sample_bytes..row.len() {
            if component_bytes == 1 {
                row[index] = row[index].wrapping_add(row[index - sample_bytes]);
            } else if index % 2 == 1 {
                // Add big-endian 16-bit components once both bytes are known.
                let left =
                    u16::from_be_bytes([row[index - 1 - sample_bytes], row[index - sample_bytes]]);
                let value = u16::from_be_bytes([row[index - 1], row[index]]).wrapping_add(left);
                row[index - 1..=index].copy_from_slice(&value.to_be_bytes());
            }
        }
    }
    Ok(data)
}

/// Reverse PNG row filters; each row starts with a byte naming its filter.
fn png_unpredict(data: &[u8], predictor: Predictor) -> Result<Vec<u8>, FilterError> {
    let sample_bits = predictor
        .colors
        .checked_mul(predictor.bits_per_component)
        .ok_or(FilterError::Corrupt)?;
    let row_bits = sample_bits
        .checked_mul(predictor.columns)
        .ok_or(FilterError::Corrupt)?;
    let pixel_bytes = sample_bits.div_ceil(8).max(1);
    let row_bytes = row_bits.div_ceil(8);
    if data.is_empty() {
        return Ok(Vec::new());
    }
    // A row longer than the data also guards the row buffer allocation.
    if row_bytes == 0 || row_bytes > data.len() {
        return Err(FilterError::Corrupt);
    }
    let mut output = Vec::with_capacity(data.len());
    let mut previous = vec![0u8; row_bytes];
    for encoded in data.chunks(row_bytes + 1) {
        let (&filter, encoded) = encoded.split_first().ok_or(FilterError::Corrupt)?;
        let mut row = encoded.to_vec();
        for index in 0..row.len() {
            let left = if index >= pixel_bytes {
                row[index - pixel_bytes]
            } else {
                0
            };
            let up = previous[index];
            let up_left = if index >= pixel_bytes {
                previous[index - pixel_bytes]
            } else {
                0
            };
            let prediction = match filter {
                0 => 0,
                1 => left,
                2 => up,
                3 => ((u16::from(left) + u16::from(up)) / 2) as u8,
                4 => paeth(left, up, up_left),
                _ => return Err(FilterError::Corrupt),
            };
            row[index] = row[index].wrapping_add(prediction);
        }
        output.extend_from_slice(&row);
        previous[..row.len()].copy_from_slice(&row);
    }
    Ok(output)
}

/// The PNG Paeth predictor: whichever neighbor is nearest to their gradient.
fn paeth(left: u8, up: u8, up_left: u8) -> u8 {
    let estimate = i16::from(left) + i16::from(up) - i16::from(up_left);
    let left_distance = (estimate - i16::from(left)).abs();
    let up_distance = (estimate - i16::from(up)).abs();
    let up_left_distance = (estimate - i16::from(up_left)).abs();
    if left_distance <= up_distance && left_distance <= up_left_distance {
        left
    } else if up_distance <= up_left_distance {
        up
    } else {
        up_left
    }
}
