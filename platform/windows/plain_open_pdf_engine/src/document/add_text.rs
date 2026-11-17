//! Adding text to a page in one of the standard 14 fonts.

use super::DocumentError;
use super::page_text::page_resources;
use super::reader::{Reader, entry};
use super::redact::{add_stream, operation, write};
use crate::font::StandardFont;
use crate::{Dictionary, ObjectReference, PdfObject};

/// The distance between the baselines of two lines, as a multiple of the
/// font size.
const LINE_SPACING: f64 = 1.2;

/// Show text on a page from `origin`, the start of its first baseline in
/// default user space, and return the width of its widest line.
pub(super) fn insert_text(
    reader: &mut Reader,
    page: ObjectReference,
    origin: (f64, f64),
    text: &str,
    font: StandardFont,
    size: f64,
    color: [f64; 3],
) -> Result<f64, DocumentError> {
    if !(size.is_finite() && size > 0.0) {
        return Err(DocumentError::Structure("a font size is positive"));
    }
    if !(origin.0.is_finite() && origin.1.is_finite()) {
        return Err(DocumentError::Structure("a point is not finite"));
    }
    let PdfObject::Dictionary(mut entries) = reader.resolve(&PdfObject::Reference(page))? else {
        return Err(DocumentError::Structure("a page is not a dictionary"));
    };
    let mut resources = page_resources(reader, page)?;
    let mut fonts = match reader.resolve_entry(&resources, "Font")? {
        PdfObject::Dictionary(fonts) => fonts,
        _ => Dictionary::new(),
    };
    let name = font_resource(reader, &mut fonts, font)?;
    resources.insert(b"Font".to_vec(), PdfObject::Dictionary(fonts));
    entries.insert(b"Resources".to_vec(), PdfObject::Dictionary(resources));

    let number = |value: f64| PdfObject::Real(value);
    let mut content = Vec::new();
    write(&operation("q", Vec::new()), &mut content);
    write(&operation("BT", Vec::new()), &mut content);
    let selected = vec![PdfObject::Name(name), number(size)];
    write(&operation("Tf", selected), &mut content);
    let color = color.map(|value| number(value.clamp(0.0, 1.0)));
    write(&operation("rg", color.to_vec()), &mut content);
    let mut widest: f64 = 0.0;
    for (index, line) in text.split('\n').enumerate() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let offset = match index {
            0 => [origin.0, origin.1],
            _ => [0.0, -size * LINE_SPACING],
        };
        write(&operation("Td", offset.map(number).to_vec()), &mut content);
        let shown = PdfObject::String(font.encode(line));
        write(&operation("Tj", vec![shown]), &mut content);
        widest = widest.max(font.text_width(line, size));
    }
    write(&operation("ET", Vec::new()), &mut content);
    write(&operation("Q", Vec::new()), &mut content);

    // The page's own content is enclosed in `q` and `Q`, so that what it
    // leaves set does not move or color the text.
    let streams: Vec<PdfObject> = match entry(&entries, "Contents") {
        None => Vec::new(),
        Some(contents) => match reader.resolve(contents)? {
            PdfObject::Array(streams) => streams,
            _ => vec![contents.clone()],
        },
    };
    let mut contents = Vec::new();
    if !streams.is_empty() {
        let opening = add_stream(reader, Dictionary::new(), b"q\n");
        contents.push(PdfObject::Reference(opening));
        contents.extend(streams);
        let mut closed = b"Q\n".to_vec();
        closed.extend(content);
        content = closed;
    }
    let added = add_stream(reader, Dictionary::new(), &content);
    contents.push(PdfObject::Reference(added));
    entries.insert(b"Contents".to_vec(), PdfObject::Array(contents));
    reader.set_object(page.number, PdfObject::Dictionary(entries), None);
    Ok(widest)
}

/// The resource name of a font dictionary for `font` in `fonts`, adding one
/// that has its widths in the encoding [`StandardFont::encode`] writes.
fn font_resource(
    reader: &mut Reader,
    fonts: &mut Dictionary,
    font: StandardFont,
) -> Result<Vec<u8>, DocumentError> {
    let base = font.name().as_bytes();
    for (name, value) in fonts.iter() {
        if let PdfObject::Dictionary(existing) = reader.resolve(value)?
            && name.starts_with(base)
            && entry(&existing, "BaseFont").and_then(PdfObject::as_name) == Some(base)
            && entry(&existing, "Widths").is_some()
            && (font.is_symbolic()
                || entry(&existing, "Encoding").and_then(PdfObject::as_name)
                    == Some(b"WinAnsiEncoding"))
        {
            return Ok(name.clone());
        }
    }
    let name = (1..)
        .map(|number: usize| match number {
            1 => base.to_vec(),
            _ => format!("{}{number}", font.name()).into_bytes(),
        })
        .find(|name| !fonts.contains_key(name.as_slice()))
        .unwrap_or_default();

    let named = |text: &str| PdfObject::Name(text.as_bytes().to_vec());
    let encoding = font.encoding();
    let widths: Vec<PdfObject> = (32..=255_u8)
        .map(|code| {
            let width = encoding
                .glyph_name(code)
                .and_then(|glyph| font.width(glyph));
            PdfObject::Integer(i64::from(width.unwrap_or(0)))
        })
        .collect();
    let mut dictionary = Dictionary::new();
    dictionary.insert(b"Type".to_vec(), named("Font"));
    dictionary.insert(b"Subtype".to_vec(), named("Type1"));
    dictionary.insert(b"BaseFont".to_vec(), named(font.name()));
    if !font.is_symbolic() {
        dictionary.insert(b"Encoding".to_vec(), named("WinAnsiEncoding"));
    }
    dictionary.insert(b"FirstChar".to_vec(), PdfObject::Integer(32));
    dictionary.insert(b"LastChar".to_vec(), PdfObject::Integer(255));
    dictionary.insert(b"Widths".to_vec(), PdfObject::Array(widths));
    let highest = reader.object_numbers().last().copied().unwrap_or(0);
    let reference = ObjectReference {
        number: highest.saturating_add(1),
        generation: 0,
    };
    reader.set_object(reference.number, PdfObject::Dictionary(dictionary), None);
    fonts.insert(name.clone(), PdfObject::Reference(reference));
    Ok(name)
}
