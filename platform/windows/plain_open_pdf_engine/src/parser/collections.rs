//! Array and dictionary grammar with nesting and allocation bounds.

use super::Parser;
use crate::{ParseError, ParseErrorKind, PdfObject};
use std::collections::BTreeMap;

impl Parser<'_> {
    pub(super) fn array(&mut self, depth: usize) -> Result<PdfObject, ParseError> {
        self.check_depth(depth)?;
        self.cursor.take()?;
        let mut values = Vec::new();
        loop {
            self.cursor.skip_trivia()?;
            if self.cursor.peek()? == Some(b']') {
                self.cursor.take()?;
                return Ok(PdfObject::Array(values));
            }
            values.push(self.value(depth)?);
        }
    }

    pub(super) fn dictionary(&mut self, depth: usize) -> Result<PdfObject, ParseError> {
        self.check_depth(depth)?;
        let mut values = BTreeMap::new();
        loop {
            self.cursor.skip_trivia()?;
            match self.cursor.peek()? {
                Some(b'>') => {
                    self.cursor.take()?;
                    match self.cursor.peek()? {
                        Some(b'>') => {
                            self.cursor.take()?;
                        }
                        Some(_) => return Err(self.cursor.error(ParseErrorKind::UnexpectedToken)),
                        None => return Err(self.cursor.error(ParseErrorKind::UnexpectedEnd)),
                    }
                    return Ok(PdfObject::Dictionary(values));
                }
                Some(b'/') => self.entry(&mut values, depth)?,
                Some(_) => return Err(self.cursor.error(ParseErrorKind::UnexpectedToken)),
                None => return Err(self.cursor.error(ParseErrorKind::UnexpectedEnd)),
            }
        }
    }

    fn entry(
        &mut self,
        values: &mut BTreeMap<Vec<u8>, PdfObject>,
        depth: usize,
    ) -> Result<(), ParseError> {
        self.count_object()?;
        let key = self.name()?;
        // Later duplicates replace earlier ones, as other readers do.
        values.insert(key, self.value(depth)?);
        Ok(())
    }
}
