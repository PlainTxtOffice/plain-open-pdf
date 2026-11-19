//! Rasterization: turning polygons into anti-aliased pixels.

/// A point in device space: pixels, with the origin at the top left.
pub(crate) type Point = (f64, f64);

/// How a path's inside is told from its outside (ISO 32000-2 section
/// 8.5.3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FillRule {
    NonZero,
    EvenOdd,
}

/// How a painted color combines with the color beneath it (ISO 32000-2
/// section 11.3.5). Modes this engine does not know paint as `Normal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum BlendMode {
    #[default]
    Normal,
    Multiply,
    Screen,
    Darken,
    Lighten,
}

impl BlendMode {
    /// The mode a `/BM` name gives.
    pub(crate) fn named(name: &[u8]) -> Self {
        match name {
            b"Multiply" => Self::Multiply,
            b"Screen" => Self::Screen,
            b"Darken" => Self::Darken,
            b"Lighten" => Self::Lighten,
            _ => Self::Normal,
        }
    }

    /// One component, from 0 to 255, after painting a source over a
    /// backdrop by `amount`, both given as fractions from 0 to 1: the
    /// backdrop's byte and the source's component each divided by 255.
    #[inline]
    fn mix(self, below: f64, above: f64, amount: f64) -> f64 {
        let blended = match self {
            Self::Normal => above,
            Self::Multiply => below * above,
            Self::Screen => below + above - below * above,
            Self::Darken => below.min(above),
            Self::Lighten => below.max(above),
        };
        (below * (1.0 - amount) + blended * amount) * 255.0
    }
}

/// Every byte divided by 255, the same correctly rounded division as at
/// run time.
const fn fractions() -> [f64; 256] {
    let mut table = [0.0; 256];
    let mut index = 0;
    while index < 256 {
        table[index] = index as f64 / 255.0;
        index += 1;
    }
    table
}

/// Each byte's fraction of 255.
static FRACTIONS: [f64; 256] = fractions();

/// Sample rows taken through each pixel row; coverage along a row is exact.
const SAMPLES: usize = 5;

/// Crossings up to this many are ordered by insertion; more by the library.
const FEW_CROSSINGS: usize = 32;

/// Room set aside for edges before any is found. Points that give no edge
/// must not set aside more: a path may hold very many of them.
const EDGES_AT_FIRST: usize = 64;

/// `value.round().clamp(0.0, 255.0) as u8`, without the library call that
/// rounding compiles to on processors without SSE4.1. The result is the
/// same for every value, infinities and NaN included.
#[inline]
pub(crate) fn to_byte(value: f32) -> u8 {
    if value.is_nan() || value <= -0.5 {
        return 0;
    }
    if value >= 254.5 {
        return 255;
    }
    // Truncation toward zero, then the part it dropped, which is exact.
    let whole = value as i32;
    let rest = value - whole as f32;
    (whole + i32::from(rest >= 0.5)) as u8
}

/// `value.round().clamp(0.0, 255.0) as u8` for a double, as `to_byte`.
#[inline]
pub(crate) fn to_byte_f64(value: f64) -> u8 {
    if value.is_nan() || value <= -0.5 {
        return 0;
    }
    if value >= 254.5 {
        return 255;
    }
    let whole = value as i32;
    let rest = value - f64::from(whole);
    (whole + i32::from(rest >= 0.5)) as u8
}

/// An image of 8-bit RGBA pixels, rows from the top, without padding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pixmap {
    width: usize,
    height: usize,
    data: Vec<u8>,
}

impl Pixmap {
    /// An opaque white image.
    pub(crate) fn white(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            data: vec![255; width * height * 4],
        }
    }

    /// The width in pixels.
    pub fn width(&self) -> usize {
        self.width
    }

    /// The height in pixels.
    pub fn height(&self) -> usize {
        self.height
    }

    /// The pixels: four bytes each, red, green, blue, and alpha, row by row
    /// from the top left.
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// The pixels without alpha: three bytes each, red, green, and blue,
    /// row by row from the top left. Rendered pages are opaque.
    pub fn rgb(&self) -> Vec<u8> {
        let pixels = self.data.as_chunks::<4>().0;
        pixels
            .iter()
            .flat_map(|pixel| [pixel[0], pixel[1], pixel[2]])
            .collect()
    }

    /// The pixels as gray levels, one byte each, row by row from the top
    /// left, weighted as ITU-R BT.601 weights red, green, and blue.
    pub fn gray(&self) -> Vec<u8> {
        let pixels = self.data.as_chunks::<4>().0;
        pixels
            .iter()
            .map(|pixel| {
                let [red, green, blue] = [pixel[0], pixel[1], pixel[2]].map(u32::from);
                ((red * 299 + green * 587 + blue * 114 + 500) / 1000) as u8
            })
            .collect()
    }

    /// The red, green, blue, and alpha of one pixel, if it is in the image.
    pub fn pixel(&self, x: usize, y: usize) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let start = (y * self.width + x) * 4;
        self.data.get(start..start + 4)?.try_into().ok()
    }

    /// Encode the image as a PNG file.
    pub fn to_png(&self) -> Vec<u8> {
        let mut raw = Vec::with_capacity(self.data.len() + self.height);
        for row in self.data.chunks(self.width.max(1) * 4) {
            raw.push(0);
            raw.extend(row);
        }
        let stream = crate::filter::deflate_zlib(&raw);

        let mut header = Vec::new();
        header.extend((self.width as u32).to_be_bytes());
        header.extend((self.height as u32).to_be_bytes());
        // Eight bits per sample, RGBA, no interlacing.
        header.extend([8, 6, 0, 0, 0]);
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        for (kind, data) in [
            (b"IHDR", &header),
            (b"IDAT", &stream),
            (b"IEND", &Vec::new()),
        ] {
            png.extend((data.len() as u32).to_be_bytes());
            let start = png.len();
            png.extend(kind);
            png.extend(data);
            let checksum = crc32(&png[start..]);
            png.extend(checksum.to_be_bytes());
        }
        png
    }
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                crc >> 1 ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// Coverage, from 0 to 255, of a rectangle whose top left is at `left` and
/// `top`, which may be outside the image.
#[derive(Clone, Copy)]
pub(crate) struct Placed<'a> {
    pub coverage: &'a [u8],
    pub width: usize,
    pub height: usize,
    pub left: i64,
    pub top: i64,
    /// The only cells that may hold coverage, when they are known: each a
    /// row and the columns from one up to another, no cell twice.
    pub runs: Option<&'a [Run]>,
}

