//! Redaction: removing what a page shows inside rectangles, as applying
//! redaction annotations does (ISO 32000-2 section 12.5.6.23).

use super::image;
use super::page_text::{
    Fonts, IDENTITY, Matrix, ShownGlyph, TextObject, TextState, multiply, page_resources,
    text_operation,
};
use super::reader::{Reader, entry};
use super::{DocumentError, Rectangle};
use crate::writer::write_object;
use crate::{Dictionary, ObjectReference, Operation, PdfObject, parse_content};
use std::collections::BTreeSet;

/// Deepest nesting of form XObjects that is rewritten.
const MAX_FORM_DEPTH: usize = 8;

/// What a redaction removed from a page.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Redaction {
    /// Glyphs removed, visible or not.
    pub glyphs: usize,
    /// Images removed, or stored again with their pixels inside the areas
    /// made white.
    pub images: usize,
    /// Paths removed, and form XObjects removed or rewritten.
    pub graphics: usize,
    /// Images partly inside an area that were left as they were: inline
    /// images, and images whose data this engine cannot decode.
    pub kept_images: usize,
}

/// A rectangle in page space as left, bottom, right, and top.
type Bounds = [f64; 4];

/// The graphics state that redaction follows; `q` saves it.
#[derive(Clone)]
struct Graphics {
    ctm: Matrix,
    text: TextState,
    line_width: f64,
    /// The fill color as red, green, and blue, which image masks paint in.
    fill: [f64; 3],
}

/// A content stream written again, with what its resources must gain and
/// may lose.
#[derive(Default)]
struct Rewritten {
    content: Vec<u8>,
    /// New XObjects, by the names the content now uses for them.
    added: Vec<(Vec<u8>, ObjectReference)>,
    /// XObject names the content still draws.
    used: BTreeSet<Vec<u8>>,
    /// XObject names whose drawing was left out or replaced.
    touched: BTreeSet<Vec<u8>>,
    /// Whether the content draws a form that uses these resources too,
    /// whose names are then all kept.
    shared: bool,
}

impl Rewritten {
    /// The names the resources should lose: those whose objects the
    /// content no longer draws, so that saving leaves them out of the file.
    fn unused(&self) -> Vec<Vec<u8>> {
        if self.shared {
            return Vec::new();
        }
        self.touched.difference(&self.used).cloned().collect()
    }
}

struct Redactor<'a> {
    reader: &'a mut Reader,
    areas: Vec<Bounds>,
    result: Redaction,
}

/// Redact the page with the given reference, whose decoded content is
/// `content`: write its content again without what lies inside the areas,
/// then paint each area with `fill`, if given.
pub(super) fn redact(
    reader: &mut Reader,
    page: ObjectReference,
    content: &[u8],
    areas: &[Rectangle],
    fill: Option<[f64; 3]>,
) -> Result<Redaction, DocumentError> {
    let areas: Vec<Bounds> = areas
        .iter()
        .map(|area| {
            let (left, right) = (area.left.min(area.right), area.left.max(area.right));
            let (bottom, top) = (area.bottom.min(area.top), area.bottom.max(area.top));
            [left, bottom, right, top]
        })
        .filter(|area| area.iter().all(|value| value.is_finite()))
        .filter(|[left, bottom, right, top]| right > left && top > bottom)
        .collect();
    let PdfObject::Dictionary(mut entries) = reader.resolve(&PdfObject::Reference(page))? else {
        return Err(DocumentError::Structure("a page is not a dictionary"));
    };
    let resources = page_resources(reader, page)?;
    let mut redactor = Redactor {
        reader,
        areas: areas.clone(),
        result: Redaction::default(),
    };
    let rewritten = redactor.rewrite(&resources, content, IDENTITY, 0)?;
    let result = redactor.result;

    // The page's own content is enclosed in `q` and `Q`, so that the areas
    // are painted in default user space whatever it leaves behind.
    let mut data = b"q\n".to_vec();
    data.extend(&rewritten.content);
    data.extend(b"Q\n");
    if let Some(color) = fill {
        let color = color.map(|value| PdfObject::Real(value.clamp(0.0, 1.0)));
        for [left, bottom, right, top] in &areas {
            write(&operation("q", Vec::new()), &mut data);
            write(&operation("rg", color.to_vec()), &mut data);
            let shape = [*left, *bottom, right - left, top - bottom].map(PdfObject::Real);
            write(&operation("re", shape.to_vec()), &mut data);
            write(&operation("f", Vec::new()), &mut data);
            write(&operation("Q", Vec::new()), &mut data);
        }
    }
    let unused = rewritten.unused();
    if !rewritten.added.is_empty() || !unused.is_empty() {
        let resources = with_objects(reader, &resources, rewritten.added, &unused)?;
        entries.insert(b"Resources".to_vec(), PdfObject::Dictionary(resources));
    }
    let stream = add_stream(reader, Dictionary::new(), &data);
    entries.insert(b"Contents".to_vec(), PdfObject::Reference(stream));
    reader.set_object(page.number, PdfObject::Dictionary(entries), None);
    Ok(result)
}

