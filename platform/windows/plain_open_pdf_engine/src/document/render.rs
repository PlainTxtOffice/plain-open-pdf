//! Rendering a page's vector graphics: paths that are filled, stroked, and
//! clipped (ISO 32000-2 sections 8.4 to 8.6), in any color space, with
//! colors, shadings, and tiling patterns; text whose font has a program or
//! can use a fallback font; and image XObjects.

use super::color::{self, Space};
use super::image::{self, Image};
use super::page_text::{
    Fonts, IDENTITY, Matrix, ShownGlyph, TextObject, TextState, Type3Font, multiply,
    page_resources, text_operation,
};
use super::pages::Page;
use super::reader::{Reader, entry};
use super::shading::{Shading, invert};
use super::{DocumentError, PixelRegion};
use crate::font::{Segment, TrueTypeFont};
use crate::raster::{
    BlendMode, Canvas, FillRule, Mask, Pixmap, Placed, Point, Window, coverage, reach, to_byte_f64,
};
use crate::{Dictionary, ObjectReference, PdfObject, parse_content};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

/// How deeply form XObjects may nest before they are left out.
const MAX_FORM_DEPTH: usize = 8;
/// The most pixels along one side of a rendered page, and in all.
const MAX_SIDE: f64 = 16_384.0;
const MAX_PIXELS: f64 = 64_000_000.0;
/// The thinnest line drawn, in pixels, so that hairlines stay visible.
const MIN_LINE_WIDTH: f64 = 0.6;
/// The most tiles of a tiling pattern drawn for one fill.
const MAX_TILES: usize = 4096;

/// A pattern to paint with (ISO 32000-2 section 8.7).
enum Paint {
    /// A shading pattern: the shading, and the matrix from its space to
    /// device space.
    Shading {
        shading: Box<Shading>,
        matrix: Matrix,
    },
    /// A tiling pattern: a cell drawn again and again.
    Tiling {
        content: Vec<u8>,
        resources: Dictionary,
        bounds: [f64; 4],
        step: (f64, f64),
        /// From pattern space to device space.
        matrix: Matrix,
        /// For an uncolored pattern, the color its cell is drawn in.
        color: Option<[f64; 3]>,
    },
}

#[derive(Clone)]
struct State {
    ctm: Matrix,
    /// The matrix the content stream started with, which patterns are
    /// placed by.
    base: Matrix,
    fill_space: Rc<Space>,
    stroke_space: Rc<Space>,
    /// The color, or `None` when painting with it draws nothing.
    fill: Option<[f64; 3]>,
    stroke: Option<[f64; 3]>,
    /// The pattern painted in place of a color.
    fill_pattern: Option<Rc<Paint>>,
    stroke_pattern: Option<Rc<Paint>>,
    /// Whether color operators are ignored, as in an uncolored pattern's
    /// cell.
    color_locked: bool,
    /// How painted colors combine with what is beneath them.
    blend: BlendMode,
    fill_alpha: f64,
    stroke_alpha: f64,
    line_width: f64,
    /// 0 for butt caps, 1 for round, 2 for projecting square.
    line_cap: i64,
    dash: Vec<f64>,
    dash_phase: f64,
    /// How much of each pixel the clipping path lets through.
    clip: Option<Rc<Mask>>,
    text: TextState,
}

/// A subpath in device space, and whether it was closed.
type Subpath = (Vec<Point>, bool);

/// A shape to paint: its coverage within the window, and the polygons that
/// coverage came from, which say how far it reaches over the whole page.
struct Shape<'a> {
    mask: &'a Mask,
    polygons: &'a [Vec<Point>],
}

struct Renderer<'a> {
    reader: &'a mut Reader,
    /// The pixels drawn: those of `window`.
    canvas: Canvas,
    /// The part of the page image that is drawn. Coverage is found only
    /// within it; everything is still placed in page coordinates.
    window: Window,
    /// The whole page image, for what must not depend on the window.
    page: Window,
    /// Draws text whose own font has no TrueType program.
    fallback: Option<&'a TrueTypeFont>,
    /// Glyphs already turned into coverage by this content stream.
    glyphs: GlyphCache,
}

/// Glyphs turned into coverage, and roughly how many bytes they hold.
#[derive(Default)]
struct GlyphCache {
    entries: HashMap<GlyphKey, Option<Rc<GlyphCoverage>>>,
    bytes: usize,
}

/// The most bytes a glyph cache holds before it starts afresh.
const MAX_GLYPH_BYTES: usize = 32 << 20;

/// Glyphs whose origin is farther than this many pixels from the page's
/// corner are drawn from their outline: so far away, a quarter of a pixel
/// cannot be told apart.
const FAR_GLYPH: f64 = (1_u64 << 40) as f64;

/// Glyphs larger than this many pixels across or down are drawn from
/// their outline each time, not kept.
const MAX_KEPT_GLYPH: f64 = 256.0;

/// How finely a kept glyph's position is told apart: a quarter of a pixel.
const GLYPH_STEPS: f64 = 4.0;

/// What a glyph's coverage depends on: its outline, the matrix's scale,
/// slant, and turn, and where in a pixel its origin falls, in quarters.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct GlyphKey {
    outline: usize,
    matrix: [u64; 4],
    across: u8,
    down: u8,
}

/// A glyph's coverage, placed relative to the pixel its origin is in.
struct GlyphCoverage {
    /// Keeps the outline, and so the address the key names, alive.
    _outline: Arc<Vec<Segment>>,
    left: i64,
    top: i64,
    width: usize,
    height: usize,
    data: Vec<u8>,
}

/// A glyph outline and the matrix from its font units to device space.
type PlacedGlyph = (Arc<Vec<Segment>>, Matrix);

/// Render a page at `scale` pixels for each point: the whole page image,
/// or the part of it inside `region`, which holds the same pixels as that
/// part of the whole image.
pub(super) fn render(
    reader: &mut Reader,
    page: &Page,
    content: &[u8],
    scale: f64,
    fallback: Option<&TrueTypeFont>,
    region: Option<PixelRegion>,
) -> Result<Pixmap, DocumentError> {
    let area = page.crop_box;
    let (width, height) = (area.width() * scale, area.height() * scale);
    let fits = |side: f64| side.is_finite() && (1.0..=MAX_SIDE).contains(&side.ceil());
    if !(scale.is_finite() && scale > 0.0)
        || !fits(width)
        || !fits(height)
        || width * height > MAX_PIXELS
    {
        return Err(DocumentError::Unsupported("a page image of this size"));
    }
    // The image is a whole number of pixels each way, and the page is
    // stretched to fill it, by less than a pixel: a page that stopped
    // partway through its last column or row would leave a pale line there.
    let (width, height) = (width.ceil(), height.ceil());
    let (across, down) = (width / area.width(), height / area.height());
    // Device space has its origin at the top left, so the page is flipped,
    // and then turned by the page's rotation.
    let upright = [
        across,
        0.0,
        0.0,
        -down,
        -area.left * across,
        area.top * down,
    ];
    let (turn, sideways) = match page.rotation {
        90 => ([0.0, 1.0, -1.0, 0.0, height, 0.0], true),
        180 => ([-1.0, 0.0, 0.0, -1.0, width, height], false),
        270 => ([0.0, -1.0, 1.0, 0.0, 0.0, width], true),
        _ => (IDENTITY, false),
    };
    let (width, height) = if sideways {
        (height, width)
    } else {
        (width, height)
    };
    let whole = Window::whole(width.ceil() as usize, height.ceil() as usize);
    // The limits above are on the whole page, so a region is drawn exactly
    // when the page is. The matrix is the whole page's too: only the window
    // differs.
    let window = match region {
        Some(region) => Window {
            left: region.left,
            top: region.top,
            right: region.left.saturating_add(region.width).min(whole.right),
            bottom: region.top.saturating_add(region.height).min(whole.bottom),
        },
        None => whole,
    };
    if window.width() == 0 || window.height() == 0 {
        return Err(DocumentError::Unsupported("a page region outside the page"));
    }
    let mut renderer = Renderer {
        reader,
        canvas: Canvas::new(&window),
        window,
        page: whole,
        fallback,
        glyphs: GlyphCache::default(),
    };
    let state = State::new(multiply(&upright, &turn), None);
    if let Some(reference) = page.reference {
        let resources = page_resources(renderer.reader, reference)?;
        renderer.run(&resources, content, state.clone(), 0)?;
        // An annotation that cannot be drawn is left out.
        let _ = renderer.annotations(reference, &resources, &state);
    }
    Ok(renderer.canvas.into_image())
}

