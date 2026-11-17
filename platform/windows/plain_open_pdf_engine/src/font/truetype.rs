//! TrueType font programs: glyph outlines and character maps, as far as
//! drawing text needs them (the OpenType specification's `head`, `maxp`,
//! `loca`, `glyf`, and `cmap` tables).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::Arc;

/// How deeply composite glyphs may nest.
const MAX_COMPONENT_DEPTH: usize = 8;
/// The most points one glyph may have, to bound work on hostile fonts.
const MAX_POINTS: usize = 100_000;

/// One step of a glyph outline, in font units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Segment {
    /// Start a contour at a point.
    Move(f64, f64),
    /// A straight line to a point.
    Line(f64, f64),
    /// A quadratic curve through a control point to a point.
    Quad(f64, f64, f64, f64),
    /// A cubic curve through two control points to a point.
    Cubic(f64, f64, f64, f64, f64, f64),
    /// Close the contour back to its start.
    Close,
}

/// A point of a contour, on the curve or a control point off it.
#[derive(Clone, Copy)]
struct Point {
    x: f64,
    y: f64,
    on_curve: bool,
}

/// A character map subtable.
#[derive(Debug, Clone, Copy)]
struct CharacterMap {
    platform: u16,
    encoding: u16,
    offset: usize,
}

/// A parsed TrueType font program.
#[derive(Debug)]
pub struct TrueTypeFont {
    data: Vec<u8>,
    glyph_count: usize,
    units_per_em: u16,
    long_offsets: bool,
    /// Where the `loca` and `glyf` tables start.
    locations: usize,
    glyphs: usize,
    maps: Vec<CharacterMap>,
    /// Where the `hmtx` table starts, and how many glyphs have their own
    /// advance in it.
    metrics: Option<(usize, usize)>,
    /// Outlines already read, by glyph index.
    outlines: RefCell<BTreeMap<u16, Arc<Vec<Segment>>>>,
}

impl TrueTypeFont {
    /// Read the tables of a font program. A font collection yields its
    /// first font. Returns `None` for data that is not a TrueType font with
    /// outlines, such as an OpenType font with CFF outlines.
    pub fn parse(data: Vec<u8>) -> Option<Self> {
        let mut start = 0;
        if data.get(..4)? == b"ttcf" {
            start = u32_at(&data, 12)? as usize;
        }
        let version = data.get(start..start + 4)?;
        if version != [0, 1, 0, 0] && version != b"true" {
            return None;
        }
        let table_count = usize::from(u16_at(&data, start + 4)?);
        let table = |tag: &[u8; 4]| -> Option<usize> {
            (0..table_count).find_map(|index| {
                let record = start + 12 + index * 16;
                (data.get(record..record + 4)? == tag)
                    .then(|| u32_at(&data, record + 8).map(|offset| offset as usize))?
            })
        };
        let head = table(b"head")?;
        let units_per_em = u16_at(&data, head + 18)?;
        let long_offsets = u16_at(&data, head + 50)? != 0;
        let glyph_count = usize::from(u16_at(&data, table(b"maxp")? + 4)?);
        let locations = table(b"loca")?;
        let glyphs = table(b"glyf")?;
        let mut maps = Vec::new();
        if let Some(cmap) = table(b"cmap") {
            let count = usize::from(u16_at(&data, cmap + 2).unwrap_or(0));
            for index in 0..count {
                let record = cmap + 4 + index * 8;
                let (Some(platform), Some(encoding), Some(offset)) = (
                    u16_at(&data, record),
                    u16_at(&data, record + 2),
                    u32_at(&data, record + 4),
                ) else {
                    break;
                };
                maps.push(CharacterMap {
                    platform,
                    encoding,
                    offset: cmap + offset as usize,
                });
            }
        }
        let metrics = table(b"hmtx")
            .zip(table(b"hhea"))
            .and_then(|(metrics, header)| {
                Some((metrics, usize::from(u16_at(&data, header + 34)?)))
            });
        (units_per_em > 0).then_some(Self {
            data,
            glyph_count,
            units_per_em,
            long_offsets,
            locations,
            glyphs,
            maps,
            metrics,
            outlines: RefCell::new(BTreeMap::new()),
        })
    }

