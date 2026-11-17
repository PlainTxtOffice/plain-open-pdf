//! Values represented by PDF object syntax, independent of document storage.

use std::collections::BTreeMap;

/// Dictionary entries indexed by decoded name bytes.
pub type Dictionary = BTreeMap<Vec<u8>, PdfObject>;

/// A parsed PDF value. Names and strings preserve bytes, not assumed UTF-8.
#[derive(Debug, Clone, PartialEq)]
pub enum PdfObject {
    /// The PDF null value.
    Null,
    /// A boolean value.
    Boolean(bool),
    /// An integer representable in signed 64 bits; larger ones become `Real`.
    Integer(i64),
    /// A finite floating-point approximation of a PDF real number.
    Real(f64),
    /// A decoded name without the leading slash.
    Name(Vec<u8>),
    /// Decoded literal or hexadecimal string bytes.
    String(Vec<u8>),
    /// Ordered child objects.
    Array(Vec<PdfObject>),
    /// Entries indexed by decoded name bytes; the last duplicate key wins.
    Dictionary(Dictionary),
    /// An unresolved reference to an indirect object.
    Reference(ObjectReference),
}

impl PdfObject {
    /// The entries, when this is a dictionary.
    pub fn as_dictionary(&self) -> Option<&Dictionary> {
        match self {
            Self::Dictionary(entries) => Some(entries),
            _ => None,
        }
    }

    /// The elements, when this is an array.
    pub fn as_array(&self) -> Option<&[PdfObject]> {
        match self {
            Self::Array(values) => Some(values),
            _ => None,
        }
    }

    /// The decoded bytes, when this is a name.
    pub fn as_name(&self) -> Option<&[u8]> {
        match self {
            Self::Name(bytes) => Some(bytes),
            _ => None,
        }
    }

    /// The value, when this is an integer.
    pub fn as_integer(&self) -> Option<i64> {
        match self {
            Self::Integer(value) => Some(*value),
            _ => None,
        }
    }

    /// The value, when this is an integer or a real number.
    pub fn as_number(&self) -> Option<f64> {
        match self {
            Self::Integer(value) => Some(*value as f64),
            Self::Real(value) => Some(*value),
            _ => None,
        }
    }
}

/// An indirect reference, resolved by [`Document`](crate::Document).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjectReference {
    /// Nonzero object number, currently limited to unsigned 32 bits.
    pub number: u32,
    /// Generation number, from zero through 65535.
    pub generation: u16,
}

/// One object parsed from the start of a byte slice.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedObject {
    /// The decoded object.
    pub value: PdfObject,
    /// Bytes consumed, including leading whitespace/comments but no trailing trivia.
    pub consumed: usize,
}