/// A row of coverage and the columns from one up to another.
pub(crate) type Run = (usize, usize, usize);

/// How much of each pixel of a rectangle of the image a shape covers,
/// from 0 to 255.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Mask {
    pub left: usize,
    pub top: usize,
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
    /// The only cells that may hold coverage, when the mask was made by
    /// scanning: every cell outside these runs is empty.
    pub runs: Option<Vec<Run>>,
}

impl Mask {
    /// The coverage at a pixel of the image: none outside the mask.
    #[inline]
    pub(crate) fn at(&self, x: usize, y: usize) -> u8 {
        let (left, row) = self.row(y);
        match x.checked_sub(left) {
            Some(column) => row.get(column).copied().unwrap_or(0),
            None => 0,
        }
    }

    /// The mask's part of a row of the image: the column it starts at and
    /// its coverage from there, empty for a row the mask does not reach.
    #[inline]
    pub(crate) fn row(&self, y: usize) -> (usize, &[u8]) {
        match y.checked_sub(self.top) {
            Some(row) if row < self.height => (
                self.left,
                &self.data[row * self.width..(row + 1) * self.width],
            ),
            _ => (self.left, &[]),
        }
    }
}

/// A rectangle of the page image: the pixels from `left` up to `right` and
/// from `top` up to `bottom`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Window {
    pub left: usize,
    pub top: usize,
    pub right: usize,
    pub bottom: usize,
}

impl Window {
    /// The window that holds a whole image of the given size.
    pub(crate) fn whole(width: usize, height: usize) -> Self {
        Self {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        }
    }

    /// The number of pixels across.
    pub(crate) fn width(&self) -> usize {
        self.right.saturating_sub(self.left)
    }

    /// The number of pixels down.
    pub(crate) fn height(&self) -> usize {
        self.bottom.saturating_sub(self.top)
    }
}

/// The pixels of one window of the page image. Everything painted is placed
/// in page coordinates, and what falls outside the window is left out, so
/// the pixels kept are those a canvas of the whole page would hold there.
pub(crate) struct Canvas {
    image: Pixmap,
    left: usize,
    top: usize,
}

impl Canvas {
    /// A white canvas for a window of the page.
    pub(crate) fn new(window: &Window) -> Self {
        Self {
            image: Pixmap::white(window.width(), window.height()),
            left: window.left,
            top: window.top,
        }
    }

    /// The window's pixels, as an image of their own.
    pub(crate) fn into_image(self) -> Pixmap {
        self.image
    }

    /// Paint a color through a coverage mask, an opacity, and a clip.
    pub(crate) fn paint(
        &mut self,
        mask: &Mask,
        color: [f64; 3],
        alpha: f64,
        clip: Option<&Mask>,
        mode: BlendMode,
    ) {
        let placed = Placed {
            coverage: &mask.data,
            width: mask.width,
            height: mask.height,
            left: mask.left as i64,
            top: mask.top as i64,
            runs: mask.runs.as_deref(),
        };
        self.paint_at(&placed, color, alpha, clip, mode);
    }

