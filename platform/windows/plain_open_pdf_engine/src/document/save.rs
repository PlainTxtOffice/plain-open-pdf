//! Writing a document out as a new file (ISO 32000-2 section 7.5).

use super::DocumentError;
use super::reader::{Reader, Stored, entry};
use super::security::Decryptor;
use crate::crypto::random_bytes;
use crate::writer::write_object;
use crate::{Dictionary, PdfObject, PdfVersion};
use std::collections::BTreeMap;

/// How a saved file is encrypted.
pub(super) struct Encryption {
    /// Encrypts each object's strings and streams.
    pub cipher: Decryptor,
    /// The encryption dictionary for new protection; `None` keeps the
    /// document's own, which the cipher came from.
    pub dictionary: Option<Dictionary>,
}

/// Apply a change to every string inside a value.
fn map_strings(object: &mut PdfObject, change: &dyn Fn(&[u8]) -> Vec<u8>) {
    match object {
        PdfObject::String(bytes) => *bytes = change(bytes),
        PdfObject::Array(values) => values
            .iter_mut()
            .for_each(|value| map_strings(value, change)),
        PdfObject::Dictionary(entries) => {
            entries
                .values_mut()
                .for_each(|value| map_strings(value, change));
        }
        _ => {}
    }
}

/// Collect the numbers of the objects a value refers to.
fn references(object: &PdfObject, found: &mut Vec<u32>) {
    match object {
        PdfObject::Reference(reference) => found.push(reference.number),
        PdfObject::Array(values) => values.iter().for_each(|value| references(value, found)),
        PdfObject::Dictionary(entries) => {
            entries.values().for_each(|value| references(value, found));
        }
        _ => {}
    }
}

