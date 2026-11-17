//! Cross-reference tables and streams: where each numbered object is stored.

use super::DocumentError;
use super::reader::{Entry, Reader, entry, size_entry};
use super::security;
use crate::{Dictionary, ObjectReference, ParseError, ParseLimits, PdfObject, parse_object};
use std::collections::{BTreeMap, BTreeSet};

/// How much of the end of the file is searched for `startxref`.
const STARTXREF_SEARCH_BYTES: usize = 2048;

/// What the file's own cross-reference data yielded.
pub(super) struct Loaded {
    /// The newest trailer, when at least one section could be read.
    pub trailer: Option<Dictionary>,
    /// Whether every section in the chain was found and read.
    pub intact: bool,
}

/// Read every cross-reference section, newest first.
///
/// The first location recorded for an object is the current one. Free
/// entries are not recorded: a hybrid file marks objects free in its table
/// and locates them in a stream instead.
pub(super) fn load(reader: &mut Reader) -> Loaded {
    let mut loaded = Loaded {
        trailer: None,
        intact: true,
    };
    let Ok(start) = start_offset(reader.bytes()) else {
        loaded.intact = false;
        return loaded;
    };
    let mut pending = vec![start];
    let mut visited = BTreeSet::new();
    while let Some(offset) = pending.pop() {
        let section = match reader.position(offset) {
            Some(position) if !visited.insert(position) => continue,
            Some(position) => read_section(reader, position),
            None => Err(DocumentError::CrossReference { offset }),
        };
        let Ok(section) = section else {
            loaded.intact = false;
            continue;
        };
        // The stack is read last-in first: a hybrid stream before older data.
        pending.extend(size_entry(&section, "Prev"));
        pending.extend(size_entry(&section, "XRefStm"));
        loaded.trailer.get_or_insert(section);
    }
    loaded
}

/// Rebuild object locations by scanning the file for object definitions.
///
/// Used when the cross-reference data is missing, damaged, or does not lead
/// to any pages. A scanned definition replaces what the file claimed, and
/// where an object is defined more than once the last definition wins, as it
/// does for incremental updates. Returns the best trailer available.
///
/// # Errors
///
/// Fails only when the trailer found names encryption that cannot be opened.
pub(super) fn repair(
    reader: &mut Reader,
    trailer: Option<Dictionary>,
    password: &[u8],
) -> Result<Option<Dictionary>, DocumentError> {
    let found = scan_definitions(reader.bytes());
    for (&number, &position) in &found {
        reader.set_entry(number, Entry::Offset(position));
    }
    let has_root = |trailer: &Dictionary| trailer.contains_key(b"Root".as_slice());
    let type_of = |object: &PdfObject| {
        let dictionary = object.as_dictionary()?;
        entry(dictionary, "Type")?.as_name().map(<[u8]>::to_vec)
    };
    let mut catalog = None;
    let mut stream_trailer = None;
    let mut object_streams = Vec::new();
    for (&number, &position) in &found {
        let Ok(object) = reader.indirect_at(position) else {
            continue;
        };
        match type_of(&object.value).as_deref() {
            Some(b"Catalog") => catalog = Some(number),
            // A cross-reference stream's dictionary is also a trailer.
            Some(b"XRef") => {
                if let PdfObject::Dictionary(dictionary) = object.value
                    && has_root(&dictionary)
                {
                    stream_trailer = Some(dictionary);
                }
            }
            Some(b"ObjStm") => object_streams.push(number),
            _ => {}
        }
    }
    let trailer = trailer
        .filter(has_root)
        .or_else(|| last_trailer(reader.bytes()))
        .or(stream_trailer);
    // Object streams are encrypted, so the key must be known before the
    // objects inside them can be listed.
    if let Some(trailer) = &trailer {
        security::install(reader, trailer, password)?;
    }
    let mut compressed = Vec::new();
    for stream in object_streams {
        let numbers = reader.object_stream_numbers(stream).unwrap_or_default();
        for (index, inner) in numbers.into_iter().enumerate() {
            if !found.contains_key(&inner) {
                reader.add_entry(inner, Entry::Compressed { stream, index });
                compressed.push(inner);
            }
        }
    }
    if trailer.is_some() {
        return Ok(trailer);
    }
    // With no trailer anywhere, the catalog itself identifies the document;
    // it may be stored in an object stream.
    let catalog = catalog.or_else(|| {
        compressed.into_iter().rev().find(|&number| {
            let reference = PdfObject::Reference(ObjectReference {
                number,
                generation: 0,
            });
            let object = reader.resolve(&reference).ok();
            object.and_then(|object| type_of(&object)).as_deref() == Some(b"Catalog")
        })
    });
    Ok(catalog.map(|number| {
        let reference = PdfObject::Reference(ObjectReference {
            number,
            generation: 0,
        });
        Dictionary::from([(b"Root".to_vec(), reference)])
    }))
}