    /// Paint a color through coverage that may lie partly or wholly
    /// outside the canvas, through an opacity and a clip.
    pub(crate) fn paint_at(
        &mut self,
        placed: &Placed<'_>,
        color: [f64; 3],
        alpha: f64,
        clip: Option<&Mask>,
        mode: BlendMode,
    ) {
        let Placed {
            coverage,
            width,
            height,
            left,
            top,
            runs,
        } = *placed;
        let color = color.map(|component| component.clamp(0.0, 1.0) * 255.0);
        let alpha = alpha.clamp(0.0, 1.0) / 255.0;
        // Each component as a fraction, as mixing takes it.
        let above = color.map(|component| component / 255.0);
        // What mixing gives for an amount of 1 in the normal mode, where the
        // color beneath does not count.
        let opaque = above.map(|fraction| to_byte_f64(fraction * 255.0));
        let normal = mode == BlendMode::Normal;
        // Whether full coverage, fully through any clip, gives an amount of
        // 1 in the normal mode, so that such a cell gets the opaque color.
        let full = normal && 255.0 * alpha == 1.0;
        // The same without a clip.
        let solid = full && clip.is_none();
        // Whether every component of the color is a number. Mixing a color
        // that is not a number gives 0 even for an amount of 0, so only a
        // plain color lets a cell with an amount of 0 be left as it is.
        let plain = above.iter().all(|fraction| !fraction.is_nan());
        // With a plain color and an opacity that is a number, a cell the
        // clip does not let through gets an amount of 0 and is left as it
        // is. A NaN opacity gives NaN amounts whatever the clip holds.
        let trim = plain && !alpha.is_nan();
        // The columns and rows of the coverage that fall in the canvas.
        let span = |start: i64, length: usize, limit: usize| {
            let first = start.saturating_neg().clamp(0, length as i64) as usize;
            let last = (limit as i64).saturating_sub(start).clamp(0, length as i64) as usize;
            (first, last.max(first))
        };
        let (origin_x, origin_y) = (self.left, self.top);
        let image_width = self.image.width;
        let (first_column, last_column) =
            span(left.saturating_sub(origin_x as i64), width, image_width);
        let (first_row, last_row) = span(
            top.saturating_sub(origin_y as i64),
            height,
            self.image.height,
        );
        let data = &mut self.image.data;
        // Paint the cells of one row from one column up to another.
        let mut paint_cells = |row: usize, first_column: usize, last_column: usize| {
            let y = (top + row as i64) as usize;
            let mut columns = (first_column, last_column);
            // The clip cells lined up with the coverage, when the row is cut
            // to the columns the clip row holds.
            let mut clip_cells: Option<&[u8]> = None;
            // The clip row looked up cell by cell otherwise.
            let mut clip_row: Option<(usize, &[u8])> = None;
            if let Some(clip) = clip {
                let (clip_left, cells) = clip.row(y);
                if trim {
                    let (low, high) = (first_column as i64, last_column as i64);
                    let from = (clip_left as i64 - left).clamp(low, high) as usize;
                    let to =
                        (clip_left as i64 + cells.len() as i64 - left).clamp(low, high) as usize;
                    if from >= to {
                        return;
                    }
                    let start = (left + from as i64) as usize - clip_left;
                    clip_cells = Some(&cells[start..start + (to - from)]);
                    columns = (from, to);
                } else {
                    clip_row = Some((clip_left, cells));
                }
            }
            let (first_column, last_column) = columns;
            let coverages = &coverage[row * width + first_column..row * width + last_column];
            let x_first = (left + first_column as i64) as usize;
            let start = (y - origin_y) * image_width + (x_first - origin_x);
            let pixels = &mut data[start * 4..(start + coverages.len()) * 4];
            let pixels = pixels.as_chunks_mut::<4>().0;
            if let Some(clip_cells) = clip_cells {
                for ((cells, clips), group) in coverages
                    .chunks(8)
                    .zip(clip_cells.chunks(8))
                    .zip(pixels.chunks_mut(8))
                {
                    if cells.iter().all(|&coverage| coverage == 0)
                        || clips.iter().all(|&through| through == 0)
                    {
                        continue;
                    }
                    if full
                        && cells.iter().all(|&coverage| coverage == 255)
                        && clips.iter().all(|&through| through == 255)
                    {
                        for pixel in group.iter_mut() {
                            pixel[..3].copy_from_slice(&opaque);
                        }
                        continue;
                    }
                    for ((&coverage, &through), pixel) in cells.iter().zip(clips).zip(group) {
                        if coverage == 0 {
                            continue;
                        }
                        let mut amount = f64::from(coverage) * alpha;
                        amount *= FRACTIONS[usize::from(through)];
                        // Nothing of a plain color over: mixing gives the byte beneath back.
                        if amount == 0.0 && plain {
                            continue;
                        }
                        if amount == 1.0 && normal {
                            pixel[..3].copy_from_slice(&opaque);
                            continue;
                        }
                        for (value, &fraction) in pixel[..3].iter_mut().zip(&above) {
                            *value = to_byte_f64(mode.mix(
                                FRACTIONS[usize::from(*value)],
                                fraction,
                                amount,
                            ));
                        }
                    }
                }
                return;
            }
            // Eight cells at a time, so that long empty runs go by quickly.
            let mut x_group = x_first;
            for (cells, group) in coverages.chunks(8).zip(pixels.chunks_mut(8)) {
                let first = x_group;
                x_group += cells.len();
                if cells.iter().all(|&coverage| coverage == 0) {
                    continue;
                }
                if solid && cells.iter().all(|&coverage| coverage == 255) {
                    for pixel in group.iter_mut() {
                        pixel[..3].copy_from_slice(&opaque);
                    }
                    continue;
                }
                for (x, (&coverage, pixel)) in (first..).zip(cells.iter().zip(group)) {
                    if coverage == 0 {
                        continue;
                    }
                    let mut amount = f64::from(coverage) * alpha;
                    if let Some((clip_left, clip_row)) = clip_row {
                        let through = match x.checked_sub(clip_left) {
                            Some(column) => clip_row.get(column).copied().unwrap_or(0),
                            None => 0,
                        };
                        amount *= FRACTIONS[usize::from(through)];
                    }
                    // Nothing of a plain color over: mixing gives the byte beneath back.
                    if amount == 0.0 && plain {
                        continue;
                    }
                    if amount == 1.0 && normal {
                        pixel[..3].copy_from_slice(&opaque);
                        continue;
                    }
                    for (value, &fraction) in pixel[..3].iter_mut().zip(&above) {
                        *value =
                            to_byte_f64(mode.mix(FRACTIONS[usize::from(*value)], fraction, amount));
                    }
                }
            }
        };
        match runs {
            // Every other cell is empty, and an empty cell paints nothing.
            Some(runs) => {
                for &(row, from, to) in runs {
                    let (from, to) = (from.max(first_column), to.min(last_column));
                    if (first_row..last_row).contains(&row) && from < to {
                        paint_cells(row, from, to);
                    }
                }
            }
            None => {
                if first_column < last_column {
                    for row in first_row..last_row {
                        paint_cells(row, first_column, last_column);
                    }
                }
            }
        }
    }

    /// Where a pixel of the page starts in the pixel bytes, if the canvas
    /// holds it.
    #[inline]
    fn start(&self, x: usize, y: usize) -> Option<usize> {
        let column = x.checked_sub(self.left)?;
        let row = y.checked_sub(self.top)?;
        if column >= self.image.width || row >= self.image.height {
            return None;
        }
        Some((row * self.image.width + column) * 4)
    }

