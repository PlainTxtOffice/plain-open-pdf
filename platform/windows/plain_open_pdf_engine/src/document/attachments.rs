//! Files embedded in the document (ISO 32000-2 sections 7.11.4 and 7.9.6).

use super::DocumentError;
use super::destination::name_tree_entries;
use super::links::file_name;
use super::reader::{Reader, entry};
use super::text::{decode_text_string, encode_text_string};
use crate::{Dictionary, ObjectReference, PdfObject};

/// A file embedded in the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    /// The key the document lists the file under.
    pub name: String,
    /// The file's own name, when its specification gives one.
    pub file_name: Option<String>,
    /// A description of the file.
    pub description: Option<String>,
    /// The stream that holds the file's bytes; read it with
    /// [`Document::stream`](super::Document::stream).
    pub stream: Option<ObjectReference>,
}

/// List the files in the catalog's `/EmbeddedFiles` name tree.
pub(super) fn read(
    reader: &mut Reader,
    trailer: &Dictionary,
) -> Result<Vec<Attachment>, DocumentError> {
    let PdfObject::Dictionary(catalog) = reader.resolve_entry(trailer, "Root")? else {
        return Ok(Vec::new());
    };
    let PdfObject::Dictionary(names) = reader.resolve_entry(&catalog, "Names")? else {
        return Ok(Vec::new());
    };
    let Some(tree) = entry(&names, "EmbeddedFiles").cloned() else {
        return Ok(Vec::new());
    };
    let mut attachments = Vec::new();
    for (name, specification) in name_tree_entries(reader, &tree) {
        let Ok(PdfObject::Dictionary(specification)) = reader.resolve(&specification) else {
            continue;
        };
        let description = match reader.resolve_entry(&specification, "Desc") {
            Ok(PdfObject::String(text)) => Some(decode_text_string(&text)),
            _ => None,
        };
        // `/EF` maps the same keys as the names to embedded file streams.
        let stream = match reader.resolve_entry(&specification, "EF") {
            Ok(PdfObject::Dictionary(files)) => {
                ["UF", "F"]
                    .into_iter()
                    .find_map(|key| match entry(&files, key) {
                        Some(PdfObject::Reference(stream)) => Some(*stream),
                        _ => None,
                    })
            }
            _ => None,
        };
        attachments.push(Attachment {
            name: decode_text_string(&name),
            file_name: file_name(reader, &specification),
            description,
            stream,
        });
    }
    Ok(attachments)
}

impl super::Document {
    /// The catalog's reference and dictionary, for changing it.
    fn catalog_for_change(&mut self) -> Result<(ObjectReference, Dictionary), DocumentError> {
        const CATALOG: DocumentError =
            DocumentError::Structure("the catalog is not an indirect object");
        let Some(&PdfObject::Reference(reference)) = entry(&self.trailer, "Root") else {
            return Err(CATALOG);
        };
        match self.object(reference)? {
            PdfObject::Dictionary(catalog) => Ok((reference, catalog)),
            _ => Err(CATALOG),
        }
    }

    /// The bytes of the embedded file listed under a name, decoded, or
    /// `None` when there is no such file.
    ///
    /// # Errors
    ///
    /// Returns a [`DocumentError`] when the catalog or the file's stream
    /// cannot be read.
    pub fn attachment_data(&mut self, name: &str) -> Result<Option<Vec<u8>>, DocumentError> {
        let found = self
            .attachments()?
            .into_iter()
            .find(|file| file.name == name);
        match found.and_then(|file| file.stream) {
            Some(stream) => self.stream(stream),
            None => Ok(None),
        }
    }

    /// Embed a file under a name, replacing any file listed under it.
    ///
    /// The data is compressed with `FlateDecode`. `file_name` is the name
    /// the file is saved under, the listed name when `None`. The embedded
    /// files name tree is written again as one node, sorted by name.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentError::Structure`] when the catalog is not an
    /// indirect object, and a [`DocumentError`] when it cannot be parsed.
    pub fn add_attachment(
        &mut self,
        name: &str,
        data: &[u8],
        file_name: Option<&str>,
        description: Option<&str>,
    ) -> Result<(), DocumentError> {
        let named = |text: &str| PdfObject::Name(text.as_bytes().to_vec());
        let mut parameters = Dictionary::new();
        let size = i64::try_from(data.len()).unwrap_or(i64::MAX);
        parameters.insert(b"Size".to_vec(), PdfObject::Integer(size));
        let mut stream_entries = Dictionary::new();
        stream_entries.insert(b"Type".to_vec(), named("EmbeddedFile"));
        stream_entries.insert(b"Params".to_vec(), PdfObject::Dictionary(parameters));
        let stream = self.add_object(PdfObject::Null);
        self.set_stream_data(stream, stream_entries, data);

        let file_name = PdfObject::String(encode_text_string(file_name.unwrap_or(name)));
        let mut files = Dictionary::new();
        files.insert(b"F".to_vec(), PdfObject::Reference(stream));
        files.insert(b"UF".to_vec(), PdfObject::Reference(stream));
        let mut specification = Dictionary::new();
        specification.insert(b"Type".to_vec(), named("Filespec"));
        specification.insert(b"F".to_vec(), file_name.clone());
        specification.insert(b"UF".to_vec(), file_name);
        if let Some(description) = description {
            let description = PdfObject::String(encode_text_string(description));
            specification.insert(b"Desc".to_vec(), description);
        }
        specification.insert(b"EF".to_vec(), PdfObject::Dictionary(files));
        let specification = self.add_object(PdfObject::Dictionary(specification));

        self.change_embedded_files(|entries| {
            entries.retain(|(key, _)| decode_text_string(key) != name);
            entries.push((
                encode_text_string(name),
                PdfObject::Reference(specification),
            ));
        })
    }