impl Redactor<'_> {
    /// Write one content stream again without what lies inside the areas.
    /// `start` maps the stream's user space to page space.
    fn rewrite(
        &mut self,
        resources: &Dictionary,
        content: &[u8],
        start: Matrix,
        depth: usize,
    ) -> Result<Rewritten, DocumentError> {
        let operations = parse_content(content).map_err(DocumentError::Syntax)?;
        let mut fonts = Fonts::new(self.reader, resources, false, None)?;
        let objects = match self.reader.resolve_entry(resources, "XObject")? {
            PdfObject::Dictionary(objects) => objects,
            _ => Dictionary::new(),
        };
        let mut rewritten = Rewritten::default();
        let mut state = Graphics {
            ctm: start,
            text: TextState::new(),
            line_width: 1.0,
            fill: [0.0; 3],
        };
        let mut saved: Vec<Graphics> = Vec::new();
        let mut object = TextObject::new();
        // A path is held until the operator that paints it, which decides
        // whether it stays.
        let mut path: Vec<&Operation> = Vec::new();
        let mut path_bounds: Option<Bounds> = None;
        let mut clip: Option<&Operation> = None;

        for operation in &operations {
            let operator = operation.operator.as_slice();
            let operands = &operation.operands;
            let numbers: Vec<f64> = operands.iter().filter_map(PdfObject::as_number).collect();
            match operator {
                b"m" | b"l" | b"c" | b"v" | b"y" | b"re" | b"h" => {
                    let points: Vec<(f64, f64)> = match (operator, numbers.as_slice()) {
                        (b"re", &[x, y, width, height]) => vec![
                            (x, y),
                            (x + width, y),
                            (x, y + height),
                            (x + width, y + height),
                        ],
                        (b"re", _) => Vec::new(),
                        _ => numbers
                            .as_chunks::<2>()
                            .0
                            .iter()
                            .map(|&[x, y]| (x, y))
                            .collect(),
                    };
                    for (x, y) in points {
                        let (x, y) = apply(&state.ctm, x, y);
                        path_bounds = Some(match path_bounds {
                            None => [x, y, x, y],
                            Some([left, bottom, right, top]) => {
                                [left.min(x), bottom.min(y), right.max(x), top.max(y)]
                            }
                        });
                    }
                    path.push(operation);
                    continue;
                }
                b"W" | b"W*" => {
                    clip = Some(operation);
                    continue;
                }
                b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"n" => {
                    let stroked = matches!(operator, b"S" | b"s" | b"B" | b"B*" | b"b" | b"b*");
                    let grow = if stroked {
                        state.line_width.abs() / 2.0 * scale(&state.ctm)
                    } else {
                        0.0
                    };
                    let covered = operator != b"n"
                        && path_bounds.is_some_and(|[left, bottom, right, top]| {
                            self.covers([left - grow, bottom - grow, right + grow, top + grow])
                        });
                    let out = &mut rewritten.content;
                    if covered {
                        // A path that also clips keeps its clip.
                        self.result.graphics += 1;
                        if let Some(clip) = clip {
                            path.iter().for_each(|operation| write(operation, out));
                            write(clip, out);
                            write(&self::operation("n", Vec::new()), out);
                        }
                    } else {
                        path.iter().for_each(|operation| write(operation, out));
                        clip.iter().for_each(|operation| write(operation, out));
                        write(operation, out);
                    }
                    path.clear();
                    path_bounds = None;
                    clip = None;
                    continue;
                }
                _ => {}
            }
            // Any other operator ends a path that was never painted, which
            // stays as it was.
            for operation in path.drain(..).chain(clip.take()) {
                write(operation, &mut rewritten.content);
            }
            path_bounds = None;

            // Each glyph a text operator shows: its code's length, its
            // width, and whether its middle lies inside an area.
            let mut shown: Vec<(usize, f64, bool)> = Vec::new();
            let (ctm, areas) = (state.ctm, &self.areas);
            let mut emit = |glyph: ShownGlyph<'_>| {
                let width = glyph.font.width(glyph.code);
                // The middle of the box text extraction gives a glyph.
                let (x, y) = apply(&glyph.start, width / 2.0, 0.3);
                let (x, y) = apply(&ctm, x, y);
                let inside = areas.iter().any(|area| contains(area, x, y));
                shown.push((glyph.code.len(), width, inside));
            };
            let text = &mut state.text;
            if text_operation(
                self.reader,
                &mut fonts,
                text,
                &mut object,
                operation,
                &mut emit,
            ) {
                let removed = shown.iter().filter(|glyph| glyph.2).count();
                match shown_again(operation, &shown, &state.text) {
                    Some(again) if removed > 0 => {
                        rewritten.content.extend(again);
                        self.result.glyphs += removed;
                    }
                    _ => write(operation, &mut rewritten.content),
                }
                continue;
            }

            match operator {
                b"q" => saved.push(state.clone()),
                b"Q" => match saved.pop() {
                    Some(previous) => state = previous,
                    // A `Q` without its `q` would undo the page's enclosing
                    // `q`; it is left out.
                    None => continue,
                },
                b"cm" => {
                    if let Ok(matrix) = <Matrix>::try_from(numbers.as_slice()) {
                        state.ctm = multiply(&matrix, &state.ctm);
                    }
                }
                b"w" => state.line_width = numbers.first().copied().unwrap_or(1.0),
                b"g" | b"rg" | b"k" | b"sc" | b"scn" if numbers.len() == operands.len() => {
                    let level = |value: f64| value.clamp(0.0, 1.0);
                    state.fill = match numbers[..] {
                        [gray] => [level(gray); 3],
                        [red, green, blue] => [red, green, blue].map(level),
                        [cyan, magenta, yellow, black] => [cyan, magenta, yellow]
                            .map(|ink| (1.0 - level(ink)) * (1.0 - level(black))),
                        _ => state.fill,
                    };
                }
                b"cs" => state.fill = [0.0; 3],
                b"Do" => {
                    if let Some(name) = operands.first().and_then(PdfObject::as_name) {
                        self.draw(name, &objects, resources, &state, depth, &mut rewritten)?;
                        continue;
                    }
                }
                b"BI" => {
                    let bounds = transform(&state.ctm, [0.0, 0.0, 1.0, 1.0]);
                    if self.covers(bounds) {
                        self.result.images += 1;
                        continue;
                    }
                    if self.overlaps(bounds) {
                        self.result.kept_images += 1;
                    }
                }
                _ => {}
            }
            write(operation, &mut rewritten.content);
        }
        for operation in path.drain(..).chain(clip) {
            write(operation, &mut rewritten.content);
        }
        // Close what the stream left open.
        for _ in &saved {
            write(&self::operation("Q", Vec::new()), &mut rewritten.content);
        }
        Ok(rewritten)
    }

    /// Write a `Do` operator for the XObject `name` again: an image or form
    /// inside an area is left out, and one partly inside is replaced by a
    /// redacted copy.
    fn draw(
        &mut self,
        name: &[u8],
        objects: &Dictionary,
        resources: &Dictionary,
        state: &Graphics,
        depth: usize,
        rewritten: &mut Rewritten,
    ) -> Result<(), DocumentError> {
        let keep = |rewritten: &mut Rewritten| {
            let original = operation("Do", vec![PdfObject::Name(name.to_vec())]);
            write(&original, &mut rewritten.content);
            rewritten.used.insert(name.to_vec());
        };
        let Some(PdfObject::Reference(reference)) = objects.get(name) else {
            keep(rewritten);
            return Ok(());
        };
        let PdfObject::Dictionary(dictionary) =
            self.reader.resolve(&PdfObject::Reference(*reference))?
        else {
            keep(rewritten);
            return Ok(());
        };
        let subtype = entry(&dictionary, "Subtype").and_then(PdfObject::as_name);
        if subtype == Some(b"Form") && entry(&dictionary, "Resources").is_none() {
            rewritten.shared = true;
        }
        let replacement = match subtype {
            Some(b"Image") => {
                let bounds = transform(&state.ctm, [0.0, 0.0, 1.0, 1.0]);
                if self.covers(bounds) {
                    self.result.images += 1;
                    rewritten.touched.insert(name.to_vec());
                    return Ok(());
                }
                if !self.overlaps(bounds) {
                    None
                } else {
                    let blanked = self.blank(reference.number, resources, state)?;
                    match blanked {
                        Some(_) => self.result.images += 1,
                        None => self.result.kept_images += 1,
                    }
                    blanked
                }
            }
            Some(b"Form") if depth < MAX_FORM_DEPTH => {
                let mut matrix = IDENTITY;
                if let PdfObject::Array(values) =
                    self.reader.resolve_entry(&dictionary, "Matrix")?
                    && let Ok(values) = <Matrix>::try_from(
                        values
                            .iter()
                            .filter_map(PdfObject::as_number)
                            .collect::<Vec<_>>()
                            .as_slice(),
                    )
                {
                    matrix = values;
                }
                let ctm = multiply(&matrix, &state.ctm);
                let bounds = match self.reader.resolve_entry(&dictionary, "BBox")? {
                    PdfObject::Array(values) => {
                        let values: Vec<f64> =
                            values.iter().filter_map(PdfObject::as_number).collect();
                        match values[..] {
                            [a, b, c, d] => {
                                Some(transform(&ctm, [a.min(c), b.min(d), a.max(c), b.max(d)]))
                            }
                            _ => None,
                        }
                    }
                    _ => None,
                };
                if bounds.is_some_and(|bounds| self.covers(bounds)) {
                    self.result.graphics += 1;
                    rewritten.touched.insert(name.to_vec());
                    return Ok(());
                }
                if bounds.is_some_and(|bounds| !self.overlaps(bounds)) {
                    None
                } else {
                    self.form(*reference, &dictionary, resources, ctm, depth)?
                }
            }
            _ => None,
        };
        match replacement {
            Some(replacement) => {
                let fresh = fresh_name(objects, &rewritten.added);
                let call = operation("Do", vec![PdfObject::Name(fresh.clone())]);
                write(&call, &mut rewritten.content);
                rewritten.touched.insert(name.to_vec());
                rewritten.used.insert(fresh.clone());
                rewritten.added.push((fresh, replacement));
            }
            None => keep(rewritten),
        }
        Ok(())
    }

    /// A redacted copy of a form XObject that `ctm` places on the page, or
    /// `None` when redaction removes nothing from it.
    fn form(
        &mut self,
        reference: ObjectReference,
        dictionary: &Dictionary,
        resources: &Dictionary,
        ctm: Matrix,
        depth: usize,
    ) -> Result<Option<ObjectReference>, DocumentError> {
        let Some(content) = self.reader.stream(reference.number)? else {
            return Ok(None);
        };
        // A form without resources of its own uses those of its parent.
        let own = match self.reader.resolve_entry(dictionary, "Resources")? {
            PdfObject::Dictionary(own) => own,
            _ => resources.clone(),
        };
        let before = self.result;
        let rewritten = self.rewrite(&own, &content, ctm, depth + 1)?;
        if self.result == before {
            return Ok(None);
        }
        self.result.graphics += 1;
        let mut copy = dictionary.clone();
        for key in ["Length", "Filter", "DecodeParms", "DL"] {
            copy.remove(key.as_bytes());
        }
        let unused = rewritten.unused();
        let own = with_objects(self.reader, &own, rewritten.added, &unused)?;
        copy.insert(b"Resources".to_vec(), PdfObject::Dictionary(own));
        Ok(Some(add_stream(self.reader, copy, &rewritten.content)))
    }

    /// A copy of an image whose pixels inside the areas are white, stored
    /// as 8-bit RGB with a soft mask for any transparency. `None` when the
    /// image cannot be decoded.
    fn blank(
        &mut self,
        number: u32,
        resources: &Dictionary,
        state: &Graphics,
    ) -> Result<Option<ObjectReference>, DocumentError> {
        let Some(mut picture) = image::load(self.reader, number, resources, state.fill, 0, None)?
        else {
            return Ok(None);
        };
        let (width, height) = (picture.width, picture.height);
        // Image space is the unit square, with the first row at its top.
        for (index, pixel) in picture.rgba.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let (row, column) = (index / width, index % width);
            let across = (column as f64 + 0.5) / width as f64;
            let up = 1.0 - (row as f64 + 0.5) / height as f64;
            let (x, y) = apply(&state.ctm, across, up);
            if self.areas.iter().any(|area| contains(area, x, y)) {
                *pixel = [255; 4];
            }
        }
        let pixels = picture.rgba.as_chunks::<4>().0;
        let color: Vec<u8> = pixels
            .iter()
            .flat_map(|pixel| [pixel[0], pixel[1], pixel[2]])
            .collect();
        let alpha: Vec<u8> = pixels.iter().map(|pixel| pixel[3]).collect();
        let image = |space: &str| {
            let mut dictionary = Dictionary::new();
            let name = |text: &str| PdfObject::Name(text.as_bytes().to_vec());
            let size = |value: usize| PdfObject::Integer(i64::try_from(value).unwrap_or(0));
            dictionary.insert(b"Type".to_vec(), name("XObject"));
            dictionary.insert(b"Subtype".to_vec(), name("Image"));
            dictionary.insert(b"Width".to_vec(), size(width));
            dictionary.insert(b"Height".to_vec(), size(height));
            dictionary.insert(b"ColorSpace".to_vec(), name(space));
            dictionary.insert(b"BitsPerComponent".to_vec(), PdfObject::Integer(8));
            dictionary
        };
        let mut dictionary = image("DeviceRGB");
        if alpha.iter().any(|&value| value != 255) {
            let mask = add_stream(self.reader, image("DeviceGray"), &alpha);
            dictionary.insert(b"SMask".to_vec(), PdfObject::Reference(mask));
        }
        Ok(Some(add_stream(self.reader, dictionary, &color)))
    }

    /// Whether one area holds all of a rectangle.
    fn covers(&self, [left, bottom, right, top]: Bounds) -> bool {
        self.areas
            .iter()
            .any(|area| area[0] <= left && right <= area[2] && area[1] <= bottom && top <= area[3])
    }

    /// Whether a rectangle and an area share some of their insides.
    fn overlaps(&self, [left, bottom, right, top]: Bounds) -> bool {
        self.areas
            .iter()
            .any(|area| left < area[2] && right > area[0] && bottom < area[3] && top > area[1])
    }
}

