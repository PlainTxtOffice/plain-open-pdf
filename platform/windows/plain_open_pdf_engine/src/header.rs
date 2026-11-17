//! Parsing of the PDF version header near the start of a file.

use std::fmt;

/// The version declared in the PDF header, not a promise of engine support.
///
/// A future document parser must also consider catalog version overrides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PdfVersion {
    /// Major version number.
    pub major: u8,
    /// Minor version number.
    pub minor: u8,
}

impl fmt::Display for PdfVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}", self.major, self.minor)
    }
}

/// Why the input does not contain a recognized, complete PDF header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderError {
    /// The input ends before the signature and version are complete.
    TruncatedHeader,
    /// `%PDF-` does not begin within the first 1024 bytes.
    InvalidSignature,
    /// The version is not two ASCII digits separated by a period.
    InvalidVersion,
    /// The version is outside PDF 1.0 through 1.7 and PDF 2.0.
    UnsupportedVersion(PdfVersion),
}

impl fmt::Display for HeaderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TruncatedHeader => formatter.write_str("PDF header is truncated"),
            Self::InvalidSignature => {
                formatter.write_str("expected %PDF- within the first 1024 bytes")
            }
            Self::InvalidVersion => formatter.write_str("expected a PDF version such as 1.7"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unrecognized PDF header version: {version}")
            }
        }
    }
}

impl std::error::Error for HeaderError {}

/// How far into the input the `%PDF-` signature may begin.
const SIGNATURE_SEARCH_BYTES: usize = 1024;

/// Parse the PDF header found near the beginning of a byte slice.
///
/// Accepts PDF 1.0 through 1.7 and 2.0. As other readers do, the `%PDF-`
/// signature may begin anywhere in the first 1024 bytes, and the bytes after
/// the version are not examined.
///
/// # Errors
///
/// Returns a [`HeaderError`] for a missing, incomplete, malformed, or
/// unrecognized header. Even a successful result does not validate the
/// document body.
pub fn parse_header(bytes: &[u8]) -> Result<PdfVersion, HeaderError> {
    locate_header(bytes).map(|(_, version)| version)
}

/// Find the header and return the offset of its `%PDF-` signature.
///
/// Byte offsets stored in a file are measured from that signature.
pub(crate) fn locate_header(bytes: &[u8]) -> Result<(usize, PdfVersion), HeaderError> {
    const SIGNATURE: &[u8] = b"%PDF-";
    let window_end = bytes
        .len()
        .min(SIGNATURE_SEARCH_BYTES - 1 + SIGNATURE.len());
    let Some(start) = bytes[..window_end]
        .windows(SIGNATURE.len())
        .position(|window| window == SIGNATURE)
    else {
        return Err(if bytes.len() < 8 {
            HeaderError::TruncatedHeader
        } else {
            HeaderError::InvalidSignature
        });
    };
    let header = bytes
        .get(start..start + 8)
        .ok_or(HeaderError::TruncatedHeader)?;
    if !header[5].is_ascii_digit() || header[6] != b'.' || !header[7].is_ascii_digit() {
        return Err(HeaderError::InvalidVersion);
    }
    let version = PdfVersion {
        major: header[5] - b'0',
        minor: header[7] - b'0',
    };
    if !matches!((version.major, version.minor), (1, 0..=7) | (2, 0)) {
        return Err(HeaderError::UnsupportedVersion(version));
    }
    Ok((start, version))
}
