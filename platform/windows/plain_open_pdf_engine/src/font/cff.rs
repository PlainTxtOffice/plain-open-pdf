//! CFF font programs: glyph outlines and the tables that select a glyph,
//! as far as drawing text needs them (Adobe Technical Notes 5176, The
//! Compact Font Format Specification, and 5177, The Type 2 Charstring
//! Format).

use super::Segment;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::Arc;

/// How deeply subroutines may call one another.
const MAX_CALL_DEPTH: usize = 10;
/// The most operators one glyph may run, to bound work on hostile fonts.
const MAX_OPERATORS: usize = 200_000;
/// The most numbers the charstring stack holds.
const MAX_STACK: usize = 96;

/// The first standard strings, which every CFF font can name without
/// storing them: string 0 is `.notdef` and strings 1 to 228 are the
/// standard Latin glyph names in this order (Appendix A of the format).
/// The standard strings after these, for expert glyphs, are not included.
const STANDARD_STRINGS: [&str; 229] = [
    ".notdef",
    "space",
    "exclam",
    "quotedbl",
    "numbersign",
    "dollar",
    "percent",
    "ampersand",
    "quoteright",
    "parenleft",
    "parenright",
    "asterisk",
    "plus",
    "comma",
    "hyphen",
    "period",
    "slash",
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "colon",
    "semicolon",
    "less",
    "equal",
    "greater",
    "question",
    "at",
    "A",
    "B",
    "C",
    "D",
    "E",
    "F",
    "G",
    "H",
    "I",
    "J",
    "K",
    "L",
    "M",
    "N",
    "O",
    "P",
    "Q",
    "R",
    "S",
    "T",
    "U",
    "V",
    "W",
    "X",
    "Y",
    "Z",
    "bracketleft",
    "backslash",
    "bracketright",
    "asciicircum",
    "underscore",
    "quoteleft",
    "a",
    "b",
    "c",
    "d",
    "e",
    "f",
    "g",
    "h",
    "i",
    "j",
    "k",
    "l",
    "m",
    "n",
    "o",
    "p",
    "q",
    "r",
    "s",
    "t",
    "u",
    "v",
    "w",
    "x",
    "y",
    "z",
    "braceleft",
    "bar",
    "braceright",
    "asciitilde",
    "exclamdown",
    "cent",
    "sterling",
    "fraction",
    "yen",
    "florin",
    "section",
    "currency",
    "quotesingle",
    "quotedblleft",
    "guillemotleft",
    "guilsinglleft",
    "guilsinglright",
    "fi",
    "fl",
    "endash",
    "dagger",
    "daggerdbl",
    "periodcentered",
    "paragraph",
    "bullet",
    "quotesinglbase",
    "quotedblbase",
    "quotedblright",
    "guillemotright",
    "ellipsis",
    "perthousand",
    "questiondown",
    "grave",
    "acute",
    "circumflex",
    "tilde",
    "macron",
    "breve",
    "dotaccent",
    "dieresis",
    "ring",
    "cedilla",
    "hungarumlaut",
    "ogonek",
    "caron",
    "emdash",
    "AE",
    "ordfeminine",
    "Lslash",
    "Oslash",
    "OE",
    "ordmasculine",
    "ae",
    "dotlessi",
    "lslash",
    "oslash",
    "oe",
    "germandbls",
    "onesuperior",
    "logicalnot",
    "mu",
    "trademark",
    "Eth",
    "onehalf",
    "plusminus",
    "Thorn",
    "onequarter",
    "divide",
    "brokenbar",
    "degree",
    "thorn",
    "threequarters",
    "twosuperior",
    "registered",
    "minus",
    "eth",
    "multiply",
    "threesuperior",
    "copyright",
    "Aacute",
    "Acircumflex",
    "Adieresis",
    "Agrave",
    "Aring",
    "Atilde",
    "Ccedilla",
    "Eacute",
    "Ecircumflex",
    "Edieresis",
    "Egrave",
    "Iacute",
    "Icircumflex",
    "Idieresis",
    "Igrave",
    "Ntilde",
    "Oacute",
    "Ocircumflex",
    "Odieresis",
    "Ograve",
    "Otilde",
    "Scaron",
    "Uacute",
    "Ucircumflex",
    "Udieresis",
    "Ugrave",
    "Yacute",
    "Ydieresis",
    "Zcaron",
    "aacute",
    "acircumflex",
    "adieresis",
    "agrave",
    "aring",
    "atilde",
    "ccedilla",
    "eacute",
    "ecircumflex",
    "edieresis",
    "egrave",
    "iacute",
    "icircumflex",
    "idieresis",
    "igrave",
    "ntilde",
    "oacute",
    "ocircumflex",
    "odieresis",
    "ograve",
    "otilde",
    "scaron",
    "uacute",
    "ucircumflex",
    "udieresis",
    "ugrave",
    "yacute",
    "ydieresis",
    "zcaron",
];
/// How many standard strings the format defines in all; a font's own
/// strings are numbered after them.
const STANDARD_STRING_COUNT: usize = 391;