/// The position of every `number generation obj` in the file, by number.
fn scan_definitions(bytes: &[u8]) -> BTreeMap<u32, usize> {
    const KEYWORD: &[u8] = b"obj";
    fn trailing_digits(bytes: &[u8]) -> usize {
        let digits = bytes.iter().rev().take_while(|byte| byte.is_ascii_digit());
        digits.count()
    }
    let mut found = BTreeMap::new();
    let mut searched = 0;
    while let Some(index) = bytes[searched..]
        .windows(KEYWORD.len())
        .position(|window| window == KEYWORD)
    {
        let keyword = searched + index;
        searched = keyword + KEYWORD.len();
        if bytes.get(searched).is_some_and(u8::is_ascii_alphanumeric) {
            continue;
        }
        // Walk back over the generation number, then the object number.
        let head = bytes[..keyword].trim_ascii_end();
        let generation_digits = trailing_digits(head);
        let head = &head[..head.len() - generation_digits];
        let before_generation = head.trim_ascii_end();
        let number_digits = trailing_digits(before_generation);
        if generation_digits == 0 || number_digits == 0 || before_generation.len() == head.len() {
            continue;
        }
        let start = before_generation.len() - number_digits;
        let number = std::str::from_utf8(&before_generation[start..])
            .ok()
            .and_then(|text| text.parse().ok());
        if let Some(number) = number {
            found.insert(number, start);
        }
    }
    found
}

/// The last `trailer` dictionary in the file that names a catalog.
fn last_trailer(bytes: &[u8]) -> Option<Dictionary> {
    const KEYWORD: &[u8] = b"trailer";
    let mut end = bytes.len();
    while let Some(keyword) = bytes[..end]
        .windows(KEYWORD.len())
        .rposition(|window| window == KEYWORD)
    {
        end = keyword;
        let parsed = parse_object(&bytes[keyword + KEYWORD.len()..], ParseLimits::default());
        if let Ok(parsed) = parsed
            && let PdfObject::Dictionary(trailer) = parsed.value
            && trailer.contains_key(b"Root".as_slice())
        {
            return Some(trailer);
        }
    }
    None
}

/// The offset that follows the last `startxref` keyword.
fn start_offset(bytes: &[u8]) -> Result<usize, DocumentError> {
    const KEYWORD: &[u8] = b"startxref";
    let error = DocumentError::CrossReference {
        offset: bytes.len(),
    };
    let search_start = bytes.len().saturating_sub(STARTXREF_SEARCH_BYTES);
    let keyword = bytes[search_start..]
        .windows(KEYWORD.len())
        .rposition(|window| window == KEYWORD)
        .ok_or(error)?;
    let mut scanner = Scanner {
        bytes,
        position: search_start + keyword + KEYWORD.len(),
    };
    scanner.integer().ok_or(error)
}

fn read_section(reader: &mut Reader, position: usize) -> Result<Dictionary, DocumentError> {
    let mut scanner = Scanner {
        bytes: reader.bytes(),
        position,
    };
    scanner.skip_whitespace();
    if scanner.rest().starts_with(b"xref") {
        scanner.position += 4;
        let (entries, trailer) = read_table(scanner, reader)?;
        for (number, entry) in entries {
            reader.add_entry(number, entry);
        }
        Ok(trailer)
    } else {
        read_stream(reader, position)
    }
}

