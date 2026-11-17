//! The page tree: pages in document order with their inherited attributes.

use super::DocumentError;
use super::reader::{Reader, entry};
use crate::{Dictionary, ObjectReference, PdfObject};
use std::collections::BTreeSet;

/// US Letter, used when a page declares no usable media box.
const DEFAULT_MEDIA_BOX: Rectangle = Rectangle {
    left: 0.0,
    bottom: 0.0,
    right: 612.0,
    top: 792.0,
};

/// A rectangle in default user space units (1/72 inch), origin bottom-left.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rectangle {
    /// Smaller horizontal coordinate.
    pub left: f64,
    /// Smaller vertical coordinate.
    pub bottom: f64,
    /// Larger horizontal coordinate.
    pub right: f64,
    /// Larger vertical coordinate.
    pub top: f64,
}

impl Rectangle {
    /// Horizontal extent.
    pub fn width(&self) -> f64 {
        self.right - self.left
    }

    /// Vertical extent.
    pub fn height(&self) -> f64 {
        self.top - self.bottom
    }

    /// The overlap of two rectangles, if they overlap at all.
    fn intersection(&self, other: &Self) -> Option<Self> {
        let overlap = Self {
            left: self.left.max(other.left),
            bottom: self.bottom.max(other.bottom),
            right: self.right.min(other.right),
            top: self.top.min(other.top),
        };
        (overlap.width() > 0.0 && overlap.height() > 0.0).then_some(overlap)
    }
}

/// One page's geometry, before rotation is applied.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Page {
    /// The page object, when the page tree refers to it indirectly.
    pub reference: Option<ObjectReference>,
    /// The physical medium; US Letter when the file declares none.
    pub media_box: Rectangle,
    /// The visible region: the crop box clipped to the media box.
    pub crop_box: Rectangle,
    /// Clockwise display rotation in degrees: 0, 90, 180, or 270.
    pub rotation: u16,
}

impl Page {
    /// The width and height of the page as it is displayed: the crop box,
    /// turned by the rotation.
    pub fn view_size(&self) -> (f64, f64) {
        let (width, height) = (self.crop_box.width(), self.crop_box.height());
        match self.rotation {
            90 | 270 => (height, width),
            _ => (width, height),
        }
    }

    /// Where a point of the page lies as the page is displayed: in points
    /// from the top left of the displayed page, with the vertical axis
    /// running down. Glyph, word, and link positions are given in the
    /// page's own space, from the bottom left before rotation; this turns
    /// them into the space a viewer draws in.
    pub fn to_view(&self, x: f64, y: f64) -> (f64, f64) {
        let (width, height) = (self.crop_box.width(), self.crop_box.height());
        let (across, down) = (x - self.crop_box.left, self.crop_box.top - y);
        match self.rotation {
            90 => (height - down, across),
            180 => (width - across, height - down),
            270 => (down, width - across),
            _ => (across, down),
        }
    }

    /// The point of the page that is displayed at a place given in points
    /// from the top left of the displayed page: the reverse of
    /// [`Page::to_view`].
    pub fn from_view(&self, x: f64, y: f64) -> (f64, f64) {
        let (width, height) = (self.crop_box.width(), self.crop_box.height());
        let (across, down) = match self.rotation {
            90 => (y, height - x),
            180 => (width - x, height - y),
            270 => (width - y, x),
            _ => (x, y),
        };
        (across + self.crop_box.left, self.crop_box.top - down)
    }
}

/// Attributes a page inherits from its ancestors unless it sets its own.
#[derive(Clone, Copy, Default)]
struct Inherited {
    media_box: Option<Rectangle>,
    crop_box: Option<Rectangle>,
    rotation: u16,
}

/// Walk the page tree from the catalog and list its pages in order.
///
/// Unreadable nodes are skipped and a node reached twice is ignored, so a
/// damaged or cyclic tree still yields the pages that can be read.
pub(super) fn collect(
    reader: &mut Reader,
    trailer: &Dictionary,
) -> Result<Vec<Page>, DocumentError> {
    let PdfObject::Dictionary(catalog) = reader.resolve_entry(trailer, "Root")? else {
        return Err(DocumentError::Structure("the document catalog is missing"));
    };
    let root = entry(&catalog, "Pages")
        .cloned()
        .ok_or(DocumentError::Structure("the page tree is missing"))?;

    let mut pages = Vec::new();
    let mut visited = BTreeSet::new();
    let mut pending = vec![(root, Inherited::default())];
    while let Some((node, mut inherited)) = pending.pop() {
        let reference = match node {
            PdfObject::Reference(reference) => Some(reference),
            _ => None,
        };
        if let Some(reference) = reference
            && !visited.insert(reference.number)
        {
            continue;
        }
        let Ok(PdfObject::Dictionary(node)) = reader.resolve(&node) else {
            continue;
        };
        if let Some(media_box) = rectangle(reader, &node, "MediaBox") {
            inherited.media_box = Some(media_box);
        }
        if let Some(crop_box) = rectangle(reader, &node, "CropBox") {
            inherited.crop_box = Some(crop_box);
        }
        if let Some(rotation) = rotation(reader, &node) {
            inherited.rotation = rotation;
        }

        let is_page = entry(&node, "Type").and_then(PdfObject::as_name) == Some(b"Page");
        let kids = match reader.resolve_entry(&node, "Kids") {
            Ok(PdfObject::Array(kids)) if !is_page => Some(kids),
            _ => None,
        };
        if let Some(kids) = kids {
            // The stack is read last-in first, so push the kids in reverse.
            pending.extend(kids.into_iter().rev().map(|kid| (kid, inherited)));
        } else {
            let media_box = inherited.media_box.unwrap_or(DEFAULT_MEDIA_BOX);
            pages.push(Page {
                reference,
                media_box,
                crop_box: inherited
                    .crop_box
                    .and_then(|crop_box| crop_box.intersection(&media_box))
                    .unwrap_or(media_box),
                rotation: inherited.rotation,
            });
        }
    }
    Ok(pages)
}

/// A rectangle entry with its corners put in order, if it is well formed.
pub(super) fn rectangle(reader: &mut Reader, node: &Dictionary, key: &str) -> Option<Rectangle> {
    let PdfObject::Array(values) = reader.resolve_entry(node, key).ok()? else {
        return None;
    };
    let mut numbers = [0.0; 4];
    if values.len() != numbers.len() {
        return None;
    }
    for (number, value) in numbers.iter_mut().zip(&values) {
        *number = reader.resolve(value).ok()?.as_number()?;
    }
    let [x0, y0, x1, y1] = numbers;
    let rectangle = Rectangle {
        left: x0.min(x1),
        bottom: y0.min(y1),
        right: x0.max(x1),
        top: y0.max(y1),
    };
    (rectangle.width() > 0.0 && rectangle.height() > 0.0).then_some(rectangle)
}

/// A `/Rotate` entry reduced to 0, 90, 180, or 270; other values are ignored.
fn rotation(reader: &mut Reader, node: &Dictionary) -> Option<u16> {
    let degrees = reader.resolve_entry(node, "Rotate").ok()?.as_integer()?;
    let degrees = degrees.rem_euclid(360);
    (degrees % 90 == 0).then_some(degrees as u16)
}
