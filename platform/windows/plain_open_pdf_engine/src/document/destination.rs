//! Destinations and name trees: how outline items and links name a page
//! (ISO 32000-2 sections 7.9.6 and 12.3.2).

use super::reader::{Reader, entry};
use super::{DocumentError, Page, Rectangle};
use crate::{Dictionary, PdfObject};
use std::collections::BTreeMap;

/// How a viewer shows the page a destination leads to
/// (ISO 32000-2 section 12.3.2.2).
///
/// Coordinates are in the page's default user space, from its bottom left.
/// `None` keeps the viewer's current value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum View {
    /// Put (`left`, `top`) at the window's top left corner and magnify by
    /// `zoom`, where 1 is 100 percent.
    Xyz {
        /// Horizontal coordinate of the window's left edge.
        left: Option<f64>,
        /// Vertical coordinate of the window's top edge.
        top: Option<f64>,
        /// Magnification.
        zoom: Option<f64>,
    },
    /// Fit the whole page in the window.
    Fit,
    /// Fit the page's width, with `top` at the window's top edge.
    FitH {
        /// Vertical coordinate of the window's top edge.
        top: Option<f64>,
    },
    /// Fit the page's height, with `left` at the window's left edge.
    FitV {
        /// Horizontal coordinate of the window's left edge.
        left: Option<f64>,
    },
    /// Fit a rectangle of the page in the window.
    FitR(Rectangle),
    /// Fit the bounding box of the page's contents in the window.
    FitB,
    /// Fit the width of the contents' bounding box, with `top` at the
    /// window's top edge.
    FitBH {
        /// Vertical coordinate of the window's top edge.
        top: Option<f64>,
    },
    /// Fit the height of the contents' bounding box, with `left` at the
    /// window's left edge.
    FitBV {
        /// Horizontal coordinate of the window's left edge.
        left: Option<f64>,
    },
}

impl View {
    /// The point to put at the window's top left corner, as far as the
    /// view gives one: (left, top).
    pub fn point(&self) -> (Option<f64>, Option<f64>) {
        match *self {
            Self::Xyz { left, top, .. } => (left, top),
            Self::FitH { top } | Self::FitBH { top } => (None, top),
            Self::FitV { left } | Self::FitBV { left } => (left, None),
            Self::FitR(area) => (Some(area.left), Some(area.top)),
            Self::Fit | Self::FitB => (None, None),
        }
    }

    /// The magnification an `Xyz` view sets, where 1 is 100 percent.
    pub fn zoom(&self) -> Option<f64> {
        match *self {
            Self::Xyz { zoom, .. } => zoom,
            _ => None,
        }
    }

    /// The view's name in a destination array, such as `XYZ` or `FitH`.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Xyz { .. } => "XYZ",
            Self::Fit => "Fit",
            Self::FitH { .. } => "FitH",
            Self::FitV { .. } => "FitV",
            Self::FitR(_) => "FitR",
            Self::FitB => "FitB",
            Self::FitBH { .. } => "FitBH",
            Self::FitBV { .. } => "FitBV",
        }
    }

    /// Read the elements after the page of a destination array.
    ///
    /// Returns `None` for an unknown view or a `FitR` without its four
    /// coordinates. A missing or non-numeric coordinate counts as null,
    /// and a zoom of 0 means the same as a null zoom.
    pub(super) fn parse(elements: &[PdfObject]) -> Option<Self> {
        let (name, values) = elements.split_first()?;
        let value = |index: usize| values.get(index).and_then(PdfObject::as_number);
        Some(match name.as_name()? {
            b"XYZ" => Self::Xyz {
                left: value(0),
                top: value(1),
                zoom: value(2).filter(|&zoom| zoom > 0.0),
            },
            b"Fit" => Self::Fit,
            b"FitH" => Self::FitH { top: value(0) },
            b"FitV" => Self::FitV { left: value(0) },
            b"FitR" => {
                let (left, bottom, right, top) = (value(0)?, value(1)?, value(2)?, value(3)?);
                Self::FitR(Rectangle {
                    left: left.min(right),
                    bottom: bottom.min(top),
                    right: left.max(right),
                    top: bottom.max(top),
                })
            }
            b"FitB" => Self::FitB,
            b"FitBH" => Self::FitBH { top: value(0) },
            b"FitBV" => Self::FitBV { left: value(0) },
            _ => return None,
        })
    }

    /// The elements that follow the page in a destination array.
    pub(super) fn elements(&self) -> Vec<PdfObject> {
        let number = |value: Option<f64>| value.map_or(PdfObject::Null, PdfObject::Real);
        let mut elements = vec![PdfObject::Name(self.name().as_bytes().to_vec())];
        match *self {
            Self::Xyz { left, top, zoom } => {
                elements.extend([number(left), number(top), number(zoom)]);
            }
            Self::FitH { top } | Self::FitBH { top } => elements.push(number(top)),
            Self::FitV { left } | Self::FitBV { left } => elements.push(number(left)),
            Self::FitR(area) => {
                elements.extend([area.left, area.bottom, area.right, area.top].map(PdfObject::Real))
            }
            Self::Fit | Self::FitB => {}
        }
        elements
    }
}

