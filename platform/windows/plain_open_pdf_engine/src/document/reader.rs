//! Loading objects by number from file offsets and object streams.

use super::DocumentError;
use super::security::Decryptor;
use crate::filter::{
    FaxParameters, FilterError, Predictor, ascii_hex_decode, ascii85_decode, ccitt_decode,
    dct_decode, dct_decode_reduced, inflate_zlib, lzw_decode, run_length_decode, unpredict,
};
use crate::parser::{IndirectObject, parse_indirect_object};
use crate::{Dictionary, ParseError, ParseLimits, PdfObject, parse_object};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Largest decoded stream accepted, to bound memory on hostile input.
const MAX_DECODED_BYTES: usize = 256 * 1_048_576;
/// Deepest chain of objects that load other objects before giving up.
const MAX_LOAD_DEPTH: usize = 32;

/// Where the cross-reference data says an object is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Entry {
    /// An indirect object at this absolute file offset.
    Offset(usize),
    /// The object at `index` within the object stream numbered `stream`.
    Compressed { stream: u32, index: usize },
}

/// A decoded object stream with the number and offset of each object in it.
#[derive(Debug)]
struct ObjectStream {
    data: Vec<u8>,
    numbers: Vec<u32>,
    offsets: Vec<usize>,
}

/// An object as it is stored: its value and, for a stream, the stream's
/// bytes before any filter is reversed.
#[derive(Debug, Clone)]
pub(super) struct Stored {
    pub generation: u16,
    pub value: PdfObject,
    pub stream: Option<Vec<u8>>,
}

#[derive(Debug)]
pub(super) struct Reader {
    /// The file, shared with other documents opened from the same bytes.
    bytes: Arc<Vec<u8>>,
    base: usize,
    entries: BTreeMap<u32, Entry>,
    /// Objects set since the file was opened, which replace the file's.
    replaced: BTreeMap<u32, Stored>,
    object_streams: BTreeMap<u32, Arc<ObjectStream>>,
    decryptor: Option<Decryptor>,
    load_depth: usize,
}

impl Drop for Reader {
    /// The last document of a file frees the images kept from it.
    fn drop(&mut self) {
        if Arc::strong_count(&self.bytes) == 1 {
            super::image::forget_file(Arc::as_ptr(&self.bytes) as usize);
        }
    }
}

pub(super) fn entry<'a>(dictionary: &'a Dictionary, key: &str) -> Option<&'a PdfObject> {
    dictionary.get(key.as_bytes())
}

/// A non-negative integer entry of a dictionary.
pub(super) fn size_entry(dictionary: &Dictionary, key: &str) -> Option<usize> {
    usize::try_from(entry(dictionary, key)?.as_integer()?).ok()
}