    /// Set one pixel's color, leaving its alpha as it is. A pixel outside
    /// the canvas is left out.
    #[inline]
    pub(crate) fn set_rgb(&mut self, x: usize, y: usize, color: [u8; 3]) {
        let Some(start) = self.start(x, y) else {
            return;
        };
        if let Some(pixel) = self.image.data.get_mut(start..start + 3) {
            pixel.copy_from_slice(&color);
        }
    }

    /// Mix a color, with components from 0 to 255, into one pixel by an
    /// amount from 0 to 1. A pixel outside the canvas is left out.
    pub(crate) fn blend(
        &mut self,
        x: usize,
        y: usize,
        color: [f64; 3],
        amount: f64,
        mode: BlendMode,
    ) {
        let amount = amount.clamp(0.0, 1.0);
        let Some(start) = self.start(x, y) else {
            return;
        };
        if let Some(pixel) = self.image.data.get_mut(start..start + 3) {
            // Fully over in the normal mode, the color beneath does not
            // count: this is what mixing gives.
            if amount == 1.0 && mode == BlendMode::Normal {
                for (value, component) in pixel.iter_mut().zip(color) {
                    *value = to_byte_f64((component / 255.0) * 255.0);
                }
                return;
            }
            for (value, component) in pixel.iter_mut().zip(color) {
                *value = to_byte_f64(mode.mix(
                    FRACTIONS[usize::from(*value)],
                    component / 255.0,
                    amount,
                ));
            }
        }
    }
}

/// An edge of a polygon, from its upper end down.
struct Edge {
    top: f64,
    bottom: f64,
    /// Horizontal position at `top`.
    x: f64,
    /// Horizontal change for each unit down.
    slope: f64,
    /// 1 when the polygon runs down this edge, -1 when it runs up.
    direction: i32,
}

/// The edges of polygons, and the box that holds their points.
struct Shape {
    edges: Vec<Edge>,
    min_x: f64,
    max_x: f64,
    min_y: f64,
    max_y: f64,
}

impl Shape {
    /// The edges of polygons, each closed from its last point to its
    /// first. A polygon with a point that is not finite is left out.
    fn new(polygons: &[Vec<Point>]) -> Self {
        let points: usize = polygons.iter().map(Vec::len).sum();
        let mut edges = Vec::with_capacity(points.min(EDGES_AT_FIRST));
        let (mut min_x, mut max_x) = (f64::INFINITY, f64::NEG_INFINITY);
        let (mut min_y, mut max_y) = (f64::INFINITY, f64::NEG_INFINITY);
        for polygon in polygons {
            if polygon.len() < 2
                || polygon
                    .iter()
                    .any(|point| !point.0.is_finite() || !point.1.is_finite())
            {
                continue;
            }
            let ends = polygon.iter().skip(1).chain(polygon.first());
            for (&(x0, y0), &(x1, y1)) in polygon.iter().zip(ends) {
                min_x = min_x.min(x0);
                max_x = max_x.max(x0);
                min_y = min_y.min(y0);
                max_y = max_y.max(y0);
                if y0 == y1 {
                    continue;
                }
                let slope = (x1 - x0) / (y1 - y0);
                edges.push(if y0 < y1 {
                    Edge {
                        top: y0,
                        bottom: y1,
                        x: x0,
                        slope,
                        direction: 1,
                    }
                } else {
                    Edge {
                        top: y1,
                        bottom: y0,
                        x: x1,
                        slope,
                        direction: -1,
                    }
                });
            }
        }
        Self {
            edges,
            min_x,
            max_x,
            min_y,
            max_y,
        }
    }

    /// The shape's box, cut to a window, and whether the shape covers
    /// nothing there: it has no edge that goes down, or its box misses the
    /// window.
    fn reach(&self, window: &Window) -> (Window, bool) {
        let clamp = |value: f64, low: usize, high: usize| {
            value.clamp(low as f64, high.max(low) as f64) as usize
        };
        let reach = Window {
            left: clamp(self.min_x.floor(), window.left, window.right),
            top: clamp(self.min_y.floor(), window.top, window.bottom),
            right: clamp(self.max_x.ceil(), window.left, window.right),
            bottom: clamp(self.max_y.ceil(), window.top, window.bottom),
        };
        let empty = self.edges.is_empty() || reach.width() == 0 || reach.height() == 0;
        (reach, empty)
    }
}

/// The box of pixels that `coverage` gives these polygons within a window,
/// or `None` where it gives an empty mask.
pub(crate) fn reach(window: &Window, polygons: &[Vec<Point>]) -> Option<Window> {
    let (reach, empty) = Shape::new(polygons).reach(window);
    (!empty).then_some(reach)
}

/// The coverage of polygons, each closed from its last point to its first,
/// within a window of the page image. The polygons and the mask are placed
/// in page coordinates, so a cell holds the same coverage whichever window
/// it is found through.
pub(crate) fn coverage(window: &Window, polygons: &[Vec<Point>], rule: FillRule) -> Mask {
    let shape = Shape::new(polygons);
    let (reach, empty) = shape.reach(window);
    let edges = shape.edges;
    let mut mask = Mask {
        left: reach.left,
        top: reach.top,
        width: reach.width(),
        height: reach.height(),
        data: Vec::new(),
        runs: None,
    };
    if empty {
        mask.width = 0;
        mask.height = 0;
        return mask;
    }
    mask.data = vec![0; mask.width * mask.height];
    if let [first, second] = edges.as_slice()
        && first.slope == 0.0
        && second.slope == 0.0
        && first.top == second.top
        && first.bottom == second.bottom
        && first.direction != second.direction
    {
        rectangle(&mut mask, first, second);
        return mask;
    }

    scan(&mut mask, edges, rule);
    mask
}