/// The start and end of each item of an INDEX, as positions in the data.
type Index = Vec<(usize, usize)>;

fn u16_at(data: &[u8], offset: usize) -> Option<usize> {
    Some(usize::from(*data.get(offset)?) << 8 | usize::from(*data.get(offset.checked_add(1)?)?))
}

/// Read an INDEX and give its items and the position just after it.
fn index(data: &[u8], start: usize) -> Option<(Index, usize)> {
    let count = u16_at(data, start)?;
    if count == 0 {
        return Some((Vec::new(), start + 2));
    }
    let size = usize::from(*data.get(start + 2)?);
    if !(1..=4).contains(&size) {
        return None;
    }
    let offset = |item: usize| -> Option<usize> {
        let at = start + 3 + item * size;
        let bytes = data.get(at..at.checked_add(size)?)?;
        Some(
            bytes
                .iter()
                .fold(0, |value, &byte| value << 8 | usize::from(byte)),
        )
    };
    // Offsets count from the byte before the first item's data.
    let base = start + 3 + (count + 1) * size - 1;
    let mut items = Vec::with_capacity(count);
    for item in 0..count {
        let (from, to) = (
            base.checked_add(offset(item)?)?,
            base.checked_add(offset(item + 1)?)?,
        );
        if from > to || to > data.len() {
            return None;
        }
        items.push((from, to));
    }
    let end = items.last().map_or(base + 1, |item| item.1);
    Some((items, end))
}

/// The entries of a DICT: each operator with the numbers before it. A
/// two-byte operator `12 n` is numbered `1200 + n`.
fn dictionary(data: &[u8]) -> BTreeMap<u16, Vec<f64>> {
    let mut entries = BTreeMap::new();
    let mut operands = Vec::new();
    let mut at = 0;
    while let Some(&byte) = data.get(at) {
        at += 1;
        match byte {
            0..=21 => {
                let mut operator = u16::from(byte);
                if byte == 12 {
                    operator = 1200 + u16::from(data.get(at).copied().unwrap_or(0));
                    at += 1;
                }
                entries.insert(operator, std::mem::take(&mut operands));
            }
            28 => {
                operands.push(f64::from(u16_at(data, at).unwrap_or(0) as u16 as i16));
                at += 2;
            }
            29 => {
                let bytes = data.get(at..at + 4).unwrap_or(&[0; 4]);
                let value = i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                operands.push(f64::from(value));
                at += 4;
            }
            // A real number, one digit or sign to each half byte.
            30 => {
                let mut text = String::new();
                'number: while let Some(&pair) = data.get(at) {
                    at += 1;
                    for nibble in [pair >> 4, pair & 15] {
                        match nibble {
                            0..=9 => text.push(char::from(b'0' + nibble)),
                            10 => text.push('.'),
                            11 => text.push('E'),
                            12 => text.push_str("E-"),
                            14 => text.push('-'),
                            15 => break 'number,
                            _ => {}
                        }
                    }
                }
                operands.push(text.parse().unwrap_or(0.0));
            }
            32..=246 => operands.push(f64::from(byte) - 139.0),
            247..=250 => {
                let next = f64::from(data.get(at).copied().unwrap_or(0));
                operands.push((f64::from(byte) - 247.0) * 256.0 + next + 108.0);
                at += 1;
            }
            251..=254 => {
                let next = f64::from(data.get(at).copied().unwrap_or(0));
                operands.push(-(f64::from(byte) - 251.0) * 256.0 - next - 108.0);
                at += 1;
            }
            _ => {}
        }
    }
    entries
}