impl State {
    /// The graphics state a content stream starts with.
    fn new(ctm: Matrix, clip: Option<Rc<Mask>>) -> Self {
        Self {
            ctm,
            base: ctm,
            fill_space: Rc::new(Space::Gray),
            stroke_space: Rc::new(Space::Gray),
            fill: Some([0.0; 3]),
            stroke: Some([0.0; 3]),
            fill_pattern: None,
            stroke_pattern: None,
            color_locked: false,
            blend: BlendMode::Normal,
            fill_alpha: 1.0,
            stroke_alpha: 1.0,
            line_width: 1.0,
            line_cap: 0,
            dash: Vec::new(),
            dash_phase: 0.0,
            clip,
            text: TextState::new(),
        }
    }
}

fn transform(matrix: &Matrix, x: f64, y: f64) -> Point {
    (
        x * matrix[0] + y * matrix[2] + matrix[4],
        x * matrix[1] + y * matrix[3] + matrix[5],
    )
}

/// Find the outline for a shown glyph and where it goes on the page.
///
/// A font with a TrueType, CFF, or Type 1 program draws its own glyph. Any
/// other font is
/// drawn with the fallback font's glyph for the same character, narrowed
/// when it is wider than the space the page gives it.
fn place_glyph(
    glyph: &ShownGlyph<'_>,
    ctm: &Matrix,
    fallback: Option<&TrueTypeFont>,
) -> Option<PlacedGlyph> {
    // A font with a CFF program draws its own glyph too.
    if let Some(program) = glyph.font.cff.as_deref() {
        let unit = 1.0 / program.units_per_em();
        let to_text = [unit, 0.0, 0.0, unit, 0.0, 0.0];
        let matrix = multiply(&to_text, &multiply(&glyph.start, ctm));
        return Some((program.outline(glyph.font.cff_glyph(glyph.code)?), matrix));
    }
    // So does a font with a Type 1 program.
    if let Some(program) = glyph.font.type1.as_deref() {
        let unit = 1.0 / program.units_per_em();
        let to_text = [unit, 0.0, 0.0, unit, 0.0, 0.0];
        let matrix = multiply(&to_text, &multiply(&glyph.start, ctm));
        return Some((program.outline(glyph.font.type1_glyph(glyph.code)?), matrix));
    }
    let own = glyph.font.program.as_deref();
    let (program, index, narrow) = match own {
        Some(program) => (program, glyph.font.glyph(glyph.code)?, 1.0),
        None => {
            let program = fallback?;
            let character = u32::from(glyph.font.text(glyph.code).chars().next()?);
            let index = program
                .glyph_index(3, 10, character)
                .or_else(|| program.glyph_index(3, 1, character))
                .or_else(|| program.glyph_index(0, 3, character))?;
            let given = glyph.font.width(glyph.code) * f64::from(program.units_per_em());
            let natural = program.advance(index).map_or(0.0, f64::from);
            let narrow = if given > 0.0 && natural > given {
                given / natural
            } else {
                1.0
            };
            (program, index, narrow)
        }
    };
    let unit = 1.0 / f64::from(program.units_per_em());
    let to_text = [unit * narrow, 0.0, 0.0, unit, 0.0, 0.0];
    let matrix = multiply(&to_text, &multiply(&glyph.start, ctm));
    Some((program.outline(index), matrix))
}

/// Add a glyph outline's contours, in device space, to `polygons`.
fn outline_polygons(outline: &[Segment], matrix: &Matrix, polygons: &mut Vec<Vec<Point>>) {
    let mut current = (0.0, 0.0);
    for segment in outline {
        match *segment {
            Segment::Move(x, y) => {
                current = transform(matrix, x, y);
                polygons.push(vec![current]);
            }
            Segment::Line(x, y) => {
                current = transform(matrix, x, y);
                if let Some(polygon) = polygons.last_mut() {
                    polygon.push(current);
                }
            }
            Segment::Cubic(first_x, first_y, second_x, second_y, x, y) => {
                let first = transform(matrix, first_x, first_y);
                let second = transform(matrix, second_x, second_y);
                let to = transform(matrix, x, y);
                if let Some(polygon) = polygons.last_mut() {
                    flatten(polygon, current, first, second, to);
                }
                current = to;
            }
            Segment::Quad(control_x, control_y, x, y) => {
                // The same curve with two control points.
                let control = transform(matrix, control_x, control_y);
                let to = transform(matrix, x, y);
                let toward = |end: Point| {
                    (
                        end.0 + (control.0 - end.0) * 2.0 / 3.0,
                        end.1 + (control.1 - end.1) * 2.0 / 3.0,
                    )
                };
                if let Some(polygon) = polygons.last_mut() {
                    flatten(polygon, current, toward(current), toward(to), to);
                }
                current = to;
            }
            Segment::Close => {}
        }
    }
}

/// A glyph's coverage with its origin at `matrix`'s translation, within
/// the pixel at the origin, or `None` when it is too large to keep.
fn glyph_coverage(outline: &Arc<Vec<Segment>>, matrix: &Matrix) -> Option<GlyphCoverage> {
    let mut polygons = Vec::new();
    outline_polygons(outline, matrix, &mut polygons);
    let points = polygons.iter().flatten();
    let (mut low_x, mut low_y) = (f64::INFINITY, f64::INFINITY);
    let (mut high_x, mut high_y) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    for &(x, y) in points {
        low_x = low_x.min(x);
        low_y = low_y.min(y);
        high_x = high_x.max(x);
        high_y = high_y.max(y);
    }
    let empty = GlyphCoverage {
        _outline: Arc::clone(outline),
        left: 0,
        top: 0,
        width: 0,
        height: 0,
        data: Vec::new(),
    };
    if !(low_x.is_finite() && low_y.is_finite() && high_x.is_finite() && high_y.is_finite()) {
        return Some(empty);
    }
    if high_x - low_x > MAX_KEPT_GLYPH || high_y - low_y > MAX_KEPT_GLYPH {
        return None;
    }
    // Shift the glyph so that its box starts at the origin of a small
    // image just large enough for it.
    let (left, top) = (low_x.floor(), low_y.floor());
    for polygon in &mut polygons {
        for point in polygon.iter_mut() {
            *point = (point.0 - left, point.1 - top);
        }
    }
    let width = (high_x - left).ceil() as usize + 1;
    let height = (high_y - top).ceil() as usize + 1;
    let mask = coverage(&Window::whole(width, height), &polygons, FillRule::NonZero);
    Some(GlyphCoverage {
        left: left as i64 + mask.left as i64,
        top: top as i64 + mask.top as i64,
        width: mask.width,
        height: mask.height,
        data: mask.data,
        ..empty
    })
}

/// Let a mask through only where another does: each cell times the
/// other's coverage there.
fn narrow(mask: &mut Mask, other: &Mask) {
    for row in 0..mask.height {
        let (left, others) = other.row(mask.top + row);
        let cells = &mut mask.data[row * mask.width..(row + 1) * mask.width];
        // The cells the other mask's row reaches; the rest show nothing.
        let from = left.saturating_sub(mask.left).min(cells.len());
        let to = (left + others.len())
            .saturating_sub(mask.left)
            .clamp(from, cells.len());
        cells[..from].fill(0);
        cells[to..].fill(0);
        let others = &others[(mask.left + from - left).min(others.len())..];
        // Eight cells at a time: wholly open leaves them as they are, and
        // wholly shut empties them, as the product would.
        for (cells, others) in cells[from..to].chunks_mut(8).zip(others.chunks(8)) {
            if others.iter().all(|&through| through == 255) {
                continue;
            }
            if others.iter().all(|&through| through == 0) {
                cells.fill(0);
                continue;
            }
            for (cell, &through) in cells.iter_mut().zip(others) {
                *cell = (u32::from(*cell) * u32::from(through) / 255) as u8;
            }
        }
    }
    // The runs still hold every cell with coverage: narrowing only empties.
}