/// Order crossings by position, keeping the order of those at the same
/// place, as a stable sort does.
fn sort_crossings(crossings: &mut [(f64, i32)]) {
    if crossings.len() > FEW_CROSSINGS {
        crossings.sort_by(|a, b| a.0.total_cmp(&b.0));
        return;
    }
    for index in 1..crossings.len() {
        let item = crossings[index];
        let mut place = index;
        while place > 0 && crossings[place - 1].0.total_cmp(&item.0).is_gt() {
            crossings[place] = crossings[place - 1];
            place -= 1;
        }
        crossings[place] = item;
    }
}

/// Fill a mask with the coverage of the shape whose edges are given,
/// sampling `SAMPLES` rows through each pixel row.
fn scan(mask: &mut Mask, mut edges: Vec<Edge>, rule: FillRule) {
    edges.sort_by(|a, b| a.top.total_cmp(&b.top));
    // Where each sample row lies below the top of its pixel row.
    let mut offsets = [0.0_f64; SAMPLES];
    for (sample, offset) in offsets.iter_mut().enumerate() {
        *offset = (sample as f64 + 0.5) / SAMPLES as f64;
    }
    let mut next = 0;
    let mut active: Vec<&Edge> = Vec::with_capacity(edges.len().min(64));
    let mut sums = vec![0.0_f32; mask.width];
    // The runs of cells that spans reached in the row at hand.
    let mut touched: Vec<(usize, usize)> = Vec::with_capacity(16);
    // The same, for every row, without overlaps: what painting goes by.
    let mut runs: Vec<Run> = Vec::with_capacity(mask.height);
    let mut crossings: Vec<(f64, i32)> = Vec::with_capacity(16);
    for row in 0..mask.height {
        let y = (mask.top + row) as f64;
        // The edges that reach into this row: those that start above its
        // bottom and end below its top.
        while next < edges.len() && edges[next].top < y + 1.0 {
            active.push(&edges[next]);
            next += 1;
        }
        active.retain(|edge| edge.bottom > y);
        if active.is_empty() {
            continue;
        }
        touched.clear();
        for &offset in &offsets {
            let sample_y = y + offset;
            crossings.clear();
            for edge in &active {
                if edge.top <= sample_y && sample_y < edge.bottom {
                    let x = edge.x + (sample_y - edge.top) * edge.slope;
                    crossings.push((x, edge.direction));
                }
            }
            sort_crossings(&mut crossings);
            let mut winding = 0;
            let mut start = 0.0;
            for &(x, direction) in &crossings {
                let was_inside = inside(winding, rule);
                winding += direction;
                match (was_inside, inside(winding, rule)) {
                    (false, true) => start = x,
                    (true, false) => {
                        let (from, to) = (start - mask.left as f64, x - mask.left as f64);
                        add_span(&mut sums, from, to);
                        // The cells the span reaches, as `add_span` has them.
                        let limit = sums.len() as f64;
                        let (from, to) = (from.clamp(0.0, limit), to.clamp(0.0, limit));
                        if to > from {
                            touched.push((from as usize, (to as usize + 1).min(sums.len())));
                        }
                    }
                    _ => {}
                }
            }
        }
        // The cells spans reached, as runs that do not overlap, for painting.
        let first_run = runs.len();
        touched.sort_unstable();
        for &(from, to) in &touched {
            match runs.last_mut() {
                Some(last) if last.0 == row && from <= last.2 => last.2 = last.2.max(to),
                _ => runs.push((row, from, to)),
            }
        }
        // Only the cells in these runs hold anything; the rest stay zero,
        // as their sums of nothing would make them. A cell in a run whose
        // sum is nothing gets the zero it already holds. A cell covered in
        // every sample row holds five ones, which give 255.
        let cells = &mut mask.data[row * mask.width..(row + 1) * mask.width];
        for &(_, from, to) in &runs[first_run..] {
            for (cell, sum) in cells[from..to].iter_mut().zip(&mut sums[from..to]) {
                let value = *sum;
                *cell = if value == SAMPLES as f32 {
                    255
                } else {
                    to_byte(value / SAMPLES as f32 * 255.0)
                };
                *sum = 0.0;
            }
        }
    }
    mask.runs = Some(runs);
}

/// Fill a mask with the coverage of an upright rectangle given by its two
/// vertical edges, as the general scan would: every sample row inside it
/// adds the same span, so each cell's sum is that span's part of the cell
/// added once for each such row, in the same order.
fn rectangle(mask: &mut Mask, first: &Edge, second: &Edge) {
    let (top, bottom) = (first.top, first.bottom);
    let (start, end) = if first.x <= second.x {
        (first.x, second.x)
    } else {
        (second.x, first.x)
    };
    let mut span = vec![0.0_f32; mask.width];
    add_span(&mut span, start - mask.left as f64, end - mask.left as f64);
    // The coverage bytes for each count of sample rows inside.
    let mut rows: Vec<Vec<u8>> = Vec::with_capacity(SAMPLES + 1);
    for count in 0..=SAMPLES {
        rows.push(
            span.iter()
                .map(|&part| {
                    let mut sum = 0.0_f32;
                    for _ in 0..count {
                        sum += part;
                    }
                    to_byte(sum / SAMPLES as f32 * 255.0)
                })
                .collect(),
        );
    }
    for row in 0..mask.height {
        let y = (mask.top + row) as f64;
        let count = (0..SAMPLES)
            .filter(|&sample| {
                let sample_y = y + (sample as f64 + 0.5) / SAMPLES as f64;
                top <= sample_y && sample_y < bottom
            })
            .count();
        mask.data[row * mask.width..(row + 1) * mask.width].copy_from_slice(&rows[count]);
    }
}

fn inside(winding: i32, rule: FillRule) -> bool {
    match rule {
        FillRule::NonZero => winding != 0,
        FillRule::EvenOdd => winding % 2 != 0,
    }
}