/// Deepest nesting of name tree nodes that is followed.
const MAX_TREE_DEPTH: usize = 64;

/// Resolves destinations of one document to page indexes.
pub(super) struct Destinations<'a> {
    pub reader: &'a mut Reader,
    pub catalog: Dictionary,
    /// Page index by page object number.
    page_numbers: BTreeMap<u32, usize>,
}

impl<'a> Destinations<'a> {
    /// Returns `None` when the document has no readable catalog.
    pub fn new(
        reader: &'a mut Reader,
        trailer: &Dictionary,
        pages: &[Page],
    ) -> Result<Option<Self>, DocumentError> {
        let PdfObject::Dictionary(catalog) = reader.resolve_entry(trailer, "Root")? else {
            return Ok(None);
        };
        let page_numbers = pages
            .iter()
            .enumerate()
            .filter_map(|(index, page)| Some((page.reference?.number, index)))
            .collect();
        Ok(Some(Self {
            reader,
            catalog,
            page_numbers,
        }))
    }

    /// The page a destination leads to, and the view of it when the
    /// destination gives a readable one: an explicit array whose first
    /// element is a page, or a name that is looked up first.
    pub fn resolve(&mut self, destination: &PdfObject) -> Option<(usize, Option<View>)> {
        let destination = match self.reader.resolve(destination).ok()? {
            // PDF 1.1 names are entries of the catalog's `/Dests` dictionary.
            PdfObject::Name(name) => {
                let named = self.reader.resolve_entry(&self.catalog, "Dests").ok()?;
                let named = named.as_dictionary()?.get(&name)?.clone();
                self.reader.resolve(&named).ok()?
            }
            // Later files keep them in a name tree keyed by string.
            PdfObject::String(name) => {
                let names = self.reader.resolve_entry(&self.catalog, "Names").ok()?;
                let tree = entry(names.as_dictionary()?, "Dests")?.clone();
                let named = name_tree_value(self.reader, &tree, &name, 0)?;
                self.reader.resolve(&named).ok()?
            }
            other => other,
        };
        // A named destination may be wrapped in a dictionary under `/D`.
        let destination = match destination {
            PdfObject::Dictionary(wrapper) => self.reader.resolve_entry(&wrapper, "D").ok()?,
            other => other,
        };
        let (page, view) = destination.as_array()?.split_first()?;
        let page = match page {
            PdfObject::Reference(page) => self.page_numbers.get(&page.number).copied()?,
            _ => return None,
        };
        Some((page, View::parse(view)))
    }
}

/// Find a key in a name tree: nodes hold `/Names` pairs or `/Kids`, and
/// `/Limits` gives the least and greatest key below a node.
fn name_tree_value(
    reader: &mut Reader,
    node: &PdfObject,
    key: &[u8],
    depth: usize,
) -> Option<PdfObject> {
    if depth > MAX_TREE_DEPTH {
        return None;
    }
    let PdfObject::Dictionary(node) = reader.resolve(node).ok()? else {
        return None;
    };
    if let Ok(PdfObject::Array(limits)) = reader.resolve_entry(&node, "Limits")
        && let [PdfObject::String(least), PdfObject::String(greatest)] = limits.as_slice()
        && !(least.as_slice()..=greatest.as_slice()).contains(&key)
    {
        return None;
    }
    if let Ok(PdfObject::Array(names)) = reader.resolve_entry(&node, "Names") {
        for pair in names.as_chunks::<2>().0 {
            if matches!(&pair[0], PdfObject::String(name) if name == key) {
                return Some(pair[1].clone());
            }
        }
    }
    if let Ok(PdfObject::Array(kids)) = reader.resolve_entry(&node, "Kids") {
        for kid in &kids {
            if let Some(value) = name_tree_value(reader, kid, key, depth + 1) {
                return Some(value);
            }
        }
    }
    None
}

/// Every key and value of a name tree, in the tree's own order.
///
/// Nodes nested too deeply are left out, which also ends a cyclic tree.
pub(super) fn name_tree_entries(
    reader: &mut Reader,
    root: &PdfObject,
) -> Vec<(Vec<u8>, PdfObject)> {
    let mut entries = Vec::new();
    let mut pending = vec![(root.clone(), 0)];
    while let Some((node, depth)) = pending.pop() {
        let Ok(PdfObject::Dictionary(node)) = reader.resolve(&node) else {
            continue;
        };
        if let Ok(PdfObject::Array(names)) = reader.resolve_entry(&node, "Names") {
            for pair in names.as_chunks::<2>().0 {
                if let PdfObject::String(name) = &pair[0] {
                    entries.push((name.clone(), pair[1].clone()));
                }
            }
        }
        if depth < MAX_TREE_DEPTH
            && let Ok(PdfObject::Array(kids)) = reader.resolve_entry(&node, "Kids")
        {
            // The stack is read last-in first, so push the kids in reverse.
            pending.extend(kids.into_iter().rev().map(|kid| (kid, depth + 1)));
        }
    }
    entries
}
