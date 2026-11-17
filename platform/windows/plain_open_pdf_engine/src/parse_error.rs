//! Object parser errors with locations in the supplied byte slice.

use std::fmt;

/// The reason object parsing failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseErrorKind {
    /// The input ended before an object was complete.
    UnexpectedEnd,
    /// A token is not valid in this position.
    UnexpectedToken,
    /// A token that starts like a number contains a non-numeric byte.
    InvalidNumber,
    /// A number is too large for a finite `f64`.
    NumberOutOfRange,
    /// A hexadecimal string contains a non-hexadecimal, non-whitespace byte.
    InvalidHexString,
    /// An input byte, object count, or nesting limit was exceeded.
    LimitExceeded,
    /// The requested nesting limit exceeds the supported maximum of 64.
    InvalidLimits,
}

/// A parsing error at a zero-based byte offset, including EOF positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseError {
    /// Offset relative to the byte slice supplied to `parse_object`.
    pub offset: usize,
    /// Machine-readable failure category.
    pub kind: ParseErrorKind,
}

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?} at byte {}", self.kind, self.offset)
    }
}

impl std::error::Error for ParseError {}