/// Add the part of each cell that the span from `start` to `end` covers.
fn add_span(sums: &mut [f32], start: f64, end: f64) {
    let start = start.clamp(0.0, sums.len() as f64);
    let end = end.clamp(0.0, sums.len() as f64);
    if end <= start {
        return;
    }
    let (first, last) = (start as usize, end as usize);
    if first == last {
        sums[first] += (end - start) as f32;
        return;
    }
    sums[first] += (first as f64 + 1.0 - start) as f32;
    for sum in &mut sums[first + 1..last] {
        *sum += 1.0;
    }
    if let Some(sum) = sums.get_mut(last) {
        *sum += (end - last as f64) as f32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rectangles_are_covered_as_the_scan_covers_them() {
        let positions = [0.0, 0.25, 0.4, 1.0, 1.5, 2.7, 3.0, 4.99, 7.2];
        for &left in &positions {
            for &top in &positions {
                for (width, height) in [(0.3, 0.1), (1.0, 1.0), (2.6, 3.3), (5.0, 0.7)] {
                    let edge = |x: f64, direction: i32| Edge {
                        top,
                        bottom: top + height,
                        x,
                        slope: 0.0,
                        direction,
                    };
                    let (first, second) = (edge(left, -1), edge(left + width, 1));
                    let empty = Mask {
                        left: left.floor() as usize,
                        top: top.floor() as usize,
                        width: ((left + width).ceil() - left.floor()) as usize,
                        height: ((top + height).ceil() - top.floor()) as usize,
                        data: Vec::new(),
                        runs: None,
                    };
                    let mut quick = empty.clone();
                    quick.data = vec![0; quick.width * quick.height];
                    let mut scanned = quick.clone();
                    rectangle(&mut quick, &first, &second);
                    scan(&mut scanned, vec![first, second], FillRule::NonZero);
                    assert_eq!(quick.data, scanned.data, "{left} {top} {width} {height}");
                }
            }
        }
    }

    #[test]
    fn bytes_round_as_the_library_rounds() {
        let mut values: Vec<f32> = (-4000..=300_000).map(|step| step as f32 / 1000.0).collect();
        values.extend([
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            -0.0,
            0.5,
            -0.5,
            254.5,
            255.5,
            0.499_999_97,
            254.499_98,
            1e20,
            -1e20,
        ]);
        // Every bit pattern of a sample of floats, including subnormals.
        values.extend((0..u32::MAX).step_by(65_537).map(f32::from_bits));
        for value in values {
            let expected = value.round().clamp(0.0, 255.0) as u8;
            assert_eq!(to_byte(value), expected, "{value}");
            let double = f64::from(value);
            let expected = double.round().clamp(0.0, 255.0) as u8;
            assert_eq!(to_byte_f64(double), expected, "{double}");
            let near = double + 1e-9;
            assert_eq!(
                to_byte_f64(near),
                near.round().clamp(0.0, 255.0) as u8,
                "{near}"
            );
        }
    }

    fn square(left: f64, top: f64, size: f64) -> Vec<Point> {
        vec![
            (left, top),
            (left + size, top),
            (left + size, top + size),
            (left, top + size),
        ]
    }

    #[test]
    fn covers_whole_and_partial_pixels() {
        let mask = coverage(
            &Window::whole(10, 10),
            &[square(2.0, 3.0, 2.25)],
            FillRule::NonZero,
        );
        assert_eq!((mask.left, mask.top, mask.width, mask.height), (2, 3, 3, 3));
        // Whole pixels, then a quarter of a pixel along the right, exactly,
        // and along the bottom, where one sample row in five is inside.
        assert_eq!(mask.data[..3], [255, 255, 64]);
        assert_eq!(mask.data[6..], [51, 51, 13]);
    }

    #[test]
    fn fill_rules_differ_where_shapes_overlap() {
        let shapes = [square(0.0, 0.0, 4.0), square(1.0, 1.0, 2.0)];
        let nonzero = coverage(&Window::whole(4, 4), &shapes, FillRule::NonZero);
        let even_odd = coverage(&Window::whole(4, 4), &shapes, FillRule::EvenOdd);
        assert_eq!(nonzero.data[5], 255);
        assert_eq!(even_odd.data[5], 0);
        assert_eq!(even_odd.data[0], 255);
        // A hole wound the other way is empty under both rules.
        let mut hole = square(1.0, 1.0, 2.0);
        hole.reverse();
        let ring = coverage(
            &Window::whole(4, 4),
            &[square(0.0, 0.0, 4.0), hole],
            FillRule::NonZero,
        );
        assert_eq!(ring.data[5], 0);
    }

    #[test]
    fn shapes_outside_the_image_are_clipped_or_empty() {
        let image = Window::whole(4, 4);
        let mask = coverage(&image, &[square(-10.0, -10.0, 12.0)], FillRule::NonZero);
        assert_eq!((mask.width, mask.height), (2, 2));
        assert_eq!(mask.data, [255; 4]);
        let outside = coverage(&image, &[square(10.0, 10.0, 5.0)], FillRule::NonZero);
        assert!(outside.data.is_empty());
        let broken = coverage(
            &image,
            &[vec![(0.0, 0.0), (f64::NAN, 1.0)]],
            FillRule::NonZero,
        );
        assert!(broken.data.is_empty());
    }

    #[test]
    fn paints_through_alpha_and_clip_and_encodes_png() {
        let whole = Window::whole(2, 1);
        let mut canvas = Canvas::new(&whole);
        let mask = coverage(&whole, &[square(0.0, 0.0, 2.0)], FillRule::NonZero);
        canvas.paint(
            &mask,
            [1.0, 0.0, 0.0],
            0.5,
            Some(&Mask {
                left: 0,
                top: 0,
                width: 1,
                height: 1,
                data: vec![255],
                runs: None,
            }),
            BlendMode::Normal,
        );
        let image = canvas.into_image();
        assert_eq!(image.pixel(0, 0), Some([255, 128, 128, 255]));
        assert_eq!(image.pixel(1, 0), Some([255, 255, 255, 255]));
        assert_eq!(image.pixel(2, 0), None);

        let png = image.to_png();
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        // The checksum of the empty end chunk is a fixed value.
        assert!(png.ends_with(&[0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xAE, 0x42, 0x60, 0x82]));
    }

    /// Shapes that try the upright rectangle, slanted edges, a ring, and
    /// places partly and wholly outside a 40 by 30 image.
    fn shapes() -> Vec<Vec<Vec<Point>>> {
        let mut hole = square(12.3, 9.1, 9.4);
        hole.reverse();
        vec![
            vec![square(3.3, 4.7, 20.45)],
            vec![vec![(10.2, 6.6), (33.9, 6.6), (33.9, 21.25), (10.2, 21.25)]],
            vec![vec![(5.5, 2.25), (36.75, 11.5), (17.1, 27.9)]],
            vec![vec![(1.0, 28.5), (38.5, 1.5), (39.5, 3.0), (2.5, 29.5)]],
            vec![square(6.6, 3.2, 22.7), hole],
            vec![square(6.6, 3.2, 22.7), square(12.3, 9.1, 9.4)],
            vec![square(-7.5, -3.25, 20.0)],
            vec![square(31.4, 22.2, 50.0)],
            vec![vec![
                (-20.0, 15.3),
                (20.4, -12.0),
                (61.0, 14.9),
                (19.7, 44.0),
            ]],
            vec![square(100.0, 100.0, 5.0)],
            vec![square(-30.0, 4.0, 5.0)],
            vec![vec![(2.0, 5.0), (30.0, 5.0)]],
            vec![vec![(0.0, 0.0), (f64::NAN, 1.0)]],
            Vec::new(),
        ]
    }

    /// Windows of a 40 by 30 image: in the middle, at each edge, one pixel,
    /// and the whole.
    fn windows() -> Vec<Window> {
        let window = |left, top, right, bottom| Window {
            left,
            top,
            right,
            bottom,
        };
        vec![
            window(0, 0, 40, 30),
            window(7, 5, 29, 23),
            window(13, 11, 14, 12),
            window(0, 0, 9, 30),
            window(31, 0, 40, 30),
            window(0, 0, 40, 6),
            window(0, 24, 40, 30),
            window(18, 3, 37, 9),
            window(33, 20, 40, 30),
        ]
    }

    #[test]
    fn coverage_in_a_window_is_that_part_of_the_whole_coverage() {
        let page = Window::whole(40, 30);
        for (number, polygons) in shapes().iter().enumerate() {
            for rule in [FillRule::NonZero, FillRule::EvenOdd] {
                let whole = coverage(&page, polygons, rule);
                for window in windows() {
                    let part = coverage(&window, polygons, rule);
                    // The mask stays inside its window.
                    if part.width > 0 {
                        assert!(part.left >= window.left && part.top >= window.top);
                        assert!(part.left + part.width <= window.right);
                        assert!(part.top + part.height <= window.bottom);
                        assert_eq!(part.data.len(), part.width * part.height);
                    }
                    for y in window.top..window.bottom {
                        for x in window.left..window.right {
                            assert_eq!(
                                part.at(x, y),
                                whole.at(x, y),
                                "shape {number} {rule:?} {window:?} at {x} {y}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn reach_is_the_box_of_the_coverage() {
        let mut pages = windows();
        pages.push(Window::whole(4, 4));
        for polygons in shapes() {
            for page in &pages {
                let mask = coverage(page, &polygons, FillRule::NonZero);
                let expected = (mask.width > 0).then_some(Window {
                    left: mask.left,
                    top: mask.top,
                    right: mask.left + mask.width,
                    bottom: mask.top + mask.height,
                });
                assert_eq!(reach(page, &polygons), expected, "{polygons:?} {page:?}");
                assert_eq!(mask.width == 0, mask.height == 0);
            }
        }
    }

    /// The pixels of a window, cut out of an image of the whole page.
    fn crop(image: &Pixmap, window: &Window) -> Vec<u8> {
        let mut pixels = Vec::new();
        for y in window.top..window.bottom {
            let start = (y * image.width + window.left) * 4;
            pixels.extend(&image.data[start..start + window.width() * 4]);
        }
        pixels
    }

    #[test]
    fn painting_a_window_gives_that_part_of_the_whole_painting() {
        let page = Window::whole(40, 30);
        let clips = [
            None,
            Some(vec![vec![(4.4, 3.3), (35.2, 8.8), (22.0, 28.6)]]),
            Some(vec![square(9.5, 7.5, 14.25)]),
        ];
        // Coverage placed by hand, partly off each side, with no runs.
        let stamp: Vec<u8> = (0..12 * 9).map(|cell| (cell * 37 % 256) as u8).collect();
        let places = [(-5_i64, -4_i64), (33, 25), (14, 10), (-40, 3), (60, 60)];
        for clip in &clips {
            for window in windows() {
                let mut whole = Canvas::new(&page);
                let mut part = Canvas::new(&window);
                for (canvas, bounds) in [(&mut whole, &page), (&mut part, &window)] {
                    let clip = clip
                        .as_ref()
                        .map(|clip| coverage(bounds, clip, FillRule::NonZero));
                    let clip = clip.as_ref();
                    for (number, polygons) in shapes().iter().enumerate() {
                        // Scanned masks carry runs; the rectangle's does not.
                        let mask = coverage(bounds, polygons, FillRule::EvenOdd);
                        let color = [number as f64 / 13.0, 0.25, 1.0 - number as f64 / 13.0];
                        let mode = match number % 3 {
                            0 => BlendMode::Normal,
                            1 => BlendMode::Multiply,
                            _ => BlendMode::Screen,
                        };
                        let alpha = if number % 2 == 0 { 1.0 } else { 0.6 };
                        canvas.paint(&mask, color, alpha, clip, mode);
                    }
                    for &(left, top) in &places {
                        let placed = Placed {
                            coverage: &stamp,
                            width: 12,
                            height: 9,
                            left,
                            top,
                            runs: None,
                        };
                        canvas.paint_at(&placed, [0.1, 0.7, 0.3], 0.8, clip, BlendMode::Normal);
                    }
                    // Single pixels, inside the page and beyond it.
                    for (x, y) in [(0, 0), (39, 29), (13, 11), (40, 3), (5, 30), (900, 900)] {
                        canvas.set_rgb(x, y, [9, 8, 7]);
                        canvas.blend(x + 1, y, [200.0, 10.0, 90.0], 0.5, BlendMode::Darken);
                    }
                }
                let (whole, part) = (whole.into_image(), part.into_image());
                assert_eq!((part.width, part.height), (window.width(), window.height()));
                assert_eq!(part.data, crop(&whole, &window), "{window:?}");
            }
        }
    }

    #[test]
    fn a_color_that_is_not_a_number_paints_zero_where_the_clip_stops_it() {
        // A calculator tint transform can give a component that is not a
        // number. Mixing it gives 0 even for an amount of 0, so a covered
        // cell is written although the clip lets nothing through.
        let whole = Window::whole(20, 2);
        let mut canvas = Canvas::new(&whole);
        // Full coverage but for the last cell of each row, with no runs.
        let mut data = vec![255; 20 * 2];
        data[19] = 0;
        data[39] = 0;
        let mask = Mask {
            left: 0,
            top: 0,
            width: 20,
            height: 2,
            data,
            runs: None,
        };
        // The clip reaches five cells of the first row and none of the second.
        let clip = Mask {
            left: 3,
            top: 0,
            width: 5,
            height: 1,
            data: vec![255, 0, 128, 0, 255],
            runs: None,
        };
        let color = [f64::NAN, 0.25, f64::NAN];
        canvas.paint(&mask, color, 1.0, Some(&clip), BlendMode::Normal);
        let image = canvas.into_image();
        for y in 0..2 {
            for x in 0..20 {
                let green = match (x, y) {
                    (3 | 7, 0) => 64,
                    (5, 0) => 159,
                    _ => 255,
                };
                let expected = if x == 19 {
                    [255, 255, 255, 255]
                } else {
                    [0, green, 0, 255]
                };
                assert_eq!(image.pixel(x, y), Some(expected), "{x} {y}");
            }
        }
    }

    #[test]
    fn a_color_that_is_not_a_number_paints_zero_at_no_opacity() {
        let whole = Window::whole(12, 1);
        let shape = [square(1.0, 0.0, 9.25)];
        // A scanned mask carries runs; the same coverage is tried without.
        let scanned = coverage(&whole, &shape, FillRule::EvenOdd);
        assert_eq!(scanned.data[9], 64);
        let mut plain = scanned.clone();
        plain.runs = None;
        for mask in [&scanned, &plain] {
            let mut canvas = Canvas::new(&whole);
            canvas.paint(mask, [f64::NAN; 3], 0.0, None, BlendMode::Normal);
            let image = canvas.into_image();
            for x in 0..12 {
                let expected = if (1..11).contains(&x) {
                    [0, 0, 0, 255]
                } else {
                    [255, 255, 255, 255]
                };
                assert_eq!(image.pixel(x, 0), Some(expected), "{x}");
            }
            // A color that is a number leaves every pixel as it was.
            let mut canvas = Canvas::new(&whole);
            canvas.paint(mask, [0.2, 0.4, 0.6], 0.0, None, BlendMode::Normal);
            assert_eq!(canvas.image.data, vec![255; 12 * 4]);
        }
    }

    #[test]
    fn single_pixels_outside_a_canvas_are_left_out() {
        let window = Window {
            left: 2,
            top: 1,
            right: 5,
            bottom: 3,
        };
        let mut canvas = Canvas::new(&window);
        // To the left, to the right, above, and below: a pixel past the
        // right edge must not land at the start of the next row.
        for (x, y) in [(1, 1), (5, 1), (6, 1), (3, 0), (3, 3), (0, 0)] {
            canvas.set_rgb(x, y, [0, 0, 0]);
            canvas.blend(x, y, [0.0; 3], 1.0, BlendMode::Normal);
        }
        assert_eq!(canvas.image.data, vec![255; 3 * 2 * 4]);
        canvas.set_rgb(4, 2, [1, 2, 3]);
        assert_eq!(canvas.into_image().pixel(2, 1), Some([1, 2, 3, 255]));
    }

    #[test]
    fn points_that_give_no_edge_set_no_room_aside() {
        // A long flat polygon, and a long one with a point that is not
        // finite: neither gives an edge, however many points it holds.
        let flat: Vec<Point> = (0..100_000).map(|x| (f64::from(x), 3.0)).collect();
        let mut broken: Vec<Point> = (0..100_000)
            .map(|x| (f64::from(x), f64::from(x % 7)))
            .collect();
        broken[50_000].1 = f64::NAN;
        let shape = Shape::new(&[flat, broken]);
        assert!(shape.edges.is_empty());
        assert!(shape.edges.capacity() <= EDGES_AT_FIRST);
        // A polygon with more edges than that still gets them all.
        let saw: Vec<Point> = (0..1_000)
            .map(|x| (f64::from(x), f64::from(x % 2)))
            .collect();
        assert_eq!(Shape::new(&[saw]).edges.len(), 1_000);
    }
}