/// Write a text-showing operator again as `TJ` without the glyphs that
/// `shown` marks, each replaced by a position adjustment of its advance so
/// that the glyphs after it stay where they were. Returns `None` when the
/// operator's strings do not match the glyphs shown.
fn shown_again(
    operation: &Operation,
    shown: &[(usize, f64, bool)],
    text: &TextState,
) -> Option<Vec<u8>> {
    let operands = &operation.operands;
    let parts = match operands.last()? {
        PdfObject::String(bytes) => vec![PdfObject::String(bytes.clone())],
        PdfObject::Array(parts) => parts.clone(),
        _ => return None,
    };
    let mut glyphs = shown.iter();
    let mut array: Vec<PdfObject> = Vec::new();
    let mut run: Vec<u8> = Vec::new();
    // Adjustments in a row are added together.
    let adjust = |array: &mut Vec<PdfObject>, run: &mut Vec<u8>, value: f64| {
        if !run.is_empty() {
            array.push(PdfObject::String(std::mem::take(run)));
        }
        match array.last_mut() {
            Some(last @ (PdfObject::Real(_) | PdfObject::Integer(_))) => {
                *last = PdfObject::Real(last.as_number().unwrap_or(0.0) + value);
            }
            _ => array.push(PdfObject::Real(value)),
        }
    };
    for part in &parts {
        let PdfObject::String(bytes) = part else {
            if let Some(value) = part.as_number() {
                adjust(&mut array, &mut run, value);
            }
            continue;
        };
        let mut rest = bytes.as_slice();
        while !rest.is_empty() {
            let &(length, width, inside) = glyphs.next()?;
            let (code, tail) = rest.split_at(length.clamp(1, rest.len()));
            rest = tail;
            if !inside {
                run.extend(code);
                continue;
            }
            let mut advance = width * text.size + text.character_spacing;
            if code == b" " {
                advance += text.word_spacing;
            }
            // An adjustment moves by a thousandth of the font size.
            if text.size != 0.0 {
                adjust(&mut array, &mut run, -advance * 1000.0 / text.size);
            }
        }
    }
    if glyphs.next().is_some() {
        return None;
    }
    if !run.is_empty() {
        array.push(PdfObject::String(run));
    }
    let mut out = Vec::new();
    match operation.operator.as_slice() {
        b"'" => write(&self::operation("T*", Vec::new()), &mut out),
        // The double quote sets the word and character spacing first.
        [0x22] => {
            let [word, character] = [operands.first()?, operands.get(1)?];
            write(&self::operation("Tw", vec![word.clone()]), &mut out);
            write(&self::operation("Tc", vec![character.clone()]), &mut out);
            write(&self::operation("T*", Vec::new()), &mut out);
        }
        _ => {}
    }
    write(
        &self::operation("TJ", vec![PdfObject::Array(array)]),
        &mut out,
    );
    Some(out)
}

