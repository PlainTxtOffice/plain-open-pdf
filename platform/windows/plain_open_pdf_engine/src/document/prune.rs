//! Removing what still points at pages that were taken out of a document,
//! so that saving leaves those pages out of the file.

use super::DocumentError;
use super::reader::{Reader, entry};
use crate::{Dictionary, ObjectReference, PdfObject};
use std::collections::BTreeSet;

/// How deep a name tree or a chain of destinations may go.
const MAX_DEPTH: usize = 32;
/// The most entries read from a name tree.
const MAX_NAMES: usize = 1_000_000;

/// The object number of the page a destination leads to, if it names its
/// page by reference: `[page /Fit ...]`, or a dictionary whose `/D` is one.
fn destination_page(reader: &mut Reader, destination: &PdfObject, depth: usize) -> Option<u32> {
    if depth > MAX_DEPTH {
        return None;
    }
    match reader.resolve(destination).ok()? {
        PdfObject::Array(parts) => match parts.first() {
            Some(PdfObject::Reference(page)) => Some(page.number),
            _ => None,
        },
        PdfObject::Dictionary(entries) => {
            destination_page(reader, entry(&entries, "D")?, depth + 1)
        }
        _ => None,
    }
}

/// Whether a destination, or a go-to action, leads to a removed page.
fn leads_to_removed(reader: &mut Reader, target: &PdfObject, removed: &BTreeSet<u32>) -> bool {
    destination_page(reader, target, 0).is_some_and(|page| removed.contains(&page))
}

/// Collect the entries of a name tree, in order.
fn name_tree(
    reader: &mut Reader,
    node: &PdfObject,
    depth: usize,
    entries: &mut Vec<(PdfObject, PdfObject)>,
) {
    if depth > MAX_DEPTH || entries.len() > MAX_NAMES {
        return;
    }
    let Ok(PdfObject::Dictionary(node)) = reader.resolve(node) else {
        return;
    };
    if let Ok(PdfObject::Array(names)) = reader.resolve_entry(&node, "Names") {
        for pair in names.as_chunks::<2>().0 {
            entries.push((pair[0].clone(), pair[1].clone()));
        }
    }
    if let Ok(PdfObject::Array(kids)) = reader.resolve_entry(&node, "Kids") {
        for kid in &kids {
            name_tree(reader, kid, depth + 1, entries);
        }
    }
}

/// Drop what leads to the removed pages: named destinations, the open
/// action, links on the kept pages, article threads, and the structure
/// tree, which describes the document page by page.
///
/// `removed` holds the object numbers of the pages taken out.
pub(super) fn prune(
    reader: &mut Reader,
    trailer: &Dictionary,
    kept: &[ObjectReference],
    removed: &BTreeSet<u32>,
) -> Result<(), DocumentError> {
    let Some(&PdfObject::Reference(catalog_reference)) = entry(trailer, "Root") else {
        return Ok(());
    };
    let PdfObject::Dictionary(mut catalog) =
        reader.resolve(&PdfObject::Reference(catalog_reference))?
    else {
        return Ok(());
    };
    for key in ["StructTreeRoot", "MarkInfo", "Threads"] {
        catalog.remove(key.as_bytes());
    }
    if let Some(action) = entry(&catalog, "OpenAction").cloned()
        && leads_to_removed(reader, &action, removed)
    {
        catalog.remove(b"OpenAction".as_slice());
    }
    // Named destinations kept in a dictionary, as PDF 1.1 did.
    if let PdfObject::Dictionary(mut names) = reader.resolve_entry(&catalog, "Dests")? {
        let gone: Vec<Vec<u8>> = names
            .iter()
            .filter(|(_, target)| leads_to_removed(reader, target, removed))
            .map(|(name, _)| name.clone())
            .collect();
        for name in gone {
            names.remove(&name);
        }
        catalog.insert(b"Dests".to_vec(), PdfObject::Dictionary(names));
    }
    // Named destinations kept in a name tree, which is rewritten as one
    // node.
    if let PdfObject::Dictionary(mut names) = reader.resolve_entry(&catalog, "Names")?
        && let Some(tree) = entry(&names, "Dests").cloned()
    {
        let mut entries = Vec::new();
        name_tree(reader, &tree, 0, &mut entries);
        let mut flat = Vec::new();
        for (name, target) in entries {
            if !leads_to_removed(reader, &target, removed) {
                flat.push(name);
                flat.push(target);
            }
        }
        let mut node = Dictionary::new();
        node.insert(b"Names".to_vec(), PdfObject::Array(flat));
        names.insert(b"Dests".to_vec(), PdfObject::Dictionary(node));
        catalog.insert(b"Names".to_vec(), PdfObject::Dictionary(names));
    }
    reader.set_object(
        catalog_reference.number,
        PdfObject::Dictionary(catalog),
        None,
    );

    for page_reference in kept {
        let PdfObject::Dictionary(mut page) =
            reader.resolve(&PdfObject::Reference(*page_reference))?
        else {
            continue;
        };
        let mut changed = page.remove(b"B".as_slice()).is_some();
        changed |= page.remove(b"StructParents".as_slice()).is_some();
        if let PdfObject::Array(annotations) = reader.resolve_entry(&page, "Annots")? {
            let mut kept_annotations = Vec::new();
            for annotation in &annotations {
                let mut gone = false;
                if let Ok(PdfObject::Dictionary(entries)) = reader.resolve(annotation) {
                    if let Some(destination) = entry(&entries, "Dest") {
                        gone = leads_to_removed(reader, destination, removed);
                    }
                    if let Ok(PdfObject::Dictionary(action)) = reader.resolve_entry(&entries, "A")
                        && entry(&action, "S").and_then(PdfObject::as_name) == Some(b"GoTo")
                        && let Some(destination) = entry(&action, "D")
                    {
                        gone |= leads_to_removed(reader, destination, removed);
                    }
                }
                if !gone {
                    kept_annotations.push(annotation.clone());
                }
            }
            if kept_annotations.len() != annotations.len() {
                page.insert(b"Annots".to_vec(), PdfObject::Array(kept_annotations));
                changed = true;
            }
        }
        if changed {
            reader.set_object(page_reference.number, PdfObject::Dictionary(page), None);
        }
    }
    Ok(())
}
