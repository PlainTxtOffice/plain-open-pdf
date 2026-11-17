//! Byte navigation and token boundaries shared by object syntax parsers.

use crate::{ParseError, ParseErrorKind};

#[derive(Clone, Copy)]
pub(super) struct Cursor<'a> {
    bytes: &'a [u8],
    max_bytes: usize,
    pub position: usize,
}

pub(super) fn whitespace(byte: u8) -> bool {
    matches!(byte, 0 | 9 | 10 | 12 | 13 | 32)
}

pub(super) fn boundary(byte: u8) -> bool {
    whitespace(byte) || b"()<>[]{}/%".contains(&byte)
}

impl<'a> Cursor<'a> {
    pub fn new(bytes: &'a [u8], max_bytes: usize) -> Self {
        Self {
            bytes,
            max_bytes,
            position: 0,
        }
    }

    pub fn error(&self, kind: ParseErrorKind) -> ParseError {
        ParseError {
            offset: self.position,
            kind,
        }
    }

    /// The bytes from the current position to the end of the input.
    pub fn remaining(&self) -> &'a [u8] {
        self.bytes.get(self.position..).unwrap_or_default()
    }

    pub fn peek(&self) -> Result<Option<u8>, ParseError> {
        if self.position >= self.bytes.len() {
            return Ok(None);
        }
        if self.position >= self.max_bytes {
            return Err(self.error(ParseErrorKind::LimitExceeded));
        }
        Ok(Some(self.bytes[self.position]))
    }

    pub fn take(&mut self) -> Result<u8, ParseError> {
        let byte = self
            .peek()?
            .ok_or_else(|| self.error(ParseErrorKind::UnexpectedEnd))?;
        self.position += 1;
        Ok(byte)
    }

    pub fn skip_trivia(&mut self) -> Result<(), ParseError> {
        loop {
            match self.peek()? {
                Some(byte) if whitespace(byte) => {
                    self.take()?;
                }
                Some(b'%') => {
                    while let Some(byte) = self.peek()? {
                        if matches!(byte, b'\r' | b'\n') {
                            break;
                        }
                        self.take()?;
                    }
                }
                _ => return Ok(()),
            }
        }
    }

    pub fn word(&mut self) -> Result<&'a [u8], ParseError> {
        let start = self.position;
        while let Some(byte) = self.peek()? {
            if boundary(byte) {
                break;
            }
            self.take()?;
        }
        Ok(&self.bytes[start..self.position])
    }
}