/// Read the subsections that follow `xref`, then the trailer dictionary.
fn read_table(
    mut scanner: Scanner<'_>,
    reader: &Reader,
) -> Result<(Vec<(u32, Entry)>, Dictionary), DocumentError> {
    let mut entries = Vec::new();
    loop {
        let error = DocumentError::CrossReference {
            offset: scanner.position,
        };
        scanner.skip_whitespace();
        if scanner.rest().starts_with(b"trailer") {
            scanner.position += 7;
            break;
        }
        let first = scanner.integer().ok_or(error)?;
        let count = scanner.integer().ok_or(error)?;
        for index in 0..count {
            let offset = scanner.integer().ok_or(error)?;
            scanner.integer().ok_or(error)?;
            let in_use = match scanner.token() {
                b"n" => true,
                b"f" => false,
                _ => return Err(error),
            };
            let number = first
                .checked_add(index)
                .and_then(|number| u32::try_from(number).ok());
            // An in-use entry at offset zero is a common way to write "free".
            if in_use
                && offset != 0
                && let Some(number) = number
                && let Some(position) = reader.position(offset)
            {
                entries.push((number, Entry::Offset(position)));
            }
        }
    }
    let trailer = parse_object(scanner.rest(), ParseLimits::default()).map_err(|error| {
        DocumentError::Syntax(ParseError {
            offset: error.offset + scanner.position,
            kind: error.kind,
        })
    })?;
    match trailer.value {
        PdfObject::Dictionary(trailer) => Ok((entries, trailer)),
        _ => Err(DocumentError::CrossReference {
            offset: scanner.position,
        }),
    }
}

/// Read a cross-reference stream; its dictionary doubles as the trailer.
fn read_stream(reader: &mut Reader, position: usize) -> Result<Dictionary, DocumentError> {
    let error = DocumentError::CrossReference { offset: position };
    let object = reader.indirect_at(position).map_err(|_| error)?;
    let (PdfObject::Dictionary(dictionary), Some(start)) = (object.value, object.stream_start)
    else {
        return Err(error);
    };
    let widths: Vec<usize> = entry(&dictionary, "W")
        .and_then(PdfObject::as_array)
        .ok_or(error)?
        .iter()
        .map(|width| width.as_integer().and_then(|w| usize::try_from(w).ok()))
        .collect::<Option<_>>()
        .ok_or(error)?;
    let &[kind_width, second_width, third_width] = widths.as_slice() else {
        return Err(error);
    };
    let row_width = kind_width + second_width + third_width;
    if widths.iter().any(|&width| width > 8) || row_width == 0 {
        return Err(error);
    }
    // `/Index` lists (first object number, count) pairs; one pair by default.
    let index: Vec<usize> = match entry(&dictionary, "Index").and_then(PdfObject::as_array) {
        Some(values) => values
            .iter()
            .filter_map(|value| value.as_integer().and_then(|v| usize::try_from(v).ok()))
            .collect(),
        None => vec![0, size_entry(&dictionary, "Size").ok_or(error)?],
    };
    let data = reader.stream_data(&dictionary, start, None)?;
    let mut rows = data.chunks_exact(row_width);
    'sections: for &[first, count] in index.as_chunks::<2>().0 {
        for offset in 0..count {
            let Some(row) = rows.next() else {
                break 'sections;
            };
            let (kind, row) = row.split_at(kind_width);
            let (second, third) = row.split_at(second_width);
            let number = first
                .checked_add(offset)
                .and_then(|number| u32::try_from(number).ok());
            // A missing type field means an ordinary in-use object.
            let kind = if kind_width == 0 { 1 } else { field(kind) };
            let entry = match kind {
                1 => reader.position(field(second)).map(Entry::Offset),
                2 => u32::try_from(field(second))
                    .ok()
                    .map(|stream| Entry::Compressed {
                        stream,
                        index: field(third),
                    }),
                _ => None,
            };
            if let (Some(number), Some(entry)) = (number, entry) {
                reader.add_entry(number, entry);
            }
        }
    }
    Ok(dictionary)
}

/// A big-endian unsigned field of at most eight bytes.
fn field(bytes: &[u8]) -> usize {
    let value = bytes
        .iter()
        .fold(0u64, |value, &byte| (value << 8) | u64::from(byte));
    usize::try_from(value).unwrap_or(usize::MAX)
}

/// Whitespace-separated tokens, as cross-reference tables are written.
struct Scanner<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Scanner<'a> {
    fn rest(&self) -> &'a [u8] {
        self.bytes.get(self.position..).unwrap_or_default()
    }

    fn skip_whitespace(&mut self) {
        let rest = self.rest();
        self.position += rest.len() - rest.trim_ascii_start().len();
    }

    fn token(&mut self) -> &'a [u8] {
        self.skip_whitespace();
        let rest = self.rest();
        let length = rest
            .iter()
            .position(u8::is_ascii_whitespace)
            .unwrap_or(rest.len());
        self.position += length;
        &rest[..length]
    }

    fn integer(&mut self) -> Option<usize> {
        let start = self.position;
        let value = std::str::from_utf8(self.token())
            .ok()
            .and_then(|text| text.parse().ok());
        if value.is_none() {
            self.position = start;
        }
        value
    }
}
