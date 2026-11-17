//! PDF number syntax and bounded lookahead for indirect references.

use super::Parser;
use crate::{ObjectReference, ParseError, ParseErrorKind, PdfObject};
use std::borrow::Cow;

fn unsigned(word: &[u8]) -> bool {
    !word.is_empty() && word.iter().all(u8::is_ascii_digit)
}

fn ascii(word: &[u8]) -> &str {
    std::str::from_utf8(word).unwrap_or_default()
}

/// Text Rust can parse as a number, or `None` when the word has other bytes.
///
/// Malformed words made only of digits, signs, and periods are repaired:
/// signs after the first byte are ignored, text from a second period on is
/// dropped, and a word without digits reads as zero.
fn number_text(word: &[u8]) -> Option<Cow<'_, str>> {
    if !word
        .iter()
        .all(|byte| matches!(byte, b'0'..=b'9' | b'+' | b'-' | b'.'))
    {
        return None;
    }
    let body = match word.first() {
        Some(b'+' | b'-') => &word[1..],
        _ => word,
    };
    let digits = body.iter().filter(|byte| byte.is_ascii_digit()).count();
    let dots = body.iter().filter(|&&byte| byte == b'.').count();
    if digits > 0 && dots <= 1 && digits + dots == body.len() {
        return Some(Cow::Borrowed(ascii(word)));
    }
    let mut text = String::new();
    if word.first() == Some(&b'-') {
        text.push('-');
    }
    let mut has_digit = false;
    let mut has_dot = false;
    for &byte in word {
        match byte {
            b'.' if has_dot => break,
            b'.' => {
                has_dot = true;
                text.push('.');
            }
            b'0'..=b'9' => {
                has_digit = true;
                text.push(char::from(byte));
            }
            _ => {}
        }
    }
    Some(if has_digit {
        Cow::Owned(text)
    } else {
        Cow::Borrowed("0")
    })
}

impl Parser<'_> {
    pub(super) fn number(&mut self) -> Result<PdfObject, ParseError> {
        let offset = self.cursor.position;
        let word = self.cursor.word()?;
        let text = number_text(word).ok_or(ParseError {
            offset,
            kind: ParseErrorKind::InvalidNumber,
        })?;
        if unsigned(word)
            && let Some(reference) = self.reference(word)?
        {
            return Ok(reference);
        }
        if !text.contains('.')
            && let Ok(value) = text.parse()
        {
            return Ok(PdfObject::Integer(value));
        }
        // Reals, and integers beyond i64, use the nearest finite f64.
        match text.parse::<f64>() {
            Ok(value) if value.is_finite() => Ok(PdfObject::Real(value)),
            _ => Err(ParseError {
                offset,
                kind: ParseErrorKind::NumberOutOfRange,
            }),
        }
    }

    /// Consume `generation R` after an unsigned integer, if present.
    ///
    /// A reference that cannot name an object (number zero, or a number or
    /// generation out of range) reads as null, like any undefined object.
    fn reference(&mut self, first: &[u8]) -> Result<Option<PdfObject>, ParseError> {
        let mut probe = self.cursor;
        probe.skip_trivia()?;
        let second = probe.word()?;
        if !unsigned(second) {
            return Ok(None);
        }
        probe.skip_trivia()?;
        if probe.word()? != b"R" {
            return Ok(None);
        }
        self.cursor = probe;
        Ok(Some(
            match (ascii(first).parse::<u32>(), ascii(second).parse::<u16>()) {
                (Ok(number), Ok(generation)) if number != 0 => {
                    PdfObject::Reference(ObjectReference { number, generation })
                }
                _ => PdfObject::Null,
            },
        ))
    }
}