/// Split a cubic curve into lines, more of them the longer it is.
fn flatten(path: &mut Vec<Point>, from: Point, first: Point, second: Point, to: Point) {
    let length = |a: Point, b: Point| (b.0 - a.0).hypot(b.1 - a.1);
    let outline = length(from, first) + length(first, second) + length(second, to);
    if !outline.is_finite() {
        return;
    }
    let steps = (outline.sqrt() * 1.5).ceil().clamp(2.0, 160.0) as usize;
    for step in 1..=steps {
        let t = step as f64 / steps as f64;
        let s = 1.0 - t;
        let (a, b, c, d) = (s * s * s, 3.0 * s * s * t, 3.0 * s * t * t, t * t * t);
        path.push((
            a * from.0 + b * first.0 + c * second.0 + d * to.0,
            a * from.1 + b * first.1 + c * second.1 + d * to.1,
        ));
    }
}

/// Cut a line of points into the dashes of a dash pattern, in pixels.
fn dashes(points: &[Point], pattern: &[f64], phase: f64) -> Vec<Vec<Point>> {
    let period: f64 = pattern.iter().sum();
    let unusable = period.is_nan() || period <= 0.0 || pattern.iter().any(|&length| length < 0.0);
    if pattern.is_empty() || unusable {
        return vec![points.to_vec()];
    }
    // Find the pattern element the phase falls in.
    let mut index = 0;
    let mut left = pattern[0];
    // An odd pattern swaps dashes and gaps each time around, so two
    // rounds make one full cycle.
    let mut skip = phase.max(0.0) % (period * 2.0);
    while skip >= left {
        skip -= left;
        index += 1;
        left = pattern[index % pattern.len()];
    }
    left -= skip;
    let mut result = Vec::new();
    let mut dash: Vec<Point> = Vec::new();
    for pair in points.windows(2) {
        let (mut from, to) = (pair[0], pair[1]);
        let mut remaining = (to.0 - from.0).hypot(to.1 - from.1);
        if index % 2 == 0 && dash.is_empty() {
            dash.push(from);
        }
        while remaining > left {
            let t = left / remaining;
            from = (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
            remaining -= left;
            if index % 2 == 0 {
                dash.push(from);
                result.push(std::mem::take(&mut dash));
            } else {
                dash.push(from);
            }
            index += 1;
            left = pattern[index % pattern.len()].max(1e-6);
            if result.len() > 100_000 {
                return result;
            }
        }
        left -= remaining;
        if index % 2 == 0 {
            dash.push(to);
        }
    }
    if dash.len() > 1 {
        result.push(dash);
    }
    result
}

/// A circle as a polygon, for round joins and caps.
fn disc(center: Point, radius: f64) -> Vec<Point> {
    let sides = (radius * 3.0).ceil().clamp(8.0, 48.0) as usize;
    (0..sides)
        .map(|side| {
            let angle = std::f64::consts::TAU * side as f64 / sides as f64;
            (
                center.0 + radius * angle.cos(),
                center.1 + radius * angle.sin(),
            )
        })
        .collect()
}

/// The outline of a stroke, as polygons to fill with the nonzero rule: a
/// rectangle along each line and a disc at each corner.
fn stroke_outline(lines: &[(Vec<Point>, bool)], width: f64, cap: i64) -> Vec<Vec<Point>> {
    let half = width / 2.0;
    let mut polygons = Vec::new();
    for (points, closed) in lines {
        let mut points = points.clone();
        points.dedup();
        if *closed && points.len() > 1 && points.first() != points.last() {
            points.push(points[0]);
        }
        if points.len() == 1 && cap == 1 {
            polygons.push(disc(points[0], half));
        }
        let last = points.len().saturating_sub(2);
        for (index, pair) in points.windows(2).enumerate() {
            let (mut from, mut to) = (pair[0], pair[1]);
            let length = (to.0 - from.0).hypot(to.1 - from.1);
            let (along_x, along_y) = ((to.0 - from.0) / length, (to.1 - from.1) / length);
            // Corners between lines are rounded; ends follow the line cap.
            let open_start = index == 0 && !closed;
            let open_end = index == last && !closed;
            let join = (index > 0 || *closed) && half > 0.75;
            if join || (open_start && cap == 1) {
                polygons.push(disc(from, half));
            }
            if open_end && cap == 1 {
                polygons.push(disc(to, half));
            }
            if cap == 2 {
                if open_start {
                    from = (from.0 - along_x * half, from.1 - along_y * half);
                }
                if open_end {
                    to = (to.0 + along_x * half, to.1 + along_y * half);
                }
            }
            let (side_x, side_y) = (-along_y * half, along_x * half);
            // This order winds the same way as the discs.
            polygons.push(vec![
                (from.0 - side_x, from.1 - side_y),
                (to.0 - side_x, to.1 - side_y),
                (to.0 + side_x, to.1 + side_y),
                (from.0 + side_x, from.1 + side_y),
            ]);
        }
    }
    polygons
}

impl Renderer<'_> {
    /// Run a content stream with its own glyph cache: each run loads its
    /// own fonts, so the glyphs of one are no use to another.
    fn run(
        &mut self,
        resources: &Dictionary,
        content: &[u8],
        state: State,
        depth: usize,
    ) -> Result<(), DocumentError> {
        let outer = std::mem::take(&mut self.glyphs);
        let result = self.run_content(resources, content, state, depth);
        self.glyphs = outer;
        result
    }

    fn run_content(
        &mut self,
        resources: &Dictionary,
        content: &[u8],
        mut state: State,
        depth: usize,
    ) -> Result<(), DocumentError> {
        let mut saved: Vec<State> = Vec::new();
        let mut path: Vec<Subpath> = Vec::new();
        // The current point, and the start of the current subpath.
        let mut current = (0.0, 0.0);
        let mut start = (0.0, 0.0);
        let mut pending_clip: Option<FillRule> = None;
        let mut fonts = Fonts::new(self.reader, resources, true, self.fallback)?;
        let mut object = TextObject::new();

        let operations = parse_content(content).map_err(DocumentError::Syntax)?;
        for operation in &operations {
            let mut shown: Vec<PlacedGlyph> = Vec::new();
            // Type 3 glyphs: the font, the glyph's stream, and the matrix
            // from glyph space to device space.
            let mut procedures: Vec<(Rc<Type3Font>, ObjectReference, Matrix)> = Vec::new();
            let (ctm, fallback) = (state.ctm, self.fallback);
            let mut emit = |glyph: ShownGlyph<'_>| {
                if let Some(font) = &glyph.font.type3 {
                    if let Some(reference) = glyph.font.type3_procedure(glyph.code) {
                        let matrix = multiply(&font.matrix, &multiply(&glyph.start, &ctm));
                        procedures.push((Rc::clone(font), reference, matrix));
                    }
                } else if let Some(placed) = place_glyph(&glyph, &ctm, fallback) {
                    shown.push(placed);
                }
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
                self.text(&shown, &state, depth);
                for (font, reference, matrix) in procedures {
                    let resources = font.resources.as_ref().unwrap_or(resources);
                    // A glyph that cannot be drawn is left out.
                    let _ = self.type3_glyph(reference, matrix, resources, &state, depth);
                }
                continue;
            }
            let operands = &operation.operands;
            let numbers: Vec<f64> = operands.iter().filter_map(PdfObject::as_number).collect();
            let number = |index: usize| numbers.get(index).copied().unwrap_or(0.0);
            let point = |index: usize| transform(&state.ctm, number(index), number(index + 1));
            let name = operands.last().and_then(PdfObject::as_name);
            let operator = operation.operator.as_slice();
            match operator {
                b"q" => saved.push(state.clone()),
                b"Q" => {
                    if let Some(previous) = saved.pop() {
                        state = previous;
                    }
                }
                b"cm" if numbers.len() == 6 => {
                    let matrix = [0, 1, 2, 3, 4, 5].map(number);
                    state.ctm = multiply(&matrix, &state.ctm);
                }
                b"w" => state.line_width = number(0),
                b"J" => state.line_cap = number(0) as i64,
                b"d" => {
                    if let Some(PdfObject::Array(pattern)) = operands.first() {
                        state.dash = pattern.iter().filter_map(PdfObject::as_number).collect();
                        state.dash_phase = number(0);
                    }
                }
                b"gs" => {
                    if let Some(name) = name {
                        self.graphics_state(resources, name, &mut state)?;
                    }
                }

                b"m" => {
                    current = point(0);
                    start = current;
                    path.push((vec![current], false));
                }
                b"l" | b"c" | b"v" | b"y" => {
                    if path.is_empty() {
                        path.push((vec![current], false));
                    }
                    let Some((points, _)) = path.last_mut() else {
                        continue;
                    };
                    let from = current;
                    match operator {
                        b"l" => {
                            current = point(0);
                            points.push(current);
                        }
                        b"c" => {
                            current = point(4);
                            flatten(points, from, point(0), point(2), current);
                        }
                        b"v" => {
                            current = point(2);
                            flatten(points, from, from, point(0), current);
                        }
                        _ => {
                            current = point(2);
                            flatten(points, from, point(0), current, current);
                        }
                    }
                }
                b"h" => {
                    if let Some((_, closed)) = path.last_mut() {
                        *closed = true;
                        current = start;
                    }
                }
                b"re" => {
                    let (x, y, width, height) = (number(0), number(1), number(2), number(3));
                    let corner = |x, y| transform(&state.ctm, x, y);
                    current = corner(x, y);
                    start = current;
                    let corners = vec![
                        current,
                        corner(x + width, y),
                        corner(x + width, y + height),
                        corner(x, y + height),
                    ];
                    path.push((corners, true));
                }

                b"W" => pending_clip = Some(FillRule::NonZero),
                b"W*" => pending_clip = Some(FillRule::EvenOdd),
                b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"n" => {
                    if matches!(operator, b"s" | b"b" | b"b*")
                        && let Some((_, closed)) = path.last_mut()
                    {
                        *closed = true;
                    }
                    let rule = match operator {
                        b"f" | b"F" | b"B" | b"b" => Some(FillRule::NonZero),
                        b"f*" | b"B*" | b"b*" => Some(FillRule::EvenOdd),
                        _ => None,
                    };
                    if let Some(rule) = rule {
                        self.fill(&path, rule, &state, depth);
                    }
                    if !matches!(operator, b"f" | b"F" | b"f*" | b"n") {
                        self.stroke(&path, &state, depth);
                    }
                    if let Some(rule) = pending_clip.take() {
                        self.clip(&path, rule, &mut state);
                    }
                    path.clear();
                }

                // In an uncolored pattern's cell, colors come from outside.
                b"g" | b"G" | b"rg" | b"RG" | b"k" | b"K" | b"cs" | b"CS" | b"sc" | b"SC"
                | b"scn" | b"SCN"
                    if state.color_locked => {}
                b"g" | b"G" | b"rg" | b"RG" | b"k" | b"K" => {
                    let space = match operator {
                        b"g" | b"G" => Space::Gray,
                        b"rg" | b"RG" => Space::Rgb,
                        _ => Space::Cmyk,
                    };
                    let color = Some(space.rgb(&numbers));
                    if operator.iter().all(u8::is_ascii_lowercase) {
                        (state.fill_space, state.fill, state.fill_pattern) =
                            (Rc::new(space), color, None);
                    } else {
                        (state.stroke_space, state.stroke, state.stroke_pattern) =
                            (Rc::new(space), color, None);
                    }
                }
                b"cs" | b"CS" => {
                    let space = match name {
                        Some(name) => {
                            let name = PdfObject::Name(name.to_vec());
                            color::load(self.reader, &name, resources, 0)?.unwrap_or(Space::Gray)
                        }
                        None => Space::Gray,
                    };
                    // A color space starts out black, or with all its ink.
                    let initial = match space {
                        Space::Pattern(_) => None,
                        _ => Some(space.rgb(&space.initial())),
                    };
                    if operator == b"cs" {
                        (state.fill_space, state.fill, state.fill_pattern) =
                            (Rc::new(space), initial, None);
                    } else {
                        (state.stroke_space, state.stroke, state.stroke_pattern) =
                            (Rc::new(space), initial, None);
                    }
                }
                b"sc" | b"scn" | b"SC" | b"SCN" => {
                    let filling = operator[0] == b's';
                    let space = Rc::clone(if filling {
                        &state.fill_space
                    } else {
                        &state.stroke_space
                    });
                    // A pattern name as the last operand selects a pattern;
                    // the numbers before it color an uncolored one.
                    let (color, pattern) = match name {
                        Some(name) => {
                            let pattern = self.pattern(resources, name, &state, &space, &numbers);
                            (None, pattern.ok().flatten().map(Rc::new))
                        }
                        None => (Some(space.rgb(&numbers)), None),
                    };
                    if filling {
                        (state.fill, state.fill_pattern) = (color, pattern);
                    } else {
                        (state.stroke, state.stroke_pattern) = (color, pattern);
                    }
                }
                b"sh" => {
                    if let Some(name) = name {
                        // A shading that cannot be read is left out.
                        let _ = self.shade_operator(resources, name, &state);
                    }
                }

                b"Do" if depth < MAX_FORM_DEPTH => {
                    if let Some(name) = name {
                        // A form that cannot be read is left out.
                        let _ = self.form(resources, name, &state, depth);
                    }
                }
                // An inline image, drawn as an image XObject is; one that
                // cannot be read is left out.
                b"BI" => {
                    let fill = state.fill.unwrap_or([0.0; 3]);
                    let operands = &operation.operands;
                    if let Ok(Some(image)) =
                        image::load_inline(self.reader, operands, resources, fill)
                    {
                        self.image(&image, &state);
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Paint glyph outlines as the text rendering mode says: filled, or
    /// in the stroke color for the outline modes, or not at all.
    fn text(&mut self, glyphs: &[PlacedGlyph], state: &State, depth: usize) {
        let (color, pattern, alpha) = match state.text.mode {
            0 | 2 | 4 | 6 => (state.fill, &state.fill_pattern, state.fill_alpha),
            1 | 5 => (state.stroke, &state.stroke_pattern, state.stroke_alpha),
            _ => return,
        };
        if color.is_none() && pattern.is_none() {
            return;
        }
        // Text in a plain color is drawn glyph by glyph, each glyph turned
        // into coverage once for each size and quarter pixel it is drawn at.
        if pattern.is_none()
            && let Some(color) = color
        {
            for (outline, matrix) in glyphs {
                self.glyph(outline, matrix, color, alpha, state);
            }
            return;
        }
        let mut polygons: Vec<Vec<Point>> = Vec::new();
        for (outline, matrix) in glyphs {
            outline_polygons(outline, matrix, &mut polygons);
        }
        if polygons.is_empty() {
            return;
        }
        let mask = coverage(&self.window, &polygons, FillRule::NonZero);
        let pattern = pattern.clone();
        let shape = Shape {
            mask: &mask,
            polygons: &polygons,
        };
        self.paint(&shape, color, pattern.as_deref(), alpha, state, depth);
    }

    /// Paint one glyph in a plain color.
    fn glyph(
        &mut self,
        outline: &Arc<Vec<Segment>>,
        matrix: &Matrix,
        color: [f64; 3],
        alpha: f64,
        state: &State,
    ) {
        if outline.is_empty() || !matrix.iter().all(|value| value.is_finite()) {
            return;
        }
        if matrix[4].abs() > FAR_GLYPH || matrix[5].abs() > FAR_GLYPH {
            self.outline_glyph(outline, matrix, color, alpha, state);
            return;
        }
        // The origin's pixel, and where in it the origin falls.
        let place = |value: f64| {
            let steps = (value * GLYPH_STEPS).round();
            ((steps / GLYPH_STEPS).floor(), steps.rem_euclid(GLYPH_STEPS))
        };
        let (pixel_x, step_x) = place(matrix[4]);
        let (pixel_y, step_y) = place(matrix[5]);
        let key = GlyphKey {
            outline: Arc::as_ptr(outline) as usize,
            matrix: [matrix[0], matrix[1], matrix[2], matrix[3]].map(f64::to_bits),
            across: step_x as u8,
            down: step_y as u8,
        };
        let kept = match self.glyphs.entries.get(&key) {
            Some(kept) => kept.clone(),
            None => {
                let mut local = *matrix;
                local[4] = step_x / GLYPH_STEPS;
                local[5] = step_y / GLYPH_STEPS;
                let kept = glyph_coverage(outline, &local).map(Rc::new);
                // Each entry costs its coverage and a little more.
                let cost = kept.as_ref().map_or(0, |kept| kept.data.len()) + 64;
                if self.glyphs.bytes + cost > MAX_GLYPH_BYTES {
                    self.glyphs = GlyphCache::default();
                }
                self.glyphs.entries.insert(key, kept.clone());
                self.glyphs.bytes += cost;
                kept
            }
        };
        match kept {
            Some(kept) => {
                let placed = Placed {
                    coverage: &kept.data,
                    width: kept.width,
                    height: kept.height,
                    left: (pixel_x as i64).saturating_add(kept.left),
                    top: (pixel_y as i64).saturating_add(kept.top),
                    runs: None,
                };
                let clip = state.clip.as_deref();
                self.canvas
                    .paint_at(&placed, color, alpha, clip, state.blend);
            }
            // A large glyph is drawn where it is, from its outline.
            None => self.outline_glyph(outline, matrix, color, alpha, state),
        }
    }

    /// Paint one glyph in a plain color where it is, from its outline.
    fn outline_glyph(
        &mut self,
        outline: &[Segment],
        matrix: &Matrix,
        color: [f64; 3],
        alpha: f64,
        state: &State,
    ) {
        let mut polygons = Vec::new();
        outline_polygons(outline, matrix, &mut polygons);
        let mask = coverage(&self.window, &polygons, FillRule::NonZero);
        let clip = state.clip.as_deref();
        self.canvas.paint(&mask, color, alpha, clip, state.blend);
    }

    /// Paint a shape with a color or a pattern, through the clip.
    fn paint(
        &mut self,
        shape: &Shape<'_>,
        color: Option<[f64; 3]>,
        pattern: Option<&Paint>,
        alpha: f64,
        state: &State,
        depth: usize,
    ) {
        let clip = state.clip.as_deref();
        let mask = shape.mask;
        match (pattern, color) {
            (Some(Paint::Shading { shading, matrix }), _) => {
                self.shade(shading, matrix, Some(mask), alpha, state, true);
            }
            (Some(tiling @ Paint::Tiling { .. }), _) => {
                self.tile(shape, tiling, alpha, state, depth)
            }
            (None, Some(color)) => self.canvas.paint(mask, color, alpha, clip, state.blend),
            (None, None) => {}
        }
    }

    fn fill(&mut self, path: &[Subpath], rule: FillRule, state: &State, depth: usize) {
        if state.fill.is_none() && state.fill_pattern.is_none() {
            return;
        }
        let polygons: Vec<Vec<Point>> = path.iter().map(|(points, _)| points.clone()).collect();
        let mask = coverage(&self.window, &polygons, rule);
        let pattern = state.fill_pattern.clone();
        let shape = Shape {
            mask: &mask,
            polygons: &polygons,
        };
        self.paint(
            &shape,
            state.fill,
            pattern.as_deref(),
            state.fill_alpha,
            state,
            depth,
        );
    }

    fn stroke(&mut self, path: &[Subpath], state: &State, depth: usize) {
        if state.stroke.is_none() && state.stroke_pattern.is_none() {
            return;
        }
        // The pen is scaled by the matrix; an uneven scale is averaged.
        let matrix = &state.ctm;
        let scale = (matrix[0] * matrix[3] - matrix[1] * matrix[2]).abs().sqrt();
        let width = (state.line_width * scale).max(MIN_LINE_WIDTH);
        let pattern: Vec<f64> = state.dash.iter().map(|length| length * scale).collect();
        let mut lines: Vec<Subpath> = Vec::new();
        for (points, closed) in path {
            if pattern.iter().any(|&length| length > 0.0) {
                let mut points = points.clone();
                if *closed && let Some(&first) = points.first() {
                    points.push(first);
                }
                let dashes = dashes(&points, &pattern, state.dash_phase * scale);
                lines.extend(dashes.into_iter().map(|dash| (dash, false)));
            } else {
                lines.push((points.clone(), *closed));
            }
        }
        let outline = stroke_outline(&lines, width, state.line_cap);
        let mask = coverage(&self.window, &outline, FillRule::NonZero);
        let pattern = state.stroke_pattern.clone();
        let alpha = state.stroke_alpha;
        let shape = Shape {
            mask: &mask,
            polygons: &outline,
        };
        self.paint(
            &shape,
            state.stroke,
            pattern.as_deref(),
            alpha,
            state,
            depth,
        );
    }

    /// Paint a shading, whose space `matrix` maps to device space, over a
    /// mask, or over everything the clip lets through when there is none.
    /// A pattern's shading fills the rest of the mask with its background.
    fn shade(
        &mut self,
        shading: &Shading,
        matrix: &Matrix,
        mask: Option<&Mask>,
        alpha: f64,
        state: &State,
        with_background: bool,
    ) {
        let (clip, blend) = (state.clip.as_deref(), state.blend);
        let Some(inverse) = invert(matrix) else {
            return;
        };
        let window = self.window;
        let (left, top, columns, rows) = match mask {
            Some(mask) => (mask.left, mask.top, mask.width, mask.height),
            None => (window.left, window.top, window.width(), window.height()),
        };
        let background = shading.background.filter(|_| with_background);
        for row in 0..rows {
            let y = top + row;
            for column in 0..columns {
                let x = left + column;
                let covered = mask.map_or(255, |mask| mask.data[row * mask.width + column]);
                let mut amount = f64::from(covered) / 255.0 * alpha.clamp(0.0, 1.0);
                if let Some(clip) = clip {
                    amount *= f64::from(clip.at(x, y)) / 255.0;
                }
                if amount <= 0.0 {
                    continue;
                }
                let (u, v) = transform(&inverse, x as f64 + 0.5, y as f64 + 0.5);
                if let Some(color) = shading.color_at(u, v).or(background) {
                    self.canvas
                        .blend(x, y, color.map(|value| value * 255.0), amount, blend);
                }
            }
        }
    }

    /// Paint the shading a resource name selects over the clip, as `sh`
    /// does.
    fn shade_operator(
        &mut self,
        resources: &Dictionary,
        name: &[u8],
        state: &State,
    ) -> Result<(), DocumentError> {
        let PdfObject::Dictionary(shadings) = self.reader.resolve_entry(resources, "Shading")?
        else {
            return Ok(());
        };
        let Some(object) = shadings.get(name).cloned() else {
            return Ok(());
        };
        if let Some(shading) = Shading::load(self.reader, &object, resources)? {
            self.shade(&shading, &state.ctm, None, state.fill_alpha, state, false);
        }
        Ok(())
    }

    /// Read the pattern a resource name selects, placed by the matrix the
    /// content stream started with. `space` and `components` give an
    /// uncolored pattern's color.
    fn pattern(
        &mut self,
        resources: &Dictionary,
        name: &[u8],
        state: &State,
        space: &Space,
        components: &[f64],
    ) -> Result<Option<Paint>, DocumentError> {
        let PdfObject::Dictionary(patterns) = self.reader.resolve_entry(resources, "Pattern")?
        else {
            return Ok(None);
        };
        let Some(object) = patterns.get(name).cloned() else {
            return Ok(None);
        };
        let PdfObject::Dictionary(pattern) = self.reader.resolve(&object)? else {
            return Ok(None);
        };
        let mut numbers = |key: &str| -> Vec<f64> {
            match self.reader.resolve_entry(&pattern, key) {
                Ok(PdfObject::Array(values)) => {
                    values.iter().filter_map(PdfObject::as_number).collect()
                }
                _ => Vec::new(),
            }
        };
        let own = <Matrix>::try_from(numbers("Matrix").as_slice()).unwrap_or(IDENTITY);
        let matrix = multiply(&own, &state.base);
        match entry(&pattern, "PatternType").and_then(PdfObject::as_integer) {
            Some(2) => {
                let Some(shading) = entry(&pattern, "Shading").cloned() else {
                    return Ok(None);
                };
                let shading = Shading::load(self.reader, &shading, resources)?;
                Ok(shading.map(|shading| Paint::Shading {
                    shading: Box::new(shading),
                    matrix,
                }))
            }
            Some(1) => {
                let bounds = match numbers("BBox")[..] {
                    [a, b, c, d] => [a.min(c), b.min(d), a.max(c), b.max(d)],
                    _ => return Ok(None),
                };
                let mut step =
                    |key: &str, fallback: f64| match self.reader.resolve_entry(&pattern, key) {
                        Ok(value) => value
                            .as_number()
                            .filter(|step| step.abs() > 1e-9)
                            .unwrap_or(fallback),
                        Err(_) => fallback,
                    };
                let step = (
                    step("XStep", bounds[2] - bounds[0]).abs(),
                    step("YStep", bounds[3] - bounds[1]).abs(),
                );
                if step.0 <= 1e-9 || step.1 <= 1e-9 {
                    return Ok(None);
                }
                let PdfObject::Reference(reference) = object else {
                    return Ok(None);
                };
                let Some(content) = self.reader.stream(reference.number)? else {
                    return Ok(None);
                };
                let own_resources = match self.reader.resolve_entry(&pattern, "Resources")? {
                    PdfObject::Dictionary(own) => own,
                    _ => resources.clone(),
                };
                // An uncolored pattern is drawn in the color given with it.
                let uncolored =
                    entry(&pattern, "PaintType").and_then(PdfObject::as_integer) == Some(2);
                let color = uncolored.then(|| space.rgb(components));
                Ok(Some(Paint::Tiling {
                    content,
                    resources: own_resources,
                    bounds,
                    step,
                    matrix,
                    color,
                }))
            }
            _ => Ok(None),
        }
    }

    /// Paint a shape with a tiling pattern: the pattern's cell is drawn at
    /// each step that reaches the shape, through its mask and the clip.
    /// Cells are not clipped to their own bounding box.
    fn tile(&mut self, shape: &Shape<'_>, tiling: &Paint, alpha: f64, state: &State, depth: usize) {
        let mask = shape.mask;
        let Paint::Tiling {
            content,
            resources,
            bounds,
            step,
            matrix,
            color,
        } = tiling
        else {
            return;
        };
        // A shape that covers nothing in the window paints nothing there.
        if depth >= MAX_FORM_DEPTH || mask.width == 0 || mask.height == 0 {
            return;
        }
        let Some(inverse) = invert(matrix) else {
            return;
        };
        // The steps are counted over the shape's reach on the whole page,
        // so that a pattern with too many tiles for the page is left out of
        // every window alike.
        let Some(area) = reach(&self.page, shape.polygons) else {
            return;
        };
        // What the cells may paint: the mask within the clip.
        let mut through = mask.clone();
        if let Some(clip) = &state.clip {
            narrow(&mut through, clip);
        }
        let through = Rc::new(through);
        // The corners of that reach in pattern space give the steps to draw.
        let corners = [
            (area.left, area.top),
            (area.right, area.top),
            (area.left, area.bottom),
            (area.right, area.bottom),
        ]
        .map(|(x, y)| transform(&inverse, x as f64, y as f64));
        let low = |values: [f64; 4]| values.iter().copied().fold(f64::INFINITY, f64::min);
        let high = |values: [f64; 4]| values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let (across, down) = (
            corners.map(|corner| corner.0),
            corners.map(|corner| corner.1),
        );
        let first_x = ((low(across) - bounds[2]) / step.0).floor();
        let last_x = ((high(across) - bounds[0]) / step.0).ceil();
        let first_y = ((low(down) - bounds[3]) / step.1).floor();
        let last_y = ((high(down) - bounds[1]) / step.1).ceil();
        let count = (last_x - first_x + 1.0) * (last_y - first_y + 1.0);
        if !count.is_finite() || count > MAX_TILES as f64 || count < 1.0 {
            return;
        }
        for row in first_y as i64..=last_y as i64 {
            for column in first_x as i64..=last_x as i64 {
                let offset = [
                    1.0,
                    0.0,
                    0.0,
                    1.0,
                    column as f64 * step.0,
                    row as f64 * step.1,
                ];
                let mut cell = State::new(multiply(&offset, matrix), Some(Rc::clone(&through)));
                cell.fill_alpha = alpha;
                cell.stroke_alpha = alpha;
                if let Some(color) = color {
                    (cell.fill, cell.stroke) = (Some(*color), Some(*color));
                    cell.color_locked = true;
                }
                // A cell that cannot be drawn is left out.
                let _ = self.run(resources, content, cell, depth + 1);
            }
        }
    }

    /// Narrow the clip to the inside of a path.
    fn clip(&mut self, path: &[Subpath], rule: FillRule, state: &mut State) {
        let polygons: Vec<Vec<Point>> = path.iter().map(|(points, _)| points.clone()).collect();
        // Without a clip before, the path's coverage is the clip; nothing
        // shows outside the mask either way.
        let mut clip = coverage(&self.window, &polygons, rule);
        if let Some(before) = &state.clip {
            narrow(&mut clip, before);
        }
        state.clip = Some(Rc::new(clip));
    }

    /// Apply the entries of a named graphics state dictionary.
    fn graphics_state(
        &mut self,
        resources: &Dictionary,
        name: &[u8],
        state: &mut State,
    ) -> Result<(), DocumentError> {
        let PdfObject::Dictionary(states) = self.reader.resolve_entry(resources, "ExtGState")?
        else {
            return Ok(());
        };
        let Some(PdfObject::Dictionary(entries)) = states
            .get(name)
            .map(|entries| self.reader.resolve(entries))
            .transpose()?
        else {
            return Ok(());
        };
        let mut number = |key| match self.reader.resolve_entry(&entries, key) {
            Ok(value) => value.as_number(),
            Err(_) => None,
        };
        if let Some(alpha) = number("ca") {
            state.fill_alpha = alpha;
        }
        if let Some(alpha) = number("CA") {
            state.stroke_alpha = alpha;
        }
        if let Some(width) = number("LW") {
            state.line_width = width;
        }
        // A list of blend modes means the first one this engine knows.
        match self.reader.resolve_entry(&entries, "BM")? {
            PdfObject::Name(name) => state.blend = BlendMode::named(&name),
            PdfObject::Array(names) => {
                state.blend = names
                    .iter()
                    .filter_map(PdfObject::as_name)
                    .map(BlendMode::named)
                    .find(|&mode| mode != BlendMode::Normal)
                    .unwrap_or(BlendMode::Normal);
            }
            _ => {}
        }
        Ok(())
    }

    /// Draw an image over the unit square of user space, which the matrix
    /// stretches to the image's place on the page.
    fn image(&mut self, image: &Image, state: &State) {
        let matrix = &state.ctm;
        let determinant = matrix[0] * matrix[3] - matrix[1] * matrix[2];
        if !determinant.is_finite() || determinant.abs() < 1e-12 {
            return;
        }
        // An image much larger than its place is averaged down first, so
        // that every part of it counts.
        let across = matrix[0].hypot(matrix[1]).max(1e-6);
        let down = matrix[2].hypot(matrix[3]).max(1e-6);
        let surplus = |pixels: usize, length: f64| ((pixels as f64 / length) as usize).clamp(1, 64);
        let fewer_across = surplus(image.width, across);
        let fewer_down = surplus(image.height, down);
        let shrunk;
        let image = if fewer_across > 1 || fewer_down > 1 {
            shrunk = image::shrink(image, fewer_across, fewer_down);
            &shrunk
        } else {
            image
        };

        let corners =
            [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)].map(|(x, y)| transform(matrix, x, y));
        // The pixels the image may reach, within the window.
        let bound = |values: [f64; 4], from: usize, to: usize| {
            let low = values.iter().copied().fold(f64::INFINITY, f64::min);
            let high = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            let clamp = |value: f64| value.clamp(from as f64, to as f64) as usize;
            (clamp(low.floor()), clamp(high.ceil()))
        };
        let window = self.window;
        let (left, right) = bound(corners.map(|corner| corner.0), window.left, window.right);
        let (top, bottom) = bound(corners.map(|corner| corner.1), window.top, window.bottom);
        // An upright image: a point's column of the image depends only on
        // its x, and its row only on its y, so they are found once for each
        // column and row of the page, by the same sums as below.
        if matrix[1] == 0.0 && matrix[2] == 0.0 {
            let offsets = [0.25, 0.75];
            let columns: Vec<[Option<usize>; 2]> = (left..right)
                .map(|x| {
                    offsets.map(|offset_x| {
                        let from_x = x as f64 + offset_x - matrix[4];
                        let u = (matrix[3] * from_x - matrix[2] * 0.0) / determinant;
                        (0.0..1.0)
                            .contains(&u)
                            .then(|| ((u * image.width as f64) as usize).min(image.width - 1))
                    })
                })
                .collect();
            let rows: Vec<[Option<usize>; 2]> = (top..bottom)
                .map(|y| {
                    offsets.map(|offset_y| {
                        let from_y = y as f64 + offset_y - matrix[5];
                        let v = (matrix[0] * from_y - matrix[1] * 0.0) / determinant;
                        (0.0..1.0).contains(&v).then(|| {
                            (((1.0 - v) * image.height as f64) as usize).min(image.height - 1)
                        })
                    })
                })
                .collect();
            // A pixel that all four points fall on, with every image pixel
            // they take opaque, the clip wholly open there or no clip, and
            // nothing to mix, gets the
            // plain average of the four colors. The table gives that average
            // for each sum of four bytes, by the very sums below, so the
            // result is the same as theirs to the bit.
            let plain = state.fill_alpha == 1.0 && state.blend == BlendMode::Normal;
            let average: [u8; 4 * 255 + 1] = std::array::from_fn(|sum| {
                let color = sum as f64 * 255.0 / (4.0 * 255.0);
                to_byte_f64((color / 255.0) * 255.0)
            });
            let pixels = image.rgba.as_chunks::<4>().0;
            let image_row = |row: usize| &pixels[row * image.width..(row + 1) * image.width];
            for (y, row_pair) in (top..bottom).zip(&rows) {
                if row_pair.iter().all(Option::is_none) {
                    continue;
                }
                let both_rows = match *row_pair {
                    [Some(first), Some(second)] if plain => {
                        Some((image_row(first), image_row(second)))
                    }
                    _ => None,
                };
                let clip_row = state.clip.as_deref().map(|clip| clip.row(y));
                // How much of a pixel of this row the clip lets through.
                let through = |x: usize| {
                    clip_row.map(|(left, row)| match x.checked_sub(left) {
                        Some(column) => row.get(column).copied().unwrap_or(0),
                        None => 0,
                    })
                };
                for (x, column_pair) in (left..right).zip(&columns) {
                    // Outside the clip, mixing by nothing leaves the page
                    // as it is.
                    if through(x) == Some(0) && state.fill_alpha.is_finite() {
                        continue;
                    }
                    if let (Some((upper, lower)), [Some(first), Some(second)]) =
                        (both_rows, *column_pair)
                        && through(x).is_none_or(|through| through == 255)
                    {
                        let corners = [upper[first], upper[second], lower[first], lower[second]];
                        if corners.iter().all(|pixel| pixel[3] == 255) {
                            let color = [0, 1, 2].map(|channel| {
                                let sum: usize = corners
                                    .iter()
                                    .map(|pixel| usize::from(pixel[channel]))
                                    .sum();
                                average[sum]
                            });
                            self.canvas.set_rgb(x, y, color);
                            continue;
                        }
                    }
                    let mut sums = [0.0_f64; 4];
                    for row in row_pair.iter().flatten() {
                        for column in column_pair.iter().flatten() {
                            let pixel = &image.rgba[(row * image.width + column) * 4..][..4];
                            let alpha = f64::from(pixel[3]);
                            for (sum, &value) in sums.iter_mut().zip(&pixel[..3]) {
                                *sum += f64::from(value) * alpha;
                            }
                            sums[3] += alpha;
                        }
                    }
                    if sums[3] <= 0.0 {
                        continue;
                    }
                    let color = [sums[0], sums[1], sums[2]].map(|sum| sum / sums[3]);
                    let mut amount = sums[3] / (4.0 * 255.0) * state.fill_alpha;
                    if let Some(clip) = &state.clip {
                        amount *= f64::from(clip.at(x, y)) / 255.0;
                    }
                    self.canvas.blend(x, y, color, amount, state.blend);
                }
            }
            return;
        }
        for y in top..bottom {
            for x in left..right {
                // Four points inside the pixel, each taking the image pixel
                // it falls on.
                let mut sums = [0.0_f64; 4];
                for (offset_x, offset_y) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)]
                {
                    let (from_x, from_y) = (
                        x as f64 + offset_x - matrix[4],
                        y as f64 + offset_y - matrix[5],
                    );
                    let u = (matrix[3] * from_x - matrix[2] * from_y) / determinant;
                    let v = (matrix[0] * from_y - matrix[1] * from_x) / determinant;
                    if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
                        continue;
                    }
                    // The image's first row is at the top of the square.
                    let column = ((u * image.width as f64) as usize).min(image.width - 1);
                    let row = (((1.0 - v) * image.height as f64) as usize).min(image.height - 1);
                    let pixel = &image.rgba[(row * image.width + column) * 4..][..4];
                    let alpha = f64::from(pixel[3]);
                    for (sum, &value) in sums.iter_mut().zip(&pixel[..3]) {
                        *sum += f64::from(value) * alpha;
                    }
                    sums[3] += alpha;
                }
                if sums[3] <= 0.0 {
                    continue;
                }
                let color = [sums[0], sums[1], sums[2]].map(|sum| sum / sums[3]);
                let mut amount = sums[3] / (4.0 * 255.0) * state.fill_alpha;
                if let Some(clip) = &state.clip {
                    amount *= f64::from(clip.at(x, y)) / 255.0;
                }
                self.canvas.blend(x, y, color, amount, state.blend);
            }
        }
    }

    /// Draw the form XObject a resource name selects, if it is one.
    /// Draw one Type 3 glyph: its stream, with `matrix` taking glyph space
    /// to device space. A glyph that starts with `d1` is a shape painted in
    /// the text's color, so it cannot change colors.
    fn type3_glyph(
        &mut self,
        reference: ObjectReference,
        matrix: Matrix,
        resources: &Dictionary,
        state: &State,
        depth: usize,
    ) -> Result<(), DocumentError> {
        if depth >= MAX_FORM_DEPTH || matches!(state.text.mode, 3 | 7) {
            return Ok(());
        }
        let Some(content) = self.reader.stream(reference.number)? else {
            return Ok(());
        };
        let mut glyph = state.clone();
        glyph.ctm = matrix;
        glyph.base = matrix;
        glyph.text = TextState::new();
        let operations = parse_content(&content).map_err(DocumentError::Syntax)?;
        if operations
            .iter()
            .any(|operation| operation.operator == b"d1")
        {
            glyph.color_locked = true;
        }
        self.run(resources, &content, glyph, depth + 1)
    }

    /// Draw the normal appearance of each annotation of a page that is not
    /// hidden, mapped onto the annotation's rectangle (ISO 32000-2 section
    /// 12.5.5).
    fn annotations(
        &mut self,
        page: ObjectReference,
        resources: &Dictionary,
        state: &State,
    ) -> Result<(), DocumentError> {
        let PdfObject::Dictionary(page) = self.reader.resolve(&PdfObject::Reference(page))? else {
            return Ok(());
        };
        let PdfObject::Array(annotations) = self.reader.resolve_entry(&page, "Annots")? else {
            return Ok(());
        };
        for annotation in annotations {
            let Ok(PdfObject::Dictionary(annotation)) = self.reader.resolve(&annotation) else {
                continue;
            };
            // Hidden (bit 2) and NoView (bit 6) annotations are not shown,
            // and a popup shows its parent's text, not a drawing.
            let flags = entry(&annotation, "F")
                .and_then(PdfObject::as_integer)
                .unwrap_or(0);
            let subtype = entry(&annotation, "Subtype").and_then(PdfObject::as_name);
            if flags & (2 | 32) != 0 || subtype == Some(b"Popup") {
                continue;
            }
            let Some(appearance) = self.appearance(&annotation)? else {
                continue;
            };
            let rectangle: Vec<f64> = match self.reader.resolve_entry(&annotation, "Rect")? {
                PdfObject::Array(values) => {
                    values.iter().filter_map(PdfObject::as_number).collect()
                }
                _ => continue,
            };
            let [a, b, c, d] = rectangle[..] else {
                continue;
            };
            let (left, bottom, right, top) = (a.min(c), b.min(d), a.max(c), b.max(d));
            let PdfObject::Dictionary(form) =
                self.reader.resolve(&PdfObject::Reference(appearance))?
            else {
                continue;
            };
            let numbers = |reader: &mut Reader, key: &str| -> Vec<f64> {
                match reader.resolve_entry(&form, key) {
                    Ok(PdfObject::Array(values)) => {
                        values.iter().filter_map(PdfObject::as_number).collect()
                    }
                    _ => Vec::new(),
                }
            };
            let bounds = numbers(self.reader, "BBox");
            let matrix =
                <Matrix>::try_from(numbers(self.reader, "Matrix").as_slice()).unwrap_or(IDENTITY);
            let [x0, y0, x1, y1] = bounds[..] else {
                continue;
            };
            // The box the appearance's matrix makes of its bounding box is
            // stretched onto the rectangle.
            let corners =
                [(x0, y0), (x1, y0), (x0, y1), (x1, y1)].map(|(x, y)| transform(&matrix, x, y));
            let low = |values: [f64; 4]| values.iter().copied().fold(f64::INFINITY, f64::min);
            let high = |values: [f64; 4]| values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            let (across, down) = (
                corners.map(|corner| corner.0),
                corners.map(|corner| corner.1),
            );
            let (box_left, box_bottom) = (low(across), low(down));
            let (box_width, box_height) = (high(across) - box_left, high(down) - box_bottom);
            if box_width.abs() < 1e-9 || box_height.abs() < 1e-9 {
                continue;
            }
            let (scale_x, scale_y) = ((right - left) / box_width, (top - bottom) / box_height);
            let fit = [
                scale_x,
                0.0,
                0.0,
                scale_y,
                left - box_left * scale_x,
                bottom - box_bottom * scale_y,
            ];
            let placed = State::new(multiply(&fit, &state.ctm), None);
            let _ = self.draw_form(appearance, resources, &placed, 0);
        }
        Ok(())
    }

    /// The stream of an annotation's normal appearance: `/AP /N` itself, or
    /// the state `/AS` picks from it.
    fn appearance(
        &mut self,
        annotation: &Dictionary,
    ) -> Result<Option<ObjectReference>, DocumentError> {
        let PdfObject::Dictionary(appearances) = self.reader.resolve_entry(annotation, "AP")?
        else {
            return Ok(None);
        };
        let Some(normal) = entry(&appearances, "N").cloned() else {
            return Ok(None);
        };
        let resolved = self.reader.resolve(&normal)?;
        let Some(dictionary) = resolved.as_dictionary() else {
            return Ok(None);
        };
        // A stream has a bounding box; a dictionary of states does not.
        if entry(dictionary, "BBox").is_some() {
            return Ok(match normal {
                PdfObject::Reference(reference) => Some(reference),
                _ => None,
            });
        }
        let Some(state) = entry(annotation, "AS").and_then(PdfObject::as_name) else {
            return Ok(None);
        };
        Ok(match dictionary.get(state) {
            Some(PdfObject::Reference(reference)) => Some(*reference),
            _ => None,
        })
    }

    fn form(
        &mut self,
        resources: &Dictionary,
        name: &[u8],
        state: &State,
        depth: usize,
    ) -> Result<(), DocumentError> {
        let PdfObject::Dictionary(objects) = self.reader.resolve_entry(resources, "XObject")?
        else {
            return Ok(());
        };
        let Some(PdfObject::Reference(reference)) = objects.get(name) else {
            return Ok(());
        };
        let PdfObject::Dictionary(form) = self.reader.resolve(&PdfObject::Reference(*reference))?
        else {
            return Ok(());
        };
        let subtype = entry(&form, "Subtype").and_then(PdfObject::as_name);
        if subtype == Some(b"Image") {
            let fill = state.fill.unwrap_or([0.0; 3]);
            // The unit square the image fills, in device pixels.
            let matrix = &state.ctm;
            let shown = (matrix[0].hypot(matrix[1]), matrix[2].hypot(matrix[3]));
            let loaded = image::load_for_drawing(
                self.reader,
                reference.number,
                resources,
                fill,
                Some(shown),
            )?;
            if let Some(image) = loaded {
                self.image(&image, state);
            }
            return Ok(());
        }
        if subtype != Some(b"Form") {
            return Ok(());
        }
        self.draw_form(*reference, resources, state, depth)
    }

    /// Draw a form XObject: its content, after its matrix, inside its
    /// bounding box.
    fn draw_form(
        &mut self,
        reference: ObjectReference,
        resources: &Dictionary,
        state: &State,
        depth: usize,
    ) -> Result<(), DocumentError> {
        let PdfObject::Dictionary(form) = self.reader.resolve(&PdfObject::Reference(reference))?
        else {
            return Ok(());
        };
        let Some(content) = self.reader.stream(reference.number)? else {
            return Ok(());
        };
        let mut numbers = |key, count| -> Result<Option<Vec<f64>>, DocumentError> {
            let PdfObject::Array(values) = self.reader.resolve_entry(&form, key)? else {
                return Ok(None);
            };
            let values: Vec<f64> = values.iter().filter_map(PdfObject::as_number).collect();
            Ok((values.len() == count).then_some(values))
        };
        let matrix = numbers("Matrix", 6)?;
        let bounds = numbers("BBox", 4)?;
        let mut inner = state.clone();
        if let Some(matrix) = matrix {
            let matrix = [0, 1, 2, 3, 4, 5].map(|index| matrix[index]);
            inner.ctm = multiply(&matrix, &state.ctm);
        }
        inner.base = inner.ctm;
        // The form draws only inside its bounding box.
        if let Some(bounds) = bounds {
            let corner = |x, y| transform(&inner.ctm, x, y);
            let corners = vec![
                corner(bounds[0], bounds[1]),
                corner(bounds[2], bounds[1]),
                corner(bounds[2], bounds[3]),
                corner(bounds[0], bounds[3]),
            ];
            self.clip(&[(corners, true)], FillRule::NonZero, &mut inner);
        }
        match self.reader.resolve_entry(&form, "Resources")? {
            PdfObject::Dictionary(own) => self.run(&own, &content, inner, depth + 1),
            _ => self.run(resources, &content, inner, depth + 1),
        }
    }
}