pub(super) fn operation(operator: &str, operands: Vec<PdfObject>) -> Operation {
    Operation {
        operator: operator.as_bytes().to_vec(),
        operands,
    }
}

/// Write an operation in content stream syntax, on a line of its own.
pub(super) fn write(operation: &Operation, out: &mut Vec<u8>) {
    if operation.operator == b"BI" {
        // The parser gives an inline image's data as its last operand.
        let (data, entries) = match operation.operands.split_last() {
            Some((PdfObject::String(data), entries)) => (data.as_slice(), entries),
            _ => (&[][..], operation.operands.as_slice()),
        };
        out.extend(b"BI");
        for entry in entries {
            out.push(b' ');
            write_object(entry, out);
        }
        out.extend(b" ID ");
        out.extend(data);
        out.extend(b"\nEI\n");
        return;
    }
    for operand in &operation.operands {
        write_object(operand, out);
        out.push(b' ');
    }
    out.extend(&operation.operator);
    out.push(b'\n');
}

/// Where a matrix takes a point.
fn apply(matrix: &Matrix, x: f64, y: f64) -> (f64, f64) {
    (
        x * matrix[0] + y * matrix[2] + matrix[4],
        x * matrix[1] + y * matrix[3] + matrix[5],
    )
}

/// The bounds of a rectangle that a matrix has moved.
fn transform(matrix: &Matrix, [left, bottom, right, top]: Bounds) -> Bounds {
    let corners = [(left, bottom), (right, bottom), (left, top), (right, top)];
    let corners = corners.map(|(x, y)| apply(matrix, x, y));
    let across = corners.map(|corner| corner.0);
    let up = corners.map(|corner| corner.1);
    [
        across.into_iter().fold(f64::INFINITY, f64::min),
        up.into_iter().fold(f64::INFINITY, f64::min),
        across.into_iter().fold(f64::NEG_INFINITY, f64::max),
        up.into_iter().fold(f64::NEG_INFINITY, f64::max),
    ]
}