    /// The number of glyphs in the font.
    pub fn glyph_count(&self) -> usize {
        self.glyph_count
    }

    /// The font units that make up the font size.
    pub fn units_per_em(&self) -> u16 {
        self.units_per_em
    }

    /// How far a glyph moves the pen, in font units, if the font says.
    pub fn advance(&self, glyph: u16) -> Option<u16> {
        let (metrics, count) = self.metrics?;
        // Glyphs past the last entry share its advance.
        let index = usize::from(glyph).min(count.checked_sub(1)?);
        u16_at(&self.data, metrics + index * 4)
    }

    /// Whether the font has a character map for a platform and encoding,
    /// such as (3, 1) for Windows Unicode.
    pub fn has_map(&self, platform: u16, encoding: u16) -> bool {
        self.maps
            .iter()
            .any(|map| map.platform == platform && map.encoding == encoding)
    }

    /// The glyph a character code selects in the character map for a
    /// platform and encoding; `None` when the map is missing or has no
    /// glyph for the code.
    pub fn glyph_index(&self, platform: u16, encoding: u16, code: u32) -> Option<u16> {
        let map = self
            .maps
            .iter()
            .find(|map| map.platform == platform && map.encoding == encoding)?;
        let at = map.offset;
        let u16_at = |offset: usize| u16_at(&self.data, offset);
        let glyph = match u16_at(at)? {
            0 => u16::from(*self.data.get(at + 6 + usize::try_from(code).ok()?)?),
            4 => {
                let code = u16::try_from(code).ok()?;
                let segments = usize::from(u16_at(at + 6)? / 2);
                let (ends, starts) = (at + 14, at + 16 + segments * 2);
                let (deltas, ranges) = (starts + segments * 2, starts + segments * 4);
                let segment = (0..segments)
                    .find(|&index| u16_at(ends + index * 2).is_some_and(|end| end >= code))?;
                let start = u16_at(starts + segment * 2)?;
                if start > code {
                    return None;
                }
                let delta = u16_at(deltas + segment * 2)?;
                let range_at = ranges + segment * 2;
                match u16_at(range_at)? {
                    0 => code.wrapping_add(delta),
                    range => {
                        let offset = range_at + usize::from(range) + usize::from(code - start) * 2;
                        match u16_at(offset)? {
                            0 => 0,
                            glyph => glyph.wrapping_add(delta),
                        }
                    }
                }
            }
            6 => {
                let first = u32::from(u16_at(at + 6)?);
                let count = u32::from(u16_at(at + 8)?);
                let index = code.checked_sub(first).filter(|&index| index < count)?;
                u16_at(at + 10 + index as usize * 2)?
            }
            12 => {
                let groups = u32_at(&self.data, at + 12)? as usize;
                let group = (0..groups.min(100_000)).find_map(|index| {
                    let record = at + 16 + index * 12;
                    let first = u32_at(&self.data, record)?;
                    let last = u32_at(&self.data, record + 4)?;
                    let glyph = u32_at(&self.data, record + 8)?;
                    (first..=last)
                        .contains(&code)
                        .then(|| glyph.checked_add(code - first))?
                })?;
                u16::try_from(group).ok()?
            }
            _ => return None,
        };
        (glyph != 0).then_some(glyph)
    }

    /// The outline of a glyph, in font units with the vertical axis up.
    /// A glyph without an outline, such as a space, and a glyph that cannot
    /// be read give no segments.
    pub fn outline(&self, glyph: u16) -> Arc<Vec<Segment>> {
        if let Some(outline) = self.outlines.borrow().get(&glyph) {
            return Arc::clone(outline);
        }
        let mut segments = Vec::new();
        let mut budget = MAX_POINTS;
        let identity = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
        if self
            .read_glyph(glyph, &identity, 0, &mut budget, &mut segments)
            .is_none()
        {
            segments.clear();
        }
        let outline = Arc::new(segments);
        self.outlines
            .borrow_mut()
            .insert(glyph, Arc::clone(&outline));
        outline
    }