/// A parsed CFF font program.
#[derive(Debug)]
pub struct CffFont {
    data: Vec<u8>,
    char_strings: Index,
    global_subroutines: Index,
    /// The local subroutines of each font dictionary; a font that is not
    /// CID-keyed has one.
    local_subroutines: Vec<Index>,
    /// For a CID-keyed font, the font dictionary of each glyph.
    dictionary_of_glyph: Vec<u8>,
    /// For each glyph, its string number, or its CID in a CID-keyed font.
    charset: Vec<u16>,
    strings: Index,
    /// The glyph the font's own encoding gives each character code; 0 for
    /// none.
    encoding: Vec<u16>,
    cid_keyed: bool,
    units_per_em: f64,
    /// Outlines already read, by glyph index.
    outlines: RefCell<BTreeMap<u16, Arc<Vec<Segment>>>>,
}

impl CffFont {
    /// Read a CFF font program, bare or inside an OpenType file. Returns
    /// `None` for data that is not a CFF font with Type 2 charstrings.
    pub fn parse(data: Vec<u8>) -> Option<Self> {
        // An OpenType file holds the font program in its `CFF ` table.
        let mut data = data;
        if data.get(..4)? == b"OTTO" {
            let tables = u16_at(&data, 4)?;
            let record = (0..tables)
                .map(|table| 12 + table * 16)
                .find(|&record| data.get(record..record + 4) == Some(b"CFF "))?;
            let field = |at: usize| -> Option<usize> {
                Some(u16_at(&data, at)? << 16 | u16_at(&data, at + 2)?)
            };
            let (start, length) = (field(record + 8)?, field(record + 12)?);
            data = data.get(start..start.checked_add(length)?)?.to_vec();
        }
        if *data.first()? != 1 {
            return None;
        }
        let header = usize::from(*data.get(2)?);
        let (_names, after) = index(&data, header)?;
        let (tops, after) = index(&data, after)?;
        let (strings, after) = index(&data, after)?;
        let (global_subroutines, _) = index(&data, after)?;
        let top = dictionary(data.get(tops.first()?.0..tops.first()?.1)?);
        let number = |entries: &BTreeMap<u16, Vec<f64>>, operator: u16| -> Option<usize> {
            let value = *entries.get(&operator)?.first()?;
            (value >= 0.0).then_some(value as usize)
        };
        if number(&top, 1206).unwrap_or(2) != 2 {
            return None;
        }
        let (char_strings, _) = index(&data, number(&top, 17)?)?;
        let glyphs = char_strings.len();
        let cid_keyed = top.contains_key(&1230);

        // The local subroutines of a private dictionary, given the entry
        // that holds its size and position.
        let subroutines = |entries: &BTreeMap<u16, Vec<f64>>| -> Index {
            let private = entries.get(&18).map(Vec::as_slice).unwrap_or_default();
            let (Some(&size), Some(&start)) = (private.first(), private.get(1)) else {
                return Vec::new();
            };
            let (size, start) = (size.max(0.0) as usize, start.max(0.0) as usize);
            let Some(bytes) = start.checked_add(size).and_then(|end| data.get(start..end)) else {
                return Vec::new();
            };
            // The position counts from the start of the private dictionary.
            let found = number(&dictionary(bytes), 19)
                .and_then(|offset| index(&data, start.checked_add(offset)?));
            found.map_or_else(Vec::new, |found| found.0)
        };
        let mut local_subroutines = Vec::new();
        let mut dictionary_of_glyph = Vec::new();
        if cid_keyed {
            let (fonts, _) = index(&data, number(&top, 1236)?)?;
            for font in &fonts {
                local_subroutines.push(subroutines(&dictionary(data.get(font.0..font.1)?)));
            }
            dictionary_of_glyph = font_dictionaries(&data, number(&top, 1237)?, glyphs)?;
        } else {
            local_subroutines.push(subroutines(&top));
        }

        let charset = match number(&top, 15).unwrap_or(0) {
            // The predefined charsets number their glyphs' strings in
            // order; only the first, of standard Latin glyphs, is known.
            0 if !cid_keyed => (0..glyphs).map(|glyph| glyph.min(228) as u16).collect(),
            0..=2 => vec![0; glyphs],
            offset => charset(&data, offset, glyphs)?,
        };
        let mut font = Self {
            char_strings,
            global_subroutines,
            local_subroutines,
            dictionary_of_glyph,
            charset,
            strings,
            encoding: vec![0; 256],
            cid_keyed,
            units_per_em: 1000.0,
            outlines: RefCell::new(BTreeMap::new()),
            data,
        };
        if let Some(&scale) = top.get(&1207).and_then(|matrix| matrix.first())
            && scale > 0.0
        {
            font.units_per_em = 1.0 / scale;
        }
        if !cid_keyed {
            font.encoding = match number(&top, 16).unwrap_or(0) {
                // The standard encoding is found by glyph name instead,
                // and the expert encoding is not known.
                0 | 1 => vec![0; 256],
                offset => encoding(&font.data, offset).unwrap_or_else(|| vec![0; 256]),
            };
        }
        Some(font)
    }