/// The most a matrix stretches a length in any of its two directions.
fn scale(matrix: &Matrix) -> f64 {
    matrix[0].hypot(matrix[1]).max(matrix[2].hypot(matrix[3]))
}

fn contains(area: &Bounds, x: f64, y: f64) -> bool {
    (area[0]..=area[2]).contains(&x) && (area[1]..=area[3]).contains(&y)
}

/// A resource name for a new XObject that neither the resources nor the
/// names added so far use.
fn fresh_name(objects: &Dictionary, added: &[(Vec<u8>, ObjectReference)]) -> Vec<u8> {
    (1..)
        .map(|number: usize| format!("Redacted{number}").into_bytes())
        .find(|name| {
            !objects.contains_key(name.as_slice()) && added.iter().all(|(used, _)| used != name)
        })
        .unwrap_or_default()
}

/// A copy of resources whose XObject dictionary also has the names added
/// and no longer has the names unused.
fn with_objects(
    reader: &mut Reader,
    resources: &Dictionary,
    added: Vec<(Vec<u8>, ObjectReference)>,
    unused: &[Vec<u8>],
) -> Result<Dictionary, DocumentError> {
    let mut resources = resources.clone();
    let mut objects = match reader.resolve_entry(&resources, "XObject")? {
        PdfObject::Dictionary(objects) => objects,
        _ => Dictionary::new(),
    };
    for name in unused {
        objects.remove(name.as_slice());
    }
    for (name, reference) in added {
        objects.insert(name, PdfObject::Reference(reference));
    }
    resources.insert(b"XObject".to_vec(), PdfObject::Dictionary(objects));
    Ok(resources)
}

/// Add a stream of plain data, compressed with `FlateDecode`, under a new
/// object number.
pub(super) fn add_stream(
    reader: &mut Reader,
    mut dictionary: Dictionary,
    data: &[u8],
) -> ObjectReference {
    let filter = PdfObject::Name(b"FlateDecode".to_vec());
    dictionary.insert(b"Filter".to_vec(), filter);
    let highest = reader.object_numbers().last().copied().unwrap_or(0);
    let reference = ObjectReference {
        number: highest.saturating_add(1),
        generation: 0,
    };
    let data = crate::filter::deflate_zlib(data);
    reader.set_object(
        reference.number,
        PdfObject::Dictionary(dictionary),
        Some(data),
    );
    reference
}
