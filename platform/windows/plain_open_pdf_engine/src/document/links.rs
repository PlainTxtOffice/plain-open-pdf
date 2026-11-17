//! Link annotations on a page and where they lead
//! (ISO 32000-2 sections 12.5.6.5 and 12.6.4).

use super::destination::{Destinations, View};
use super::pages::rectangle;
use super::reader::{Reader, entry};
use super::text::{decode_text_string, encode_text_string};
use super::{DocumentError, Page, Rectangle};
use crate::{Dictionary, PdfObject};

/// A clickable region of a page.
#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    /// The region, in the page's default user space.
    pub rectangle: Rectangle,
    /// What activating the link does.
    pub target: LinkTarget,
    /// How the target page is shown, for a link to a page of this or
    /// another document whose destination gives a view.
    pub view: Option<View>,
}

/// Where a link leads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkTarget {
    /// A page of this document, by zero-based index.
    Page(usize),
    /// A uniform resource identifier, such as a web address.
    Uri(String),
    /// Another PDF file (a remote go-to action).
    RemoteDocument {
        /// The file specification's path.
        file: String,
        /// Zero-based page index in that file, when the action gives one.
        page: Option<usize>,
    },
    /// A file or application to open (a launch action).
    Launch(String),
    /// A viewer operation by name, such as `NextPage` (a named action).
    Named(String),
}

/// List the links of one page in the order the page stores them.
///
/// Link annotations whose rectangle or target cannot be read are left out,
/// as are those with actions other than the ones [`LinkTarget`] describes.
pub(super) fn read(
    reader: &mut Reader,
    trailer: &Dictionary,
    pages: &[Page],
    page: &Page,
) -> Result<Vec<Link>, DocumentError> {
    let Some(reference) = page.reference else {
        return Ok(Vec::new());
    };
    let Some(mut destinations) = Destinations::new(reader, trailer, pages)? else {
        return Ok(Vec::new());
    };
    let reader = &mut *destinations.reader;
    let PdfObject::Dictionary(page) = reader.resolve(&PdfObject::Reference(reference))? else {
        return Ok(Vec::new());
    };
    let PdfObject::Array(annotations) = reader.resolve_entry(&page, "Annots")? else {
        return Ok(Vec::new());
    };
    let mut links = Vec::new();
    for annotation in &annotations {
        let Ok(PdfObject::Dictionary(annotation)) = destinations.reader.resolve(annotation) else {
            continue;
        };
        if entry(&annotation, "Subtype").and_then(PdfObject::as_name) != Some(b"Link") {
            continue;
        }
        let Some(rectangle) = rectangle(destinations.reader, &annotation, "Rect") else {
            continue;
        };
        if let Some((target, view)) = target(&mut destinations, &annotation) {
            links.push(Link {
                rectangle,
                target,
                view,
            });
        }
    }
    Ok(links)
}

/// The target of a link annotation and the view of its page: its `/Dest`,
/// or else its action `/A`.
fn target(
    destinations: &mut Destinations<'_>,
    annotation: &Dictionary,
) -> Option<(LinkTarget, Option<View>)> {
    let page = |(page, view)| (LinkTarget::Page(page), view);
    if let Some(destination) = entry(annotation, "Dest") {
        return destinations.resolve(destination).map(page);
    }
    let action = destinations.reader.resolve_entry(annotation, "A").ok()?;
    let action = action.as_dictionary()?;
    let mut text = |key| match destinations.reader.resolve_entry(action, key) {
        Ok(PdfObject::String(bytes)) => Some(decode_text_string(&bytes)),
        _ => None,
    };
    match entry(action, "S")?.as_name()? {
        b"GoTo" => {
            let destination = entry(action, "D")?.clone();
            destinations.resolve(&destination).map(page)
        }
        b"URI" => text("URI").map(|uri| (LinkTarget::Uri(uri), None)),
        b"GoToR" => {
            // A remote destination names its page by number, not reference.
            let (page, view) = match destinations.reader.resolve_entry(action, "D") {
                Ok(PdfObject::Array(destination)) => match destination.split_first() {
                    Some((page, view)) => (
                        page.as_integer()
                            .and_then(|page| usize::try_from(page).ok()),
                        View::parse(view),
                    ),
                    None => (None, None),
                },
                _ => (None, None),
            };
            let file = file_path(destinations.reader, action)?;
            Some((LinkTarget::RemoteDocument { file, page }, view))
        }
        b"Launch" => {
            file_path(destinations.reader, action).map(|file| (LinkTarget::Launch(file), None))
        }
        b"Named" => {
            let name = entry(action, "N")?.as_name()?;
            let name = String::from_utf8_lossy(name).into_owned();
            Some((LinkTarget::Named(name), None))
        }
        _ => None,
    }
}