/// Write the document as a complete file: every object that can be reached
/// from the trailer, uncompressed, with one cross-reference table.
///
/// Stream data is copied as stored, with its filters. Objects nothing
/// refers to any more are left out, as are objects that cannot be read.
/// Without `encryption`, an encrypted document is written decrypted: the
/// trailer's `/Encrypt` entry is not kept. With it, every string and stream
/// is encrypted, except those of the encryption dictionary and an XMP
/// stream the encryption leaves plain.
pub(super) fn write(
    reader: &mut Reader,
    mut version: PdfVersion,
    trailer: &Dictionary,
    encryption: Option<Encryption>,
) -> Result<Vec<u8>, DocumentError> {
    let mut kept = Dictionary::new();
    let keep_encryption = encryption.as_ref().is_some_and(|e| e.dictionary.is_none());
    for key in ["Root", "Info", "ID", "Encrypt"] {
        if key == "Encrypt" && !keep_encryption {
            continue;
        }
        if let Some(value) = entry(trailer, key) {
            kept.insert(key.as_bytes().to_vec(), value.clone());
        }
    }
    // An encrypted file needs an identifier; revision 6 does not derive its
    // keys from it, so a new one can be made.
    if encryption.is_some() && !kept.contains_key(b"ID".as_slice()) {
        let id = PdfObject::String(random_bytes(16));
        kept.insert(b"ID".to_vec(), PdfObject::Array(vec![id.clone(), id]));
    }

    let mut objects: BTreeMap<u32, Option<Stored>> = BTreeMap::new();
    let mut pending = Vec::new();
    references(&PdfObject::Dictionary(kept.clone()), &mut pending);
    while let Some(number) = pending.pop() {
        if objects.contains_key(&number) {
            continue;
        }
        let stored = reader.stored(number).ok().flatten();
        if let Some(stored) = &stored {
            references(&stored.value, &mut pending);
        }
        objects.insert(number, stored);
    }

    // New protection's dictionary is written as an object after the rest,
    // and the old protection's is replaced, so its strings stay plain.
    let mut plain_object = encryption.as_ref().and_then(|e| e.cipher.dictionary_object);
    if let Some(Encryption {
        dictionary: Some(dictionary),
        ..
    }) = &encryption
    {
        let number = objects.keys().next_back().map_or(1, |&highest| highest + 1);
        let value = PdfObject::Dictionary(dictionary.clone());
        let stored = Stored {
            generation: 0,
            value,
            stream: None,
        };
        objects.insert(number, Some(stored));
        let reference = crate::ObjectReference {
            number,
            generation: 0,
        };
        kept.insert(b"Encrypt".to_vec(), PdfObject::Reference(reference));
        plain_object = Some(number);
        // AES-256 came with PDF 1.7's extensions.
        version = version.max(PdfVersion { major: 1, minor: 7 });
    }

    let mut output = format!("%PDF-{version}\n").into_bytes();
    // A comment of bytes above 127 marks the file as binary.
    output.extend(b"%\xE2\xE3\xCF\xD3\n");
    let mut offsets: BTreeMap<u32, (usize, u16)> = BTreeMap::new();
    for (&number, stored) in &objects {
        let Some(stored) = stored else {
            continue;
        };
        offsets.insert(number, (output.len(), stored.generation));
        output.extend(format!("{number} {} obj\n", stored.generation).as_bytes());
        let generation = u32::from(stored.generation);
        let mut value = stored.value.clone();
        let mut stream = stored.stream.clone();
        if let Some(Encryption { cipher, .. }) = &encryption
            && plain_object != Some(number)
        {
            map_strings(&mut value, &|data| {
                cipher.encrypt_string(number, generation, data)
            });
            if let (PdfObject::Dictionary(dictionary), Some(data)) = (&stored.value, &mut stream) {
                let kind = entry(dictionary, "Type").and_then(PdfObject::as_name);
                // A stream that names its own crypt filter is left as it is.
                let own_filter = match entry(dictionary, "Filter") {
                    Some(PdfObject::Name(name)) => name == b"Crypt",
                    Some(PdfObject::Array(names)) => {
                        names.iter().any(|name| name.as_name() == Some(b"Crypt"))
                    }
                    _ => false,
                };
                if !own_filter && (kind != Some(b"Metadata") || cipher.encrypt_metadata) {
                    *data = cipher.encrypt_stream(number, generation, data);
                }
            }
        }
        match (&value, &stream) {
            (PdfObject::Dictionary(dictionary), Some(data)) => {
                let mut dictionary = dictionary.clone();
                let length = PdfObject::Integer(i64::try_from(data.len()).unwrap_or(0));
                dictionary.insert(b"Length".to_vec(), length);
                write_object(&PdfObject::Dictionary(dictionary), &mut output);
                output.extend(b"\nstream\n");
                output.extend(data);
                output.extend(b"\nendstream");
            }
            (value, _) => write_object(value, &mut output),
        }
        output.extend(b"\nendobj\n");
    }

    // One table covers every number up to the highest; unused numbers form
    // the free list that starts at object 0.
    let size = offsets.keys().next_back().map_or(1, |&highest| highest + 1);
    let free: Vec<u32> = (1..size)
        .filter(|number| !offsets.contains_key(number))
        .collect();
    let table_start = output.len();
    output.extend(format!("xref\n0 {size}\n").as_bytes());
    let mut next_free = free.iter().copied().chain([0]);
    let first_free = next_free.next().unwrap_or(0);
    output.extend(format!("{first_free:010} 65535 f \n").as_bytes());
    for number in 1..size {
        let line = match offsets.get(&number) {
            Some((offset, generation)) => format!("{offset:010} {generation:05} n \n"),
            None => format!("{:010} 00000 f \n", next_free.next().unwrap_or(0)),
        };
        output.extend(line.as_bytes());
    }
    kept.insert(b"Size".to_vec(), PdfObject::Integer(i64::from(size)));
    output.extend(b"trailer\n");
    write_object(&PdfObject::Dictionary(kept), &mut output);
    output.extend(format!("\nstartxref\n{table_start}\n%%EOF\n").as_bytes());
    Ok(output)
}
