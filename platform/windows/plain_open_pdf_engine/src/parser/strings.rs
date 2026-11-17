//! Name escapes, hexadecimal bytes, and literal string escaping.

use super::{Parser, cursor::whitespace};
use crate::{ParseError, ParseErrorKind};

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

impl Parser<'_> {
    pub(super) fn name(&mut self) -> Result<Vec<u8>, ParseError> {
        self.cursor.take()?;
        let word = self.cursor.word()?;
        let mut result = Vec::new();
        let mut index = 0;
        while index < word.len() {
            // A `#` without two hexadecimal digits is kept as a literal byte.
            let byte = if word[index] == b'#'
                && let Some(high) = word.get(index + 1).copied().and_then(hex)
                && let Some(low) = word.get(index + 2).copied().and_then(hex)
            {
                index += 2;
                (high << 4) | low
            } else {
                word[index]
            };
            result.push(byte);
            index += 1;
        }
        Ok(result)
    }

    pub(super) fn hex_string(&mut self) -> Result<Vec<u8>, ParseError> {
        let mut result = Vec::new();
        let mut high = None;
        loop {
            let offset = self.cursor.position;
            let byte = self.cursor.take()?;
            if byte == b'>' {
                if let Some(value) = high {
                    result.push(value << 4);
                }
                return Ok(result);
            }
            if whitespace(byte) {
                continue;
            }
            let digit = hex(byte).ok_or(ParseError {
                offset,
                kind: ParseErrorKind::InvalidHexString,
            })?;
            if let Some(value) = high.take() {
                result.push((value << 4) | digit);
            } else {
                high = Some(digit);
            }
        }
    }

    pub(super) fn literal(&mut self) -> Result<Vec<u8>, ParseError> {
        self.cursor.take()?;
        let mut nesting = 1usize;
        let mut result = Vec::new();
        loop {
            match self.cursor.take()? {
                b'(' => {
                    nesting += 1;
                    result.push(b'(');
                }
                b')' => {
                    nesting -= 1;
                    if nesting == 0 {
                        return Ok(result);
                    }
                    result.push(b')');
                }
                b'\\' => {
                    if let Some(byte) = self.escape()? {
                        result.push(byte);
                    }
                }
                b'\r' => {
                    self.consume_lf()?;
                    result.push(b'\n');
                }
                byte => result.push(byte),
            }
        }
    }

    fn consume_lf(&mut self) -> Result<(), ParseError> {
        if self.cursor.peek()? == Some(b'\n') {
            self.cursor.take()?;
        }
        Ok(())
    }

    fn escape(&mut self) -> Result<Option<u8>, ParseError> {
        let byte = self.cursor.take()?;
        let decoded = match byte {
            b'n' => b'\n',
            b'r' => b'\r',
            b't' => b'\t',
            b'b' => 8,
            b'f' => 12,
            b'\n' => return Ok(None),
            b'\r' => {
                self.consume_lf()?;
                return Ok(None);
            }
            b'0'..=b'7' => return self.octal(byte).map(Some),
            // PDF ignores the backslash for unrecognized escapes.
            other => other,
        };
        Ok(Some(decoded))
    }

    fn octal(&mut self, first: u8) -> Result<u8, ParseError> {
        let mut value = u16::from(first - b'0');
        for _ in 0..2 {
            match self.cursor.peek()? {
                Some(byte @ b'0'..=b'7') => {
                    self.cursor.take()?;
                    value = value * 8 + u16::from(byte - b'0');
                }
                _ => break,
            }
        }
        Ok((value & 255) as u8)
    }
}