/// The path in an action's file specification `/F`: a string, or a
/// dictionary whose `/UF` (Unicode) entry is preferred over `/F`.
fn file_path(reader: &mut Reader, action: &Dictionary) -> Option<String> {
    match reader.resolve_entry(action, "F").ok()? {
        PdfObject::String(path) => Some(decode_text_string(&path)),
        PdfObject::Dictionary(specification) => file_name(reader, &specification),
        _ => None,
    }
}

/// The file name of a file specification dictionary.
pub(super) fn file_name(reader: &mut Reader, specification: &Dictionary) -> Option<String> {
    ["UF", "F"]
        .into_iter()
        .find_map(|key| match reader.resolve_entry(specification, key) {
            Ok(PdfObject::String(name)) => Some(decode_text_string(&name)),
            _ => None,
        })
}

impl super::Document {
    /// Add a link to a page: a rectangle in the page's default user space
    /// that leads to `target`. `view` sets how a target page, in this or
    /// another document, is shown; without one, a page of this document
    /// opens at its top and a page of another fits the window. The link is
    /// drawn without a border.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentError::Structure`] when the page index, or the
    /// target page of this document, is out of range, or the page is not an
    /// indirect object, and a [`DocumentError`] when the page cannot be
    /// parsed.
    pub fn add_link(
        &mut self,
        page_index: usize,
        rectangle: Rectangle,
        target: &LinkTarget,
        view: Option<View>,
    ) -> Result<(), DocumentError> {
        let out_of_range = DocumentError::Structure("a page index is out of range");
        let Some(page) = self.pages.get(page_index) else {
            return Err(out_of_range);
        };
        let Some(reference) = page.reference else {
            return Err(DocumentError::Structure(
                "the page is not an indirect object",
            ));
        };
        let named = |text: &str| PdfObject::Name(text.as_bytes().to_vec());
        let text = |text: &str| PdfObject::String(encode_text_string(text));
        let mut annotation = Dictionary::new();
        annotation.insert(b"Type".to_vec(), named("Annot"));
        annotation.insert(b"Subtype".to_vec(), named("Link"));
        let corners = [
            rectangle.left.min(rectangle.right),
            rectangle.bottom.min(rectangle.top),
            rectangle.left.max(rectangle.right),
            rectangle.bottom.max(rectangle.top),
        ];
        let corners = corners.map(PdfObject::Real).to_vec();
        annotation.insert(b"Rect".to_vec(), PdfObject::Array(corners));
        let border = vec![PdfObject::Integer(0); 3];
        annotation.insert(b"Border".to_vec(), PdfObject::Array(border));
        let action = |kind: &str, entries: Vec<(&str, PdfObject)>| {
            let mut action = Dictionary::new();
            action.insert(b"S".to_vec(), named(kind));
            for (key, value) in entries {
                action.insert(key.as_bytes().to_vec(), value);
            }
            PdfObject::Dictionary(action)
        };
        match target {
            LinkTarget::Page(index) => {
                let target_page = self.pages.get(*index).ok_or(out_of_range)?;
                let Some(target_reference) = target_page.reference else {
                    return Err(DocumentError::Structure(
                        "the page is not an indirect object",
                    ));
                };
                let view = view.unwrap_or(View::Xyz {
                    left: None,
                    top: Some(target_page.crop_box.top),
                    zoom: None,
                });
                let mut destination = vec![PdfObject::Reference(target_reference)];
                destination.extend(view.elements());
                annotation.insert(b"Dest".to_vec(), PdfObject::Array(destination));
            }
            LinkTarget::Uri(uri) => {
                // A URI is ASCII; its bytes are written as they are.
                let uri = PdfObject::String(uri.as_bytes().to_vec());
                annotation.insert(b"A".to_vec(), action("URI", vec![("URI", uri)]));
            }
            LinkTarget::RemoteDocument { file, page } => {
                let page = i64::try_from(page.unwrap_or(0)).unwrap_or(0);
                let mut destination = vec![PdfObject::Integer(page)];
                destination.extend(view.unwrap_or(View::Fit).elements());
                let entries = vec![("F", text(file)), ("D", PdfObject::Array(destination))];
                annotation.insert(b"A".to_vec(), action("GoToR", entries));
            }
            LinkTarget::Launch(file) => {
                annotation.insert(b"A".to_vec(), action("Launch", vec![("F", text(file))]));
            }
            LinkTarget::Named(name) => {
                let entries = vec![("N", named(name))];
                annotation.insert(b"A".to_vec(), action("Named", entries));
            }
        }
        let annotation = self.add_object(PdfObject::Dictionary(annotation));

        let PdfObject::Dictionary(mut entries) = self.object(reference)? else {
            return Err(DocumentError::Structure(
                "the page is not an indirect object",
            ));
        };
        let mut annotations = match self.reader.resolve_entry(&entries, "Annots")? {
            PdfObject::Array(annotations) => annotations,
            _ => Vec::new(),
        };
        annotations.push(PdfObject::Reference(annotation));
        entries.insert(b"Annots".to_vec(), PdfObject::Array(annotations));
        self.set_object(reference, PdfObject::Dictionary(entries));
        Ok(())
    }
}
