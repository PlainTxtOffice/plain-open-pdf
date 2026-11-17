//! Content streams: operands followed by an operator
//! (ISO 32000-2 section 7.8.2).

use super::cursor::whitespace;
use super::{ParseLimits, Parser};
use crate::{ParseError, ParseErrorKind, PdfObject};

/// One instruction of a content stream.
#[derive(Debug, Clone, PartialEq)]
pub struct Operation {
    /// The operator keyword, such as `Tj` or `cm`.
    pub operator: Vec<u8>,
    /// The operands that preceded the operator, in order.
    pub operands: Vec<PdfObject>,
}

/// Parse a content stream into its operations.
///
/// An inline image is one `BI` operation whose operands are the keys and
/// values of its dictionary followed by its image data as a string: the
/// bytes after the whitespace that follows `ID`, up to the whitespace
/// before `EI`. Operands left without an operator at the end are dropped,
/// and a stray delimiter is skipped.
///
/// # Errors
///
/// Returns a located [`ParseError`] for an operand that cannot be parsed or
/// an inline image without its `ID` keyword.
pub fn parse_content(bytes: &[u8]) -> Result<Vec<Operation>, ParseError> {
    let limits = ParseLimits {
        max_bytes: bytes.len(),
        ..ParseLimits::default()
    };
    let mut parser = Parser::new(bytes, limits)?;
    let mut operations = Vec::new();
    let mut operands = Vec::new();
    loop {
        parser.cursor.skip_trivia()?;
        let Some(byte) = parser.cursor.peek()? else {
            return Ok(operations);
        };
        if starts_object(byte) {
            operands.push(parser.value(0)?);
            continue;
        }
        match parser.cursor.word()? {
            b"" => {
                parser.cursor.take()?;
            }
            b"true" => operands.push(PdfObject::Boolean(true)),
            b"false" => operands.push(PdfObject::Boolean(false)),
            b"null" => operands.push(PdfObject::Null),
            operator => {
                if operator == b"BI" {
                    operands = parser.inline_image()?;
                }
                operations.push(Operation {
                    operator: operator.to_vec(),
                    operands: std::mem::take(&mut operands),
                });
                // The object limit bounds one operation, not the stream.
                parser.objects = 0;
            }
        }
    }
}

/// Whether a byte begins a name, string, array, dictionary, or number.
fn starts_object(byte: u8) -> bool {
    matches!(
        byte,
        b'/' | b'(' | b'[' | b'<' | b'+' | b'-' | b'.' | b'0'..=b'9'
    )
}

impl Parser<'_> {
    /// Read an inline image after `BI`: key and value pairs, `ID`, the image
    /// data, and `EI`. Returns the keys and values, then the data.
    fn inline_image(&mut self) -> Result<Vec<PdfObject>, ParseError> {
        let mut entries = Vec::new();
        loop {
            self.cursor.skip_trivia()?;
            if self.cursor.peek()? == Some(b'/') {
                entries.push(self.value(0)?);
                entries.push(self.value(0)?);
                continue;
            }
            let offset = self.cursor.position;
            if self.cursor.word()? != b"ID" {
                return Err(ParseError {
                    offset,
                    kind: ParseErrorKind::UnexpectedToken,
                });
            }
            break;
        }
        // The data ends at an `EI` that stands alone between whitespace.
        let data = self.cursor.remaining();
        let end = (0..data.len().saturating_sub(1)).find(|&index| {
            &data[index..index + 2] == b"EI"
                && (index == 0 || whitespace(data[index - 1]))
                && data.get(index + 2).is_none_or(|&byte| whitespace(byte))
        });
        // One whitespace byte follows `ID`, and one comes before `EI`.
        let start = data.len().min(1);
        let stop = end
            .map_or(data.len(), |end| end.saturating_sub(1))
            .max(start);
        entries.push(PdfObject::String(data[start..stop].to_vec()));
        self.cursor.position += end.map_or(data.len(), |end| end + 2);
        Ok(entries)
    }
}