    /// The number of glyphs in the font.
    pub fn glyph_count(&self) -> usize {
        self.char_strings.len()
    }

    /// The font units that make up the font size, usually 1000.
    pub fn units_per_em(&self) -> f64 {
        self.units_per_em
    }

    /// Whether glyphs are selected by CID, not by name.
    pub fn is_cid_keyed(&self) -> bool {
        self.cid_keyed
    }

    /// The name of a glyph, if the font is not CID-keyed and the name is
    /// one of the font's own strings or of the first standard strings.
    pub fn glyph_name(&self, glyph: u16) -> Option<&str> {
        if self.cid_keyed {
            return None;
        }
        let string = usize::from(*self.charset.get(usize::from(glyph))?);
        if let Some(name) = STANDARD_STRINGS.get(string) {
            return Some(name);
        }
        let own = self
            .strings
            .get(string.checked_sub(STANDARD_STRING_COUNT)?)?;
        std::str::from_utf8(self.data.get(own.0..own.1)?).ok()
    }

    /// The glyph with a name, in a font that is not CID-keyed.
    pub fn glyph_by_name(&self, name: &str) -> Option<u16> {
        let found =
            (1..self.charset.len()).find(|&glyph| self.glyph_name(glyph as u16) == Some(name));
        found.map(|glyph| glyph as u16)
    }

    /// The glyph the font's own encoding gives a character code.
    pub fn glyph_by_code(&self, code: u8) -> Option<u16> {
        let glyph = *self.encoding.get(usize::from(code))?;
        (glyph != 0).then_some(glyph)
    }

    /// The glyph for a CID: looked up in the charset of a CID-keyed font,
    /// and the CID itself in any other.
    pub fn glyph_by_cid(&self, cid: u16) -> Option<u16> {
        if !self.cid_keyed {
            return (usize::from(cid) < self.char_strings.len()).then_some(cid);
        }
        if cid == 0 {
            return Some(0);
        }
        let found = self
            .charset
            .iter()
            .skip(1)
            .position(|&entry| entry == cid)?;
        u16::try_from(found + 1).ok()
    }

    /// The outline of a glyph, in font units with the vertical axis up.
    /// A glyph without an outline, such as a space, and a glyph that cannot
    /// be read give no segments.
    pub fn outline(&self, glyph: u16) -> Arc<Vec<Segment>> {
        if let Some(outline) = self.outlines.borrow().get(&glyph) {
            return Arc::clone(outline);
        }
        let mut reader = CharString {
            font: self,
            local: self.local_for(glyph),
            stack: Vec::new(),
            x: 0.0,
            y: 0.0,
            stems: 0,
            width_seen: false,
            open: false,
            operators: 0,
            segments: Vec::new(),
        };
        let run = self
            .char_strings
            .get(usize::from(glyph))
            .and_then(|&(start, end)| reader.run(start, end, 0));
        if run.is_none() {
            reader.segments.clear();
        } else if reader.open {
            reader.segments.push(Segment::Close);
        }
        let outline = Arc::new(reader.segments);
        self.outlines
            .borrow_mut()
            .insert(glyph, Arc::clone(&outline));
        outline
    }

    /// The local subroutines that apply to a glyph.
    fn local_for(&self, glyph: u16) -> &[(usize, usize)] {
        let which = self
            .dictionary_of_glyph
            .get(usize::from(glyph))
            .copied()
            .unwrap_or(0);
        self.local_subroutines
            .get(usize::from(which))
            .map_or(&[], Vec::as_slice)
    }
}

