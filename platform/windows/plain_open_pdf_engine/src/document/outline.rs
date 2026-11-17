//! The document outline, also called bookmarks (ISO 32000-2 section 12.3.3).

use super::destination::{Destinations, View};
use super::reader::{Reader, entry};
use super::text::decode_text_string;
use super::{DocumentError, Page};
use crate::{Dictionary, PdfObject};
use std::collections::BTreeSet;

/// Deepest nesting of outline items that is followed.
const MAX_DEPTH: usize = 64;

/// One entry of the document outline.
#[derive(Debug, Clone, PartialEq)]
pub struct OutlineItem {
    /// Nesting depth; top-level items are level 1.
    pub level: usize,
    /// The item's title.
    pub title: String,
    /// Zero-based index of the page the item leads to, when it leads to a
    /// page of this document.
    pub page: Option<usize>,
    /// How the page is shown, when the item's destination gives a view.
    pub view: Option<View>,
}

/// List the outline items in reading order.
///
/// Items reached twice are listed once, so a cyclic outline still ends.
pub(super) fn read(
    reader: &mut Reader,
    trailer: &Dictionary,
    pages: &[Page],
) -> Result<Vec<OutlineItem>, DocumentError> {
    let Some(mut destinations) = Destinations::new(reader, trailer, pages)? else {
        return Ok(Vec::new());
    };
    let outlines = destinations
        .reader
        .resolve_entry(&destinations.catalog, "Outlines")?;
    let PdfObject::Dictionary(outlines) = outlines else {
        return Ok(Vec::new());
    };
    let mut items = Vec::new();
    let mut visited = BTreeSet::new();
    // Each pending entry is the first unread sibling at some level.
    let mut pending = vec![(entry(&outlines, "First").cloned(), 1)];
    while let Some((item, level)) = pending.pop() {
        let Some(item) = item else {
            continue;
        };
        if let PdfObject::Reference(reference) = item
            && !visited.insert(reference.number)
        {
            continue;
        }
        let Ok(PdfObject::Dictionary(item)) = destinations.reader.resolve(&item) else {
            continue;
        };
        let title = match destinations.reader.resolve_entry(&item, "Title") {
            Ok(PdfObject::String(title)) => decode_text_string(&title),
            _ => String::new(),
        };
        let (page, view) = item_page(&mut destinations, &item).unzip();
        items.push(OutlineItem {
            level,
            title,
            page,
            view: view.flatten(),
        });
        // Children come before the next sibling, so push the sibling first.
        pending.push((entry(&item, "Next").cloned(), level));
        if level < MAX_DEPTH {
            pending.push((entry(&item, "First").cloned(), level + 1));
        }
    }
    Ok(items)
}

/// The page an outline item leads to through `/Dest` or a go-to action,
/// and the view of it.
fn item_page(
    destinations: &mut Destinations<'_>,
    item: &Dictionary,
) -> Option<(usize, Option<View>)> {
    let destination = match entry(item, "Dest") {
        Some(destination) => destination.clone(),
        None => {
            let action = destinations.reader.resolve_entry(item, "A").ok()?;
            let action = action.as_dictionary()?;
            if entry(action, "S").and_then(PdfObject::as_name) != Some(b"GoTo") {
                return None;
            }
            entry(action, "D")?.clone()
        }
    };
    destinations.resolve(&destination)
}