    /// Remove the embedded file listed under a name, and return whether
    /// there was one. Saving leaves its data out of the file.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentError::Structure`] when the catalog is not an
    /// indirect object, and a [`DocumentError`] when it cannot be parsed.
    pub fn remove_attachment(&mut self, name: &str) -> Result<bool, DocumentError> {
        if !self.attachments()?.iter().any(|file| file.name == name) {
            return Ok(false);
        }
        self.change_embedded_files(|entries| {
            entries.retain(|(key, _)| decode_text_string(key) != name);
        })?;
        Ok(true)
    }

    /// Change the entries of the embedded files name tree and write it
    /// again as one sorted node, or remove it when no entries are left.
    fn change_embedded_files(
        &mut self,
        change: impl FnOnce(&mut Vec<(Vec<u8>, PdfObject)>),
    ) -> Result<(), DocumentError> {
        let (catalog_reference, mut catalog) = self.catalog_for_change()?;
        let names_entry = entry(&catalog, "Names").cloned();
        let mut names = match self.reader.resolve_entry(&catalog, "Names")? {
            PdfObject::Dictionary(names) => names,
            _ => Dictionary::new(),
        };
        let mut entries = match entry(&names, "EmbeddedFiles").cloned() {
            Some(tree) => name_tree_entries(&mut self.reader, &tree),
            None => Vec::new(),
        };
        change(&mut entries);
        // Keys are sorted by their bytes, and a key listed twice keeps its
        // last value.
        entries.reverse();
        entries.sort_by(|first, second| first.0.cmp(&second.0));
        entries.dedup_by(|later, earlier| later.0 == earlier.0);
        if entries.is_empty() {
            names.remove(b"EmbeddedFiles".as_slice());
        } else {
            let pairs = entries
                .into_iter()
                .flat_map(|(key, value)| [PdfObject::String(key), value])
                .collect();
            let mut node = Dictionary::new();
            node.insert(b"Names".to_vec(), PdfObject::Array(pairs));
            let node = self.add_object(PdfObject::Dictionary(node));
            names.insert(b"EmbeddedFiles".to_vec(), PdfObject::Reference(node));
        }
        match names_entry {
            Some(PdfObject::Reference(reference)) => {
                self.set_object(reference, PdfObject::Dictionary(names));
            }
            _ => {
                if names.is_empty() {
                    catalog.remove(b"Names".as_slice());
                } else {
                    catalog.insert(b"Names".to_vec(), PdfObject::Dictionary(names));
                }
                self.set_object(catalog_reference, PdfObject::Dictionary(catalog));
            }
        }
        Ok(())
    }

    /// The document's XMP metadata, the catalog's `/Metadata` stream,
    /// decoded; `None` when it has none.
    ///
    /// # Errors
    ///
    /// Returns a [`DocumentError`] when the catalog or stream cannot be
    /// read.
    pub fn xmp_metadata(&mut self) -> Result<Option<Vec<u8>>, DocumentError> {
        let PdfObject::Dictionary(catalog) = self.reader.resolve_entry(&self.trailer, "Root")?
        else {
            return Ok(None);
        };
        match entry(&catalog, "Metadata") {
            Some(&PdfObject::Reference(stream)) => self.stream(stream),
            _ => Ok(None),
        }
    }

    /// Set the document's XMP metadata, stored uncompressed as XMP readers
    /// expect, or remove it with `None`. The data is not checked.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentError::Structure`] when the catalog is not an
    /// indirect object, and a [`DocumentError`] when it cannot be parsed.
    pub fn set_xmp_metadata(&mut self, data: Option<&[u8]>) -> Result<(), DocumentError> {
        let (reference, mut catalog) = self.catalog_for_change()?;
        match data {
            None => {
                catalog.remove(b"Metadata".as_slice());
            }
            Some(data) => {
                let named = |text: &str| PdfObject::Name(text.as_bytes().to_vec());
                let mut entries = Dictionary::new();
                entries.insert(b"Type".to_vec(), named("Metadata"));
                entries.insert(b"Subtype".to_vec(), named("XML"));
                let stream = self.add_object(PdfObject::Null);
                self.set_stream(stream, entries, data.to_vec());
                catalog.insert(b"Metadata".to_vec(), PdfObject::Reference(stream));
            }
        }
        self.set_object(reference, PdfObject::Dictionary(catalog));
        Ok(())
    }
}