/// Read a charset: the string number, or CID, of each glyph after the
/// first.
fn charset(data: &[u8], start: usize, glyphs: usize) -> Option<Vec<u16>> {
    let mut entries = vec![0];
    let format = *data.get(start)?;
    let mut at = start + 1;
    while entries.len() < glyphs {
        let first = u16_at(data, at)?;
        let more = match format {
            0 => 0,
            1 => usize::from(*data.get(at + 2)?),
            2 => u16_at(data, at + 2)?,
            _ => return None,
        };
        at += [2, 3, 4][usize::from(format)];
        for step in 0..=more {
            if entries.len() < glyphs {
                entries.push((first + step) as u16);
            }
        }
    }
    Some(entries)
}

/// Read a font's own encoding: the glyph of each character code.
fn encoding(data: &[u8], start: usize) -> Option<Vec<u16>> {
    let mut glyphs = vec![0; 256];
    let format = *data.get(start)?;
    let count = usize::from(*data.get(start + 1)?);
    let mut at = start + 2;
    match format & 0x7F {
        // The codes of glyphs 1, 2, and so on.
        0 => {
            for glyph in 1..=count {
                glyphs[usize::from(*data.get(at)?)] = glyph as u16;
                at += 1;
            }
        }
        // Ranges of codes for runs of glyphs.
        1 => {
            let mut glyph = 1;
            for _ in 0..count {
                let (first, more) = (usize::from(*data.get(at)?), usize::from(*data.get(at + 1)?));
                at += 2;
                for code in first..=first + more {
                    if let Some(entry) = glyphs.get_mut(code) {
                        *entry = glyph;
                    }
                    glyph += 1;
                }
            }
        }
        _ => return None,
    }
    Some(glyphs)
}

/// Read which font dictionary each glyph of a CID-keyed font uses.
fn font_dictionaries(data: &[u8], start: usize, glyphs: usize) -> Option<Vec<u8>> {
    match *data.get(start)? {
        0 => Some(data.get(start + 1..start + 1 + glyphs)?.to_vec()),
        // Ranges: each starts at a glyph and runs to the next range.
        3 => {
            let ranges = u16_at(data, start + 1)?;
            let mut result = vec![0; glyphs];
            for range in 0..ranges {
                let at = start + 3 + range * 3;
                let (first, which) = (u16_at(data, at)?, *data.get(at + 2)?);
                let next = u16_at(data, at + 3)?;
                for entry in result.get_mut(first..next.min(glyphs))? {
                    *entry = which;
                }
            }
            Some(result)
        }
        _ => None,
    }
}

/// Runs a Type 2 charstring and collects the outline it draws.
struct CharString<'a> {
    font: &'a CffFont,
    local: &'a [(usize, usize)],
    stack: Vec<f64>,
    x: f64,
    y: f64,
    /// How many stem hints were declared, which sizes the hint masks.
    stems: usize,
    /// Whether the glyph's width, an extra first number, has been passed.
    width_seen: bool,
    open: bool,
    operators: usize,
    segments: Vec<Segment>,
}