impl Reader {
    /// `base` is the offset of the header, from which file offsets count.
    pub fn new(bytes: Arc<Vec<u8>>, base: usize) -> Self {
        Self {
            bytes,
            base,
            entries: BTreeMap::new(),
            replaced: BTreeMap::new(),
            object_streams: BTreeMap::new(),
            decryptor: None,
            load_depth: 0,
        }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The file's bytes, which other documents may share.
    pub fn file(&self) -> &Arc<Vec<u8>> {
        &self.bytes
    }

    /// Whether any object was set since the file was opened.
    pub fn is_modified(&self) -> bool {
        !self.replaced.is_empty()
    }

    /// Convert an offset stored in the file to a position in the bytes.
    pub fn position(&self, offset: usize) -> Option<usize> {
        self.base
            .checked_add(offset)
            .filter(|&position| position < self.bytes.len())
    }

    /// Record where an object is stored unless a newer section already did.
    pub fn add_entry(&mut self, number: u32, entry: Entry) {
        self.entries.entry(number).or_insert(entry);
    }

    pub fn has_decryptor(&self) -> bool {
        self.decryptor.is_some()
    }

    /// How the document is decrypted, when it is encrypted.
    pub fn decryptor(&self) -> Option<&Decryptor> {
        self.decryptor.as_ref()
    }

    /// Decrypt everything loaded from now on.
    pub fn set_decryptor(&mut self, decryptor: Decryptor) {
        self.decryptor = Some(decryptor);
    }

    /// Record where an object is stored, replacing what was known.
    pub fn set_entry(&mut self, number: u32, entry: Entry) {
        self.entries.insert(number, entry);
    }

    /// The numbers of the objects held in an object stream, in stream order.
    pub fn object_stream_numbers(&mut self, stream: u32) -> Result<Vec<u32>, DocumentError> {
        Ok(self.object_stream(stream)?.numbers.clone())
    }

    /// Parse the indirect object at a position, with an absolute stream start.
    pub fn indirect_at(&self, position: usize) -> Result<IndirectObject, DocumentError> {
        let bytes = self
            .bytes
            .get(position..)
            .ok_or(DocumentError::CrossReference { offset: position })?;
        let mut object = parse_indirect_object(bytes, ParseLimits::default()).map_err(|error| {
            DocumentError::Syntax(ParseError {
                offset: error.offset + position,
                kind: error.kind,
            })
        })?;
        object.stream_start = object.stream_start.map(|start| start + position);
        Ok(object)
    }

    /// Replace a reference with the object it names; other values are cloned.
    pub fn resolve(&mut self, object: &PdfObject) -> Result<PdfObject, DocumentError> {
        let mut current = object.clone();
        for _ in 0..MAX_LOAD_DEPTH {
            let PdfObject::Reference(reference) = current else {
                return Ok(current);
            };
            current = self.load(reference.number)?;
        }
        Err(DocumentError::Structure("references form a cycle"))
    }

    /// The resolved value of a dictionary entry, or null when it is absent.
    pub fn resolve_entry(
        &mut self,
        dictionary: &Dictionary,
        key: &str,
    ) -> Result<PdfObject, DocumentError> {
        match entry(dictionary, key) {
            Some(value) => self.resolve(value),
            None => Ok(PdfObject::Null),
        }
    }

    /// Load an object's value by number; undefined objects are null.
    fn load(&mut self, number: u32) -> Result<PdfObject, DocumentError> {
        if self.load_depth >= MAX_LOAD_DEPTH {
            return Err(DocumentError::Structure("objects depend on each other"));
        }
        self.load_depth += 1;
        let result = self.load_entry(number);
        self.load_depth -= 1;
        result
    }

    fn load_entry(&mut self, number: u32) -> Result<PdfObject, DocumentError> {
        if let Some(stored) = self.replaced.get(&number) {
            return Ok(stored.value.clone());
        }
        match self.entries.get(&number).copied() {
            None => Ok(PdfObject::Null),
            Some(Entry::Offset(position)) => {
                let mut object = self.indirect_at(position)?;
                // Strings inside an object stream are covered by the
                // stream's own encryption, so only these need decrypting.
                if let Some(decryptor) = &self.decryptor
                    && decryptor.dictionary_object != Some(object.number)
                {
                    decrypt_strings(&mut object.value, &|data| {
                        decryptor.decrypt_string(object.number, object.generation, data)
                    });
                }
                Ok(object.value)
            }
            Some(Entry::Compressed { stream, index }) => {
                let stream = self.object_stream(stream)?;
                let Some(data) = stream
                    .offsets
                    .get(index)
                    .and_then(|&offset| stream.data.get(offset..))
                else {
                    return Ok(PdfObject::Null);
                };
                parse_object(data, ParseLimits::default())
                    .map(|parsed| parsed.value)
                    .map_err(DocumentError::Syntax)
            }
        }
    }

    /// Decode the data of a stream object; `None` when it is not a stream.
    pub fn stream(&mut self, number: u32) -> Result<Option<Vec<u8>>, DocumentError> {
        self.stream_reduced(number, 1)
    }

    /// Decode a stream object as `stream` does, except that a last
    /// `DCTDecode` filter decodes its image at its size divided by `reduce`
    /// (see `dct_decode_reduced`).
    pub fn stream_reduced(
        &mut self,
        number: u32,
        reduce: usize,
    ) -> Result<Option<Vec<u8>>, DocumentError> {
        if let Some(stored) = self.replaced.get(&number) {
            let (Some(dictionary), Some(data)) = (stored.value.as_dictionary(), &stored.stream)
            else {
                return Ok(None);
            };
            let (dictionary, data) = (dictionary.clone(), data.clone());
            let filters = self.filters(&dictionary)?;
            return reverse_filters(data, &filters.0, &filters.1, 0, reduce).map(Some);
        }
        let Some(Entry::Offset(position)) = self.entries.get(&number).copied() else {
            return Ok(None);
        };
        let object = self.indirect_at(position)?;
        let (Some(dictionary), Some(start)) = (object.value.as_dictionary(), object.stream_start)
        else {
            return Ok(None);
        };
        let identity = Some((object.number, object.generation));
        self.stream_data_reduced(dictionary, start, identity, reduce)
            .map(Some)
    }

    /// The filters a stream dictionary names, and their parameters.
    fn filters(
        &mut self,
        dictionary: &Dictionary,
    ) -> Result<(Vec<Vec<u8>>, Vec<PdfObject>), DocumentError> {
        let filters = match self.resolve_entry(dictionary, "Filter")? {
            PdfObject::Name(name) => vec![name],
            PdfObject::Array(names) => names
                .iter()
                .map(|name| name.as_name().unwrap_or_default().to_vec())
                .collect(),
            _ => Vec::new(),
        };
        let parameters = match self.resolve_entry(dictionary, "DecodeParms")? {
            PdfObject::Array(values) => values,
            value => vec![value],
        };
        Ok((filters, parameters))
    }

    fn object_stream(&mut self, number: u32) -> Result<Arc<ObjectStream>, DocumentError> {
        if let Some(stream) = self.object_streams.get(&number) {
            return Ok(Arc::clone(stream));
        }
        const MISSING: DocumentError = DocumentError::Structure("an object stream is missing");
        let Some(Entry::Offset(position)) = self.entries.get(&number).copied() else {
            return Err(MISSING);
        };
        let object = self.indirect_at(position)?;
        let (Some(dictionary), Some(start)) = (object.value.as_dictionary(), object.stream_start)
        else {
            return Err(MISSING);
        };
        let identity = Some((object.number, object.generation));
        let data = self.stream_data(dictionary, start, identity)?;
        let first = size_entry(dictionary, "First").ok_or(MISSING)?;
        let count = size_entry(dictionary, "N").ok_or(MISSING)?;
        // The stream opens with `count` pairs: object number, then offset
        // relative to `first`.
        let header: Vec<usize> = data
            .get(..first)
            .ok_or(MISSING)?
            .split(u8::is_ascii_whitespace)
            .filter(|token| !token.is_empty())
            .map_while(|token| std::str::from_utf8(token).ok()?.parse().ok())
            .collect();
        let pairs = header.as_chunks::<2>().0.iter().take(count);
        let numbers = pairs
            .clone()
            .map(|pair| u32::try_from(pair[0]).unwrap_or(0))
            .collect();
        let offsets = pairs.map(|pair| pair[1].saturating_add(first)).collect();
        let stream = Arc::new(ObjectStream {
            data,
            numbers,
            offsets,
        });
        self.object_streams.insert(number, Arc::clone(&stream));
        Ok(stream)
    }

    /// Decode the data of the stream that starts at an absolute position.
    ///
    /// `identity` is the stream's object and generation number, which key
    /// its decryption; `None` reads a stream that is never encrypted.
    pub fn stream_data(
        &mut self,
        dictionary: &Dictionary,
        start: usize,
        identity: Option<(u32, u32)>,
    ) -> Result<Vec<u8>, DocumentError> {
        self.stream_data_reduced(dictionary, start, identity, 1)
    }

    fn stream_data_reduced(
        &mut self,
        dictionary: &Dictionary,
        start: usize,
        identity: Option<(u32, u32)>,
        reduce: usize,
    ) -> Result<Vec<u8>, DocumentError> {
        let length = self.resolve_entry(dictionary, "Length")?;
        let length = length.as_integer().and_then(|v| usize::try_from(v).ok());
        let (mut filters, mut parameters) = self.filters(dictionary)?;
        let mut data = raw_stream(&self.bytes, start, length).to_vec();
        // A stream that names its own crypt filter opts out of the
        // document's default; in practice that filter is the identity.
        let own_crypt_filter = filters.iter().position(|filter| filter == b"Crypt");
        if let Some(index) = own_crypt_filter {
            filters.remove(index);
            if index < parameters.len() {
                parameters.remove(index);
            }
        }
        let kind = entry(dictionary, "Type").and_then(PdfObject::as_name);
        if let Some(decryptor) = &self.decryptor
            && let Some((number, generation)) = identity
            && own_crypt_filter.is_none()
            && kind != Some(b"XRef")
            && (kind != Some(b"Metadata") || decryptor.encrypt_metadata)
        {
            data = decryptor.decrypt_stream(number, generation, &data);
        }
        reverse_filters(data, &filters, &parameters, start, reduce)
    }

    /// Reverse the filters a stream dictionary names on data that is not
    /// encrypted. `start` locates the stream in error reports.
    pub fn decode_stream(
        &mut self,
        dictionary: &Dictionary,
        data: Vec<u8>,
        start: usize,
    ) -> Result<Vec<u8>, DocumentError> {
        let (filters, parameters) = self.filters(dictionary)?;
        reverse_filters(data, &filters, &parameters, start, 1)
    }

    /// The numbers of every object the document defines, in order.
    pub fn object_numbers(&self) -> Vec<u32> {
        let mut numbers: Vec<u32> = self.entries.keys().copied().collect();
        numbers.extend(self.replaced.keys());
        numbers.sort_unstable();
        numbers.dedup();
        numbers
    }

    /// Define or replace an object. A stream's bytes are stored as given,
    /// so its dictionary must name the filters they are encoded with.
    pub fn set_object(&mut self, number: u32, value: PdfObject, stream: Option<Vec<u8>>) {
        let generation = match self.replaced.get(&number) {
            Some(stored) => stored.generation,
            None => match self.entries.get(&number) {
                Some(Entry::Offset(position)) => self
                    .indirect_at(*position)
                    .map_or(0, |object| u16::try_from(object.generation).unwrap_or(0)),
                _ => 0,
            },
        };
        self.replaced.insert(
            number,
            Stored {
                generation,
                value,
                stream,
            },
        );
    }

    /// An object as stored, for writing it out again. Object streams and
    /// cross-reference streams, which a rewritten file does not keep, and
    /// undefined objects yield `None`.
    pub fn stored(&mut self, number: u32) -> Result<Option<Stored>, DocumentError> {
        if let Some(stored) = self.replaced.get(&number) {
            return Ok(Some(stored.clone()));
        }
        match self.entries.get(&number).copied() {
            None => Ok(None),
            Some(Entry::Compressed { .. }) => Ok(Some(Stored {
                generation: 0,
                value: self.load(number)?,
                stream: None,
            })),
            Some(Entry::Offset(position)) => {
                let object = self.indirect_at(position)?;
                let generation = u16::try_from(object.generation).unwrap_or(0);
                let kind = object
                    .value
                    .as_dictionary()
                    .and_then(|dictionary| entry(dictionary, "Type"))
                    .and_then(PdfObject::as_name);
                let stream = match (object.value.as_dictionary(), object.stream_start) {
                    (Some(_), Some(_)) if matches!(kind, Some(b"ObjStm" | b"XRef")) => {
                        return Ok(None);
                    }
                    (Some(dictionary), Some(start)) => {
                        let length = self.resolve_entry(dictionary, "Length")?;
                        let length = length.as_integer().and_then(|v| usize::try_from(v).ok());
                        let mut data = raw_stream(&self.bytes, start, length).to_vec();
                        // The same streams are decrypted as when decoding.
                        let own_crypt_filter = match self.resolve_entry(dictionary, "Filter")? {
                            PdfObject::Name(name) => name == b"Crypt",
                            PdfObject::Array(names) => {
                                names.iter().any(|name| name.as_name() == Some(b"Crypt"))
                            }
                            _ => false,
                        };
                        if let Some(decryptor) = &self.decryptor
                            && !own_crypt_filter
                            && (kind != Some(b"Metadata") || decryptor.encrypt_metadata)
                        {
                            data =
                                decryptor.decrypt_stream(object.number, object.generation, &data);
                        }
                        Some(data)
                    }
                    _ => None,
                };
                let mut value = object.value;
                if let Some(decryptor) = &self.decryptor
                    && decryptor.dictionary_object != Some(object.number)
                {
                    decrypt_strings(&mut value, &|data| {
                        decryptor.decrypt_string(object.number, object.generation, data)
                    });
                }
                Ok(Some(Stored {
                    generation,
                    value,
                    stream,
                }))
            }
        }
    }
}

/// Reverse a stream's filters in order, each with its decode parameters.
fn reverse_filters(
    mut data: Vec<u8>,
    filters: &[Vec<u8>],
    parameters: &[PdfObject],
    start: usize,
    reduce: usize,
) -> Result<Vec<u8>, DocumentError> {
    for (index, filter) in filters.iter().enumerate() {
        let parameters = parameters.get(index).and_then(PdfObject::as_dictionary);
        let last_dct =
            index + 1 == filters.len() && matches!(filter.as_slice(), b"DCTDecode" | b"DCT");
        let decoded = if last_dct && reduce > 1 {
            dct_decode_reduced(&data, MAX_DECODED_BYTES, reduce)
        } else {
            decode(data, filter, parameters)
        };
        data = decoded.map_err(|error| match error {
            FilterError::Unsupported => DocumentError::Unsupported("a stream filter or predictor"),
            FilterError::Corrupt | FilterError::LimitExceeded => {
                DocumentError::Stream { offset: start }
            }
        })?;
    }
    Ok(data)
}

/// Replace every string in an object, at any depth, with its decryption.
fn decrypt_strings(object: &mut PdfObject, decrypt: &dyn Fn(&[u8]) -> Vec<u8>) {
    match object {
        PdfObject::String(bytes) => *bytes = decrypt(bytes),
        PdfObject::Array(values) => {
            for value in values {
                decrypt_strings(value, decrypt);
            }
        }
        PdfObject::Dictionary(entries) => {
            for value in entries.values_mut() {
                decrypt_strings(value, decrypt);
            }
        }
        _ => {}
    }
}

/// The stored bytes of a stream, before any filter is reversed.
///
/// `/Length` is trusted only when `endstream` follows the data it describes;
/// otherwise the data runs to the next `endstream` keyword.
fn raw_stream(bytes: &[u8], start: usize, length: Option<usize>) -> &[u8] {
    const KEYWORD: &[u8] = b"endstream";
    if let Some(end) = length.and_then(|length| start.checked_add(length))
        && let Some(rest) = bytes.get(end..)
        && rest.trim_ascii_start().starts_with(KEYWORD)
    {
        return &bytes[start..end];
    }
    let tail = bytes.get(start..).unwrap_or_default();
    let end = tail
        .windows(KEYWORD.len())
        .position(|window| window == KEYWORD)
        .unwrap_or(tail.len());
    let data = &tail[..end];
    // One end-of-line marker separates the data from the keyword.
    let data = data.strip_suffix(b"\n").unwrap_or(data);
    data.strip_suffix(b"\r").unwrap_or(data)
}

/// Reverse one named filter, with its decode parameters if it has any.
fn decode(
    data: Vec<u8>,
    filter: &[u8],
    parameters: Option<&Dictionary>,
) -> Result<Vec<u8>, FilterError> {
    let integer = |key| {
        let value = entry(parameters?, key)?;
        value.as_integer()
    };
    let size = |key, default| {
        let value = integer(key).and_then(|value| usize::try_from(value).ok());
        value.unwrap_or(default)
    };
    let predictor = Predictor {
        kind: integer("Predictor").unwrap_or(1),
        columns: size("Columns", 1),
        colors: size("Colors", 1),
        bits_per_component: size("BitsPerComponent", 8),
    };
    match filter {
        b"FlateDecode" | b"Fl" => unpredict(inflate_zlib(&data, MAX_DECODED_BYTES)?, predictor),
        b"LZWDecode" | b"LZW" => {
            let early_change = integer("EarlyChange") != Some(0);
            unpredict(
                lzw_decode(&data, early_change, MAX_DECODED_BYTES)?,
                predictor,
            )
        }
        b"ASCIIHexDecode" | b"AHx" => ascii_hex_decode(&data),
        b"ASCII85Decode" | b"A85" => ascii85_decode(&data),
        b"RunLengthDecode" | b"RL" => run_length_decode(&data, MAX_DECODED_BYTES),
        b"DCTDecode" | b"DCT" => dct_decode(&data, MAX_DECODED_BYTES),
        b"CCITTFaxDecode" | b"CCF" => {
            let flag = |key: &str, default: bool| match parameters.and_then(|p| entry(p, key)) {
                Some(PdfObject::Boolean(value)) => *value,
                _ => default,
            };
            let defaults = FaxParameters::default();
            let fax = FaxParameters {
                k: integer("K").unwrap_or(0),
                byte_align: flag("EncodedByteAlign", false),
                columns: size("Columns", defaults.columns),
                rows: size("Rows", 0),
                end_of_block: flag("EndOfBlock", true),
                black_is_1: flag("BlackIs1", false),
            };
            ccitt_decode(&data, fax, MAX_DECODED_BYTES)
        }
        _ => Err(FilterError::Unsupported),
    }
}
