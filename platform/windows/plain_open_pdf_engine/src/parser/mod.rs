//! Bounded parsing of PDF object syntax, without file access or reference resolution.

mod collections;
mod content;
mod cursor;
mod numbers;
mod strings;

use crate::{ParseError, ParseErrorKind, ParsedObject, PdfObject};
use cursor::Cursor;

pub use content::{Operation, parse_content};

/// Resource bounds for each call to [`parse_object`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseLimits {
    /// Maximum input prefix examined, including comments and reference lookahead.
    pub max_bytes: usize,
    /// Maximum simultaneously nested arrays and dictionaries.
    /// Values above 64 are rejected to bound recursive stack use.
    pub max_depth: usize,
    /// Maximum values, including containers and dictionary keys.
    pub max_objects: usize,
}

impl Default for ParseLimits {
    fn default() -> Self {
        Self {
            max_bytes: 16 * 1_048_576,
            max_depth: 32,
            max_objects: 500_000,
        }
    }
}

/// Parse one object prefix using PDF lexical rules (ISO 32000, 7.2 and 7.3).
///
/// Trailing bytes are not validated. A dictionary followed by `stream` returns
/// only the dictionary; streams and indirect-object wrappers require a future
/// document parser. Byte strings are not decrypted or decoded as text.
/// Numeric storage is limited to i64, finite f64, and u32/u16 references.
///
/// Common real-world defects are repaired, not rejected: the last duplicate
/// dictionary key wins, references that cannot name an object (such as
/// `0 0 R`) read as null, malformed name escapes stay literal, and malformed
/// numbers such as `--5` or `1.2.3` read as the nearest sensible value.
///
/// ```
/// use plain_open_pdf_engine::{ParseLimits, PdfObject, parse_object};
/// let parsed = parse_object(b"[0 0 612 792]", ParseLimits::default())?;
/// assert!(matches!(parsed.value, PdfObject::Array(_)));
/// assert_eq!(parsed.consumed, 13);
/// # Ok::<(), plain_open_pdf_engine::ParseError>(())
/// ```
///
/// # Errors
/// Returns a located [`ParseError`] for unrepairable syntax, numbers beyond
/// finite f64, exceeded resource limits, or a nesting limit greater than 64.
pub fn parse_object(bytes: &[u8], limits: ParseLimits) -> Result<ParsedObject, ParseError> {
    let mut parser = Parser::new(bytes, limits)?;
    let value = parser.value(0)?;
    Ok(ParsedObject {
        value,
        consumed: parser.cursor.position,
    })
}

/// An indirect object definition: `number generation obj value endobj`.
pub(crate) struct IndirectObject {
    /// The object number the definition declares.
    pub number: u32,
    /// The generation number the definition declares.
    pub generation: u32,
    /// The object's value; for a stream, its dictionary.
    pub value: PdfObject,
    /// Offset of the first byte of stream data, when the object is a stream.
    pub stream_start: Option<usize>,
}

/// Parse an indirect object definition from the start of a byte slice.
///
/// Stream data is located, not read: its length comes from the dictionary and
/// may itself be an indirect reference. `endobj` is not required.
pub(crate) fn parse_indirect_object(
    bytes: &[u8],
    limits: ParseLimits,
) -> Result<IndirectObject, ParseError> {
    let mut parser = Parser::new(bytes, limits)?;
    let number = parser.header_number()?;
    let generation = parser.header_number()?;
    parser.cursor.skip_trivia()?;
    let offset = parser.cursor.position;
    if parser.cursor.word()? != b"obj" {
        return Err(ParseError {
            offset,
            kind: ParseErrorKind::UnexpectedToken,
        });
    }
    let value = parser.value(0)?;
    let mut probe = parser.cursor;
    probe.skip_trivia()?;
    let stream_start = if probe.word()? == b"stream" {
        // The keyword ends with CRLF or LF; a lone CR is tolerated.
        if probe.peek()? == Some(b'\r') {
            probe.take()?;
        }
        if probe.peek()? == Some(b'\n') {
            probe.take()?;
        }
        Some(probe.position)
    } else {
        None
    };
    Ok(IndirectObject {
        number,
        generation,
        value,
        stream_start,
    })
}

struct Parser<'a> {
    cursor: Cursor<'a>,
    limits: ParseLimits,
    objects: usize,
}

impl<'a> Parser<'a> {
    fn new(bytes: &'a [u8], limits: ParseLimits) -> Result<Self, ParseError> {
        if limits.max_depth > 64 {
            return Err(ParseError {
                offset: 0,
                kind: ParseErrorKind::InvalidLimits,
            });
        }
        Ok(Self {
            cursor: Cursor::new(bytes, limits.max_bytes),
            limits,
            objects: 0,
        })
    }

    /// Read the object or generation number that opens an indirect object.
    fn header_number(&mut self) -> Result<u32, ParseError> {
        self.cursor.skip_trivia()?;
        let offset = self.cursor.position;
        let word = self.cursor.word()?;
        std::str::from_utf8(word)
            .ok()
            .and_then(|text| text.parse().ok())
            .ok_or(ParseError {
                offset,
                kind: ParseErrorKind::UnexpectedToken,
            })
    }

    fn count_object(&mut self) -> Result<(), ParseError> {
        if self.objects >= self.limits.max_objects {
            return Err(self.cursor.error(ParseErrorKind::LimitExceeded));
        }
        self.objects += 1;
        Ok(())
    }

    fn check_depth(&self, depth: usize) -> Result<(), ParseError> {
        if depth > self.limits.max_depth {
            return Err(self.cursor.error(ParseErrorKind::LimitExceeded));
        }
        Ok(())
    }

    fn value(&mut self, depth: usize) -> Result<PdfObject, ParseError> {
        self.cursor.skip_trivia()?;
        self.count_object()?;
        match self.cursor.peek()? {
            Some(b'/') => self.name().map(PdfObject::Name),
            Some(b'(') => self.literal().map(PdfObject::String),
            Some(b'[') => self.array(depth + 1),
            Some(b'<') => {
                self.cursor.take()?;
                if self.cursor.peek()? == Some(b'<') {
                    self.cursor.take()?;
                    self.dictionary(depth + 1)
                } else {
                    self.hex_string().map(PdfObject::String)
                }
            }
            Some(b'+' | b'-' | b'.' | b'0'..=b'9') => self.number(),
            Some(_) => self.keyword(),
            None => Err(self.cursor.error(ParseErrorKind::UnexpectedEnd)),
        }
    }

    fn keyword(&mut self) -> Result<PdfObject, ParseError> {
        let offset = self.cursor.position;
        match self.cursor.word()? {
            b"true" => Ok(PdfObject::Boolean(true)),
            b"false" => Ok(PdfObject::Boolean(false)),
            b"null" => Ok(PdfObject::Null),
            _ => Err(ParseError {
                offset,
                kind: ParseErrorKind::UnexpectedToken,
            }),
        }
    }
}