impl CharString<'_> {
    /// Drop the width when the first operator has one number more than it
    /// takes, or an odd count where it takes pairs.
    fn skip_width(&mut self, has_extra: bool) {
        if !self.width_seen && has_extra && !self.stack.is_empty() {
            self.stack.remove(0);
        }
        self.width_seen = true;
    }

    fn move_to(&mut self, dx: f64, dy: f64) {
        if self.open {
            self.segments.push(Segment::Close);
        }
        self.x += dx;
        self.y += dy;
        self.segments.push(Segment::Move(self.x, self.y));
        self.open = true;
    }

    fn line_to(&mut self, dx: f64, dy: f64) {
        self.x += dx;
        self.y += dy;
        self.segments.push(Segment::Line(self.x, self.y));
    }

    /// A curve given by the steps to its two control points and its end.
    fn curve_to(&mut self, steps: [f64; 6]) {
        let first = (self.x + steps[0], self.y + steps[1]);
        let second = (first.0 + steps[2], first.1 + steps[3]);
        self.x = second.0 + steps[4];
        self.y = second.1 + steps[5];
        let curve = Segment::Cubic(first.0, first.1, second.0, second.1, self.x, self.y);
        self.segments.push(curve);
    }

    /// Run the charstring between two positions. `None` means it cannot be
    /// read; `Some(true)` that the glyph has ended.
    fn run(&mut self, start: usize, end: usize, depth: usize) -> Option<bool> {
        let data = self.font.data.get(start..end)?;
        let mut at = 0;
        while let Some(&byte) = data.get(at) {
            at += 1;
            // Numbers are pushed; anything else is an operator.
            match byte {
                28 => {
                    self.stack.push(f64::from(u16_at(data, at)? as u16 as i16));
                    at += 2;
                }
                32..=246 => self.stack.push(f64::from(byte) - 139.0),
                247..=250 => {
                    let next = f64::from(*data.get(at)?);
                    self.stack
                        .push((f64::from(byte) - 247.0) * 256.0 + next + 108.0);
                    at += 1;
                }
                251..=254 => {
                    let next = f64::from(*data.get(at)?);
                    self.stack
                        .push(-(f64::from(byte) - 251.0) * 256.0 - next - 108.0);
                    at += 1;
                }
                // A number with sixteen bits before and after the point.
                255 => {
                    let bytes = data.get(at..at + 4)?;
                    let value = i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                    self.stack.push(f64::from(value) / 65_536.0);
                    at += 4;
                }
                _ => {
                    self.operators += 1;
                    if self.operators > MAX_OPERATORS {
                        return None;
                    }
                    let escape = if byte == 12 {
                        at += 1;
                        *data.get(at - 1)?
                    } else {
                        0
                    };
                    match byte {
                        // Hint masks are followed by one bit for each stem.
                        19 | 20 => {
                            self.skip_width(self.stack.len() % 2 == 1);
                            self.stems += self.stack.len() / 2;
                            at += self.stems.div_ceil(8);
                        }
                        10 | 29 => {
                            let routines = if byte == 10 {
                                self.local
                            } else {
                                self.font.global_subroutines.as_slice()
                            };
                            let bias = match routines.len() {
                                0..=1239 => 107.0,
                                1240..=33_899 => 1131.0,
                                _ => 32_768.0,
                            };
                            let number = self.stack.pop()? + bias;
                            if depth >= MAX_CALL_DEPTH || number < 0.0 {
                                return None;
                            }
                            let &(from, to) = routines.get(number as usize)?;
                            if self.run(from, to, depth + 1)? {
                                return Some(true);
                            }
                            continue;
                        }
                        11 => return Some(false),
                        14 => {
                            self.skip_width(self.stack.len() % 2 == 1);
                            return Some(true);
                        }
                        _ => self.operator(byte, escape),
                    }
                    self.stack.clear();
                }
            }
            if self.stack.len() > MAX_STACK {
                return None;
            }
        }
        Some(false)
    }

    /// Run one path or hint operator on the numbers before it.
    fn operator(&mut self, operator: u8, escape: u8) {
        let count = self.stack.len();
        match operator {
            // Stem hints, in pairs.
            1 | 3 | 18 | 23 => {
                self.skip_width(count % 2 == 1);
                self.stems += self.stack.len() / 2;
            }
            21 => {
                self.skip_width(count > 2);
                if let [dx, dy] = self.stack[..] {
                    self.move_to(dx, dy);
                }
            }
            22 | 4 => {
                self.skip_width(count > 1);
                if let [step] = self.stack[..] {
                    if operator == 22 {
                        self.move_to(step, 0.0);
                    } else {
                        self.move_to(0.0, step);
                    }
                }
            }
            5 => {
                for pair in self.stack.clone().as_chunks::<2>().0 {
                    self.line_to(pair[0], pair[1]);
                }
            }
            // Lines that turn each time, starting across or up.
            6 | 7 => {
                for (index, &step) in self.stack.clone().iter().enumerate() {
                    if (index % 2 == 0) == (operator == 6) {
                        self.line_to(step, 0.0);
                    } else {
                        self.line_to(0.0, step);
                    }
                }
            }
            8 => {
                for six in self.stack.clone().as_chunks::<6>().0 {
                    self.curve_to([six[0], six[1], six[2], six[3], six[4], six[5]]);
                }
            }
            // Curves and then a line, or lines and then a curve.
            24 if count >= 8 => {
                let numbers = self.stack.clone();
                for six in numbers[..count - 2].as_chunks::<6>().0 {
                    self.curve_to([six[0], six[1], six[2], six[3], six[4], six[5]]);
                }
                self.line_to(numbers[count - 2], numbers[count - 1]);
            }
            25 if count >= 8 => {
                let numbers = self.stack.clone();
                for pair in numbers[..count - 6].as_chunks::<2>().0 {
                    self.line_to(pair[0], pair[1]);
                }
                let six = &numbers[count - 6..];
                self.curve_to([six[0], six[1], six[2], six[3], six[4], six[5]]);
            }
            // Curves that start and end up and down, or across.
            26 | 27 => {
                let numbers = self.stack.clone();
                let (mut side, rest) = match count % 4 {
                    1 => (numbers[0], &numbers[1..]),
                    _ => (0.0, &numbers[..]),
                };
                for four in rest.as_chunks::<4>().0 {
                    if operator == 26 {
                        self.curve_to([side, four[0], four[1], four[2], 0.0, four[3]]);
                    } else {
                        self.curve_to([four[0], side, four[1], four[2], four[3], 0.0]);
                    }
                    side = 0.0;
                }
            }
            // Curves that turn each time, starting up or across.
            30 | 31 => {
                let numbers = self.stack.clone();
                let mut across = operator == 31;
                let groups = count / 4;
                for (group, four) in numbers.as_chunks::<4>().0.iter().enumerate() {
                    // The last curve may end off its axis by one more step.
                    let extra = if group + 1 == groups && count % 4 == 1 {
                        numbers[count - 1]
                    } else {
                        0.0
                    };
                    if across {
                        self.curve_to([four[0], 0.0, four[1], four[2], extra, four[3]]);
                    } else {
                        self.curve_to([0.0, four[0], four[1], four[2], four[3], extra]);
                    }
                    across = !across;
                }
            }
            12 => self.flex(escape),
            _ => {}
        }
    }

    /// The flex operators: two curves that meet at a nearly flat join.
    fn flex(&mut self, escape: u8) {
        let n = self.stack.clone();
        let (start_x, start_y) = (self.x, self.y);
        match (escape, n.len()) {
            (34, 7..) => {
                self.curve_to([n[0], 0.0, n[1], n[2], n[3], 0.0]);
                let back = start_y - self.y;
                self.curve_to([n[4], 0.0, n[5], back, n[6], 0.0]);
            }
            (35, 12..) => {
                self.curve_to([n[0], n[1], n[2], n[3], n[4], n[5]]);
                self.curve_to([n[6], n[7], n[8], n[9], n[10], n[11]]);
            }
            (36, 9..) => {
                self.curve_to([n[0], n[1], n[2], n[3], n[4], 0.0]);
                let back = start_y - (self.y + n[7]);
                self.curve_to([n[5], 0.0, n[6], n[7], n[8], back]);
            }
            (37, 11..) => {
                self.curve_to([n[0], n[1], n[2], n[3], n[4], n[5]]);
                // The last step runs along whichever axis the curves moved
                // further on, and the other axis returns to the start.
                let moved_x: f64 = [n[0], n[2], n[4], n[6], n[8]].iter().sum();
                let moved_y: f64 = [n[1], n[3], n[5], n[7], n[9]].iter().sum();
                let (end_x, end_y) = (self.x + n[6] + n[8], self.y + n[7] + n[9]);
                let last = if moved_x.abs() > moved_y.abs() {
                    (n[10], start_y - end_y)
                } else {
                    (start_x - end_x, n[10])
                };
                self.curve_to([n[6], n[7], n[8], n[9], last.0, last.1]);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::glyph_text;

    #[test]
    fn standard_strings_are_known_glyph_names() {
        assert_eq!(STANDARD_STRINGS[34], "A");
        assert_eq!(STANDARD_STRINGS[66], "a");
        assert_eq!(STANDARD_STRINGS[228], "zcaron");
        for name in &STANDARD_STRINGS[1..] {
            assert!(glyph_text(name.as_bytes()).is_some(), "{name}");
        }
        let mut sorted = STANDARD_STRINGS.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), STANDARD_STRINGS.len());
    }
}