    /// The bytes of a glyph's description; empty for a glyph without one.
    fn glyph_data(&self, glyph: u16) -> Option<&[u8]> {
        let glyph = usize::from(glyph);
        if glyph >= self.glyph_count {
            return None;
        }
        let location = |index: usize| -> Option<usize> {
            if self.long_offsets {
                Some(u32_at(&self.data, self.locations + index * 4)? as usize)
            } else {
                Some(usize::from(u16_at(&self.data, self.locations + index * 2)?) * 2)
            }
        };
        let (start, end) = (location(glyph)?, location(glyph + 1)?);
        self.data
            .get(self.glyphs.checked_add(start)?..self.glyphs.checked_add(end)?)
    }

    /// Append a glyph's contours, moved by a matrix `[a b c d e f]`.
    fn read_glyph(
        &self,
        glyph: u16,
        matrix: &[f64; 6],
        depth: usize,
        budget: &mut usize,
        segments: &mut Vec<Segment>,
    ) -> Option<()> {
        let data = self.glyph_data(glyph)?;
        if data.is_empty() {
            return Some(());
        }
        let contours = i16_at(data, 0)?;
        if contours >= 0 {
            return simple_glyph(data, contours as usize, matrix, budget, segments);
        }
        if depth >= MAX_COMPONENT_DEPTH {
            return None;
        }
        // A composite glyph lists other glyphs, each with a placement.
        let mut at = 10;
        loop {
            let flags = u16_at(data, at)?;
            let component = u16_at(data, at + 2)?;
            at += 4;
            let (x, y) = if flags & 0x1 != 0 {
                at += 4;
                (
                    f64::from(i16_at(data, at - 4)?),
                    f64::from(i16_at(data, at - 2)?),
                )
            } else {
                at += 2;
                let byte = |offset: usize| data.get(offset).map(|&byte| f64::from(byte as i8));
                (byte(at - 2)?, byte(at - 1)?)
            };
            // Offsets given as point numbers are not supported and count
            // as no offset.
            let (x, y) = if flags & 0x2 != 0 { (x, y) } else { (0.0, 0.0) };
            let mut fixed = || -> Option<f64> {
                at += 2;
                Some(f64::from(i16_at(data, at - 2)?) / 16_384.0)
            };
            let own = if flags & 0x8 != 0 {
                let scale = fixed()?;
                [scale, 0.0, 0.0, scale, x, y]
            } else if flags & 0x40 != 0 {
                [fixed()?, 0.0, 0.0, fixed()?, x, y]
            } else if flags & 0x80 != 0 {
                [fixed()?, fixed()?, fixed()?, fixed()?, x, y]
            } else {
                [1.0, 0.0, 0.0, 1.0, x, y]
            };
            // The component's matrix applies first, then this glyph's.
            let combined = [
                own[0] * matrix[0] + own[1] * matrix[2],
                own[0] * matrix[1] + own[1] * matrix[3],
                own[2] * matrix[0] + own[3] * matrix[2],
                own[2] * matrix[1] + own[3] * matrix[3],
                own[4] * matrix[0] + own[5] * matrix[2] + matrix[4],
                own[4] * matrix[1] + own[5] * matrix[3] + matrix[5],
            ];
            self.read_glyph(component, &combined, depth + 1, budget, segments)?;
            if flags & 0x20 == 0 {
                return Some(());
            }
        }
    }
}

fn u16_at(data: &[u8], offset: usize) -> Option<u16> {
    let bytes = data.get(offset..offset.checked_add(2)?)?;
    Some(u16::from_be_bytes([bytes[0], bytes[1]]))
}

fn i16_at(data: &[u8], offset: usize) -> Option<i16> {
    u16_at(data, offset).map(|value| value as i16)
}

fn u32_at(data: &[u8], offset: usize) -> Option<u32> {
    let bytes = data.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

/// Append the contours of a simple glyph description.
fn simple_glyph(
    data: &[u8],
    contours: usize,
    matrix: &[f64; 6],
    budget: &mut usize,
    segments: &mut Vec<Segment>,
) -> Option<()> {
    let mut ends = Vec::with_capacity(contours);
    for contour in 0..contours {
        ends.push(usize::from(u16_at(data, 10 + contour * 2)?));
    }
    let count = ends.last().map_or(0, |&last| last + 1);
    *budget = budget.checked_sub(count)?;
    let instructions = usize::from(u16_at(data, 10 + contours * 2)?);
    let mut at = 12 + contours * 2 + instructions;
    // One flag byte for each point; a flag may say it repeats.
    let mut flags = Vec::with_capacity(count);
    while flags.len() < count {
        let flag = *data.get(at)?;
        at += 1;
        let mut repeats = 0;
        if flag & 0x8 != 0 {
            repeats = usize::from(*data.get(at)?);
            at += 1;
        }
        for _ in 0..=repeats {
            flags.push(flag);
        }
    }
    flags.truncate(count);
    // Coordinates are changes from the point before: a byte with its sign
    // in the flags, a word, or no change at all.
    let mut read = |short: u8, same: u8| -> Option<Vec<f64>> {
        let mut value = 0.0;
        let mut values = Vec::with_capacity(count);
        for &flag in &flags {
            if flag & short != 0 {
                let change = f64::from(*data.get(at)?);
                at += 1;
                value += if flag & same != 0 { change } else { -change };
            } else if flag & same == 0 {
                value += f64::from(i16_at(data, at)?);
                at += 2;
            }
            values.push(value);
        }
        Some(values)
    };
    let xs = read(0x2, 0x10)?;
    let ys = read(0x4, 0x20)?;
    let point = |index: usize| Point {
        x: xs[index] * matrix[0] + ys[index] * matrix[2] + matrix[4],
        y: xs[index] * matrix[1] + ys[index] * matrix[3] + matrix[5],
        on_curve: flags[index] & 0x1 != 0,
    };
    let mut first = 0;
    for end in ends {
        if end < first || end >= count {
            return None;
        }
        let points: Vec<Point> = (first..=end).map(point).collect();
        contour(&points, segments);
        first = end + 1;
    }
    Some(())
}

/// Append one contour. Two control points in a row have a point on the
/// curve halfway between them.
fn contour(points: &[Point], segments: &mut Vec<Segment>) {
    let middle = |a: Point, b: Point| Point {
        x: (a.x + b.x) / 2.0,
        y: (a.y + b.y) / 2.0,
        on_curve: true,
    };
    let (Some(&first), Some(&last)) = (points.first(), points.last()) else {
        return;
    };
    // Start on the curve: at the first such point, or between two controls.
    let start_index = points.iter().position(|point| point.on_curve);
    let start = match start_index {
        Some(index) => points[index],
        None => middle(last, first),
    };
    segments.push(Segment::Move(start.x, start.y));
    let offset = start_index.map_or(0, |index| index + 1);
    let mut control: Option<Point> = None;
    let rest = (0..points.len()).map(|step| points[(offset + step) % points.len()]);
    // Visiting every point once more ends back at the start.
    for point in rest
        .take(points.len() - usize::from(start_index.is_some()))
        .chain([start])
    {
        match (control, point.on_curve) {
            (None, true) => segments.push(Segment::Line(point.x, point.y)),
            (None, false) => control = Some(point),
            (Some(by), true) => {
                segments.push(Segment::Quad(by.x, by.y, point.x, point.y));
                control = None;
            }
            (Some(by), false) => {
                let to = middle(by, point);
                segments.push(Segment::Quad(by.x, by.y, to.x, to.y));
                control = Some(point);
            }
        }
    }
    segments.push(Segment::Close);
}
