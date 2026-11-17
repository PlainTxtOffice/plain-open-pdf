//! Type 1 font programs (Adobe Type 1 Font Format, as ISO 32000-2 section
//! 9.9 embeds them): the encoding and font matrix of the clear text part,
//! and the subroutines and charstrings of the eexec-encrypted part.

use super::SimpleEncoding;
use super::truetype::Segment;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::Arc;

/// The key that starts eexec decryption of the private part.
const EEXEC_KEY: u16 = 55665;
/// The key that starts decryption of each charstring and subroutine.
const CHARSTRING_KEY: u16 = 4330;
/// How deeply subroutine calls and accented characters may nest.
const MAX_DEPTH: usize = 10;
/// The most commands one glyph may run, to bound work on hostile fonts.
const MAX_STEPS: usize = 100_000;
/// The most operands the charstring stack holds.
const MAX_STACK: usize = 48;

/// A parsed Type 1 font program.
#[derive(Debug)]
pub struct Type1Font {
    /// Each glyph's name, in the order of the charstrings.
    names: Vec<String>,
    /// Glyph numbers by name.
    by_name: BTreeMap<String, u16>,
    /// Each glyph's decrypted charstring.
    char_strings: Vec<Vec<u8>>,
    /// The decrypted subroutines.
    subroutines: Vec<Vec<u8>>,
    /// The glyph name the font's own encoding gives each code.
    encoding: Vec<Option<String>>,
    units_per_em: f64,
    /// Outlines already read, by glyph number.
    outlines: RefCell<BTreeMap<u16, Arc<Vec<Segment>>>>,
}

/// Undo eexec or charstring encryption, which starts from `key`, and drop
/// the first `skip` bytes, which are random.
fn decrypt(data: &[u8], key: u16, skip: usize) -> Vec<u8> {
    let mut state = key;
    let plain = data.iter().map(|&byte| {
        let plain = byte ^ (state >> 8) as u8;
        state = (u16::from(byte).wrapping_add(state))
            .wrapping_mul(52845)
            .wrapping_add(22719);
        plain
    });
    plain.skip(skip).collect()
}

/// Join the segments of a PFB file, whose parts each start with 0x80, a
/// type byte, and a little-endian length; other data is returned as it is.
fn without_pfb_headers(data: Vec<u8>) -> Vec<u8> {
    if data.first() != Some(&0x80) {
        return data;
    }
    let mut joined = Vec::new();
    let mut at = 0;
    while let (Some(&0x80), Some(&kind)) = (data.get(at), data.get(at + 1)) {
        if kind == 3 {
            break;
        }
        let Some(length) = data.get(at + 2..at + 6) else {
            break;
        };
        let length = u32::from_le_bytes([length[0], length[1], length[2], length[3]]) as usize;
        let start = at + 6;
        let end = start.saturating_add(length).min(data.len());
        joined.extend(&data[start..end]);
        at = end;
    }
    joined
}

/// Where the first `needle` at or after `start` ends. The private part is
/// searched by bytes, not tokens, since binary charstrings lie between its
/// keys.
fn after(data: &[u8], start: usize, needle: &[u8]) -> Option<usize> {
    let found = data
        .get(start..)?
        .windows(needle.len())
        .position(|part| part == needle)?;
    Some(start + found + needle.len())
}

fn whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\r' | b'\n' | b'\x0C' | 0)
}

fn delimiter(byte: u8) -> bool {
    matches!(
        byte,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

/// Reads PostScript tokens from the plain parts of a font program.
struct Tokens<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Tokens<'a> {
    /// The next token: a name with its slash, a delimiter, or a run of
    /// other bytes. Comments are skipped.
    fn next(&mut self) -> Option<&'a [u8]> {
        loop {
            while self.data.get(self.at).is_some_and(|&byte| whitespace(byte)) {
                self.at += 1;
            }
            if self.data.get(self.at) == Some(&b'%') {
                while self
                    .data
                    .get(self.at)
                    .is_some_and(|&b| b != b'\n' && b != b'\r')
                {
                    self.at += 1;
                }
                continue;
            }
            break;
        }
        let start = self.at;
        let first = *self.data.get(start)?;
        self.at += 1;
        if delimiter(first) && first != b'/' {
            return Some(&self.data[start..self.at]);
        }
        while self
            .data
            .get(self.at)
            .is_some_and(|&byte| !whitespace(byte) && !delimiter(byte))
        {
            self.at += 1;
        }
        Some(&self.data[start..self.at])
    }

    fn integer(&mut self) -> Option<i64> {
        std::str::from_utf8(self.next()?).ok()?.parse().ok()
    }

    /// Binary data of a length, after the one space that follows the token
    /// that reads it (`RD` or `-|`).
    fn binary(&mut self, length: usize) -> Option<&'a [u8]> {
        self.next()?;
        let start = self.at + 1;
        let data = self.data.get(start..start.checked_add(length)?)?;
        self.at = start + length;
        Some(data)
    }

    /// Move past the first occurrence of a token; `false` when there is none.
    fn find(&mut self, wanted: &[u8]) -> bool {
        while let Some(token) = self.next() {
            if token == wanted {
                return true;
            }
        }
        false
    }
}

impl Type1Font {
    /// Read a Type 1 font program: an embedded `/FontFile`, or a PFA or PFB
    /// file. Returns `None` for data that is not one.
    pub fn parse(data: Vec<u8>) -> Option<Self> {
        let data = without_pfb_headers(data);
        let eexec = data.windows(5).position(|part| part == b"eexec")?;
        let clear = &data[..eexec];
        let mut start = eexec + 5;
        while data.get(start).is_some_and(|&byte| whitespace(byte)) {
            start += 1;
        }
        let encrypted = &data[start..];
        // The private part is binary, or hexadecimal when its first bytes
        // are hexadecimal digits.
        let hex = encrypted.len() >= 4 && encrypted[..4].iter().all(u8::is_ascii_hexdigit);
        let encrypted = if hex {
            let digits: Vec<u8> = encrypted
                .iter()
                .copied()
                .filter(|byte| !whitespace(*byte))
                .take_while(u8::is_ascii_hexdigit)
                .collect();
            digits
                .as_chunks::<2>()
                .0
                .iter()
                .filter_map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
                .collect()
        } else {
            encrypted.to_vec()
        };
        let private = decrypt(&encrypted, EEXEC_KEY, 4);

        let mut font = Self {
            names: Vec::new(),
            by_name: BTreeMap::new(),
            char_strings: Vec::new(),
            subroutines: Vec::new(),
            encoding: vec![None; 256],
            units_per_em: 1000.0,
            outlines: RefCell::new(BTreeMap::new()),
        };
        font.read_clear_text(clear);
        let skip = match after(&private, 0, b"/lenIV") {
            Some(at) => (Tokens { data: &private, at }).integer().unwrap_or(4),
            None => 4,
        };
        // A negative length means the charstrings are not encrypted.
        let open = |data: &[u8]| match usize::try_from(skip) {
            Ok(skip) => decrypt(data, CHARSTRING_KEY, skip),
            Err(_) => data.to_vec(),
        };

        let mut resume = 0;
        if let Some(at) = after(&private, 0, b"/Subrs") {
            let mut tokens = Tokens { data: &private, at };
            let count = usize::try_from(tokens.integer().unwrap_or(0)).unwrap_or(0);
            font.subroutines = vec![Vec::new(); count.min(65_536)];
            tokens.next();
            loop {
                let mark = tokens.at;
                if tokens.next() != Some(b"dup") {
                    tokens.at = mark;
                    break;
                }
                let index = tokens
                    .integer()
                    .and_then(|index| usize::try_from(index).ok());
                let length = tokens
                    .integer()
                    .and_then(|length| usize::try_from(length).ok());
                let (Some(index), Some(length)) = (index, length) else {
                    break;
                };
                let Some(data) = tokens.binary(length) else {
                    break;
                };
                if let Some(slot) = font.subroutines.get_mut(index) {
                    *slot = open(data);
                }
                // `NP`, `|`, or `noaccess put` ends each entry.
                let mark = tokens.at;
                match tokens.next() {
                    Some(b"noaccess") => {
                        tokens.next();
                    }
                    Some(b"NP" | b"|") => {}
                    _ => tokens.at = mark,
                }
            }
            resume = tokens.at;
        }
        let at = after(&private, resume, b"/CharStrings")?;
        let mut tokens = Tokens { data: &private, at };
        tokens.integer()?;
        while let Some(token) = tokens.next() {
            if token.first() == Some(&b'/') {
                let name = String::from_utf8_lossy(&token[1..]).into_owned();
                let Some(length) = tokens.integer().and_then(|v| usize::try_from(v).ok()) else {
                    break;
                };
                let Some(data) = tokens.binary(length) else {
                    break;
                };
                let Ok(glyph) = u16::try_from(font.char_strings.len()) else {
                    break;
                };
                font.by_name.entry(name.clone()).or_insert(glyph);
                font.names.push(name);
                font.char_strings.push(open(data));
            } else if token == b"end" {
                break;
            }
        }
        (!font.char_strings.is_empty()).then_some(font)
    }

    /// Read the font matrix and the encoding from the clear text part.
    fn read_clear_text(&mut self, clear: &[u8]) {
        let mut tokens = Tokens { data: clear, at: 0 };
        if tokens.find(b"/FontMatrix") && matches!(tokens.next(), Some(b"[" | b"{")) {
            let scale = tokens
                .next()
                .and_then(|token| std::str::from_utf8(token).ok()?.parse::<f64>().ok());
            if let Some(scale) = scale.filter(|scale| scale.abs() > 1e-9) {
                self.units_per_em = 1.0 / scale.abs();
            }
        }
        let mut tokens = Tokens { data: clear, at: 0 };
        if !tokens.find(b"/Encoding") {
            return;
        }
        let mark = tokens.at;
        if tokens.next() == Some(b"StandardEncoding") {
            let standard = SimpleEncoding::standard();
            for code in 0..=255_u8 {
                self.encoding[usize::from(code)] = standard.glyph_name(code).map(str::to_owned);
            }
            return;
        }
        tokens.at = mark;
        // `dup code /name put` for each code, until the array is defined.
        while let Some(token) = tokens.next() {
            match token {
                b"def" | b"readonly" if self.encoding.iter().any(Option::is_some) => break,
                b"dup" => {
                    let code = tokens.integer().and_then(|code| u8::try_from(code).ok());
                    let name = tokens.next().filter(|name| name.first() == Some(&b'/'));
                    if let (Some(code), Some(name)) = (code, name) {
                        let name = String::from_utf8_lossy(&name[1..]).into_owned();
                        self.encoding[usize::from(code)] = Some(name);
                    }
                }
                _ => {}
            }
        }
    }

    /// The number of glyphs in the font.
    pub fn glyph_count(&self) -> usize {
        self.char_strings.len()
    }

    /// The font units that make up the font size, usually 1000.
    pub fn units_per_em(&self) -> f64 {
        self.units_per_em
    }

    /// The name of a glyph.
    pub fn glyph_name(&self, glyph: u16) -> Option<&str> {
        self.names.get(usize::from(glyph)).map(String::as_str)
    }

    /// The glyph with a name.
    pub fn glyph_by_name(&self, name: &str) -> Option<u16> {
        self.by_name.get(name).copied()
    }

    /// The glyph the font's own encoding gives a character code.
    pub fn glyph_by_code(&self, code: u8) -> Option<u16> {
        let name = self.encoding.get(usize::from(code))?.as_deref()?;
        self.glyph_by_name(name)
    }

    /// The glyph name the font's own encoding gives a character code.
    pub fn encoded_name(&self, code: u8) -> Option<&str> {
        self.encoding.get(usize::from(code))?.as_deref()
    }

    /// The outline of a glyph, in font units with the vertical axis up. A
    /// glyph without an outline, such as a space, and a glyph that cannot
    /// be read give no segments.
    pub fn outline(&self, glyph: u16) -> Arc<Vec<Segment>> {
        if let Some(outline) = self.outlines.borrow().get(&glyph) {
            return Arc::clone(outline);
        }
        let mut interpreter = Interpreter::new(self, (0.0, 0.0));
        if let Some(code) = self.char_strings.get(usize::from(glyph)) {
            interpreter.run(code, 0);
        }
        let outline = Arc::new(interpreter.finish());
        self.outlines
            .borrow_mut()
            .insert(glyph, Arc::clone(&outline));
        outline
    }
}

/// Runs charstrings (Type 1 Font Format chapter 6), collecting segments.
struct Interpreter<'a> {
    font: &'a Type1Font,
    /// Where the glyph's origin falls, for the accent of an accented
    /// character.
    origin: (f64, f64),
    stack: Vec<f64>,
    /// Values `callothersubr` leaves for `pop`.
    results: Vec<f64>,
    x: f64,
    y: f64,
    segments: Vec<Segment>,
    open: bool,
    /// The points of a flex, while one is being read.
    flex: Option<Vec<(f64, f64)>>,
    steps: usize,
    done: bool,
}

impl<'a> Interpreter<'a> {
    fn new(font: &'a Type1Font, origin: (f64, f64)) -> Self {
        Self {
            font,
            origin,
            stack: Vec::new(),
            results: Vec::new(),
            x: 0.0,
            y: 0.0,
            segments: Vec::new(),
            open: false,
            flex: None,
            steps: 0,
            done: false,
        }
    }

    fn finish(mut self) -> Vec<Segment> {
        if self.open {
            self.segments.push(Segment::Close);
        }
        self.segments
    }

    fn point(&self) -> (f64, f64) {
        (self.x + self.origin.0, self.y + self.origin.1)
    }

    fn move_by(&mut self, dx: f64, dy: f64) {
        self.x += dx;
        self.y += dy;
        if let Some(points) = &mut self.flex {
            points.push((self.x, self.y));
            return;
        }
        if self.open {
            self.segments.push(Segment::Close);
        }
        let (x, y) = self.point();
        self.segments.push(Segment::Move(x, y));
        self.open = true;
    }

    fn line_by(&mut self, dx: f64, dy: f64) {
        self.x += dx;
        self.y += dy;
        let (x, y) = self.point();
        self.segments.push(Segment::Line(x, y));
    }

    fn curve_by(&mut self, deltas: [f64; 6]) {
        let mut points = [(0.0, 0.0); 3];
        for (point, pair) in points.iter_mut().zip(deltas.as_chunks::<2>().0) {
            self.x += pair[0];
            self.y += pair[1];
            *point = self.point();
        }
        let [(x1, y1), (x2, y2), (x3, y3)] = points;
        self.segments.push(Segment::Cubic(x1, y1, x2, y2, x3, y3));
    }

    /// Draw an accented character's base or accent glyph, given by its
    /// code in the standard encoding, with its origin at `origin`.
    fn compose(&mut self, code: f64, origin: (f64, f64), depth: usize) {
        let standard = SimpleEncoding::standard();
        let Ok(code) = u8::try_from(code as i64) else {
            return;
        };
        let glyph = standard
            .glyph_name(code)
            .and_then(|name| self.font.glyph_by_name(name));
        let Some(code) = glyph.and_then(|glyph| self.font.char_strings.get(usize::from(glyph)))
        else {
            return;
        };
        let mut part = Interpreter::new(self.font, origin);
        part.steps = self.steps;
        part.run(code, depth + 1);
        self.steps = part.steps;
        self.segments.extend(part.finish());
    }

    /// Run a charstring or subroutine. Returns at `return`, at its end, or
    /// when the glyph is done.
    fn run(&mut self, code: &[u8], depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        let mut at = 0;
        while let Some(&byte) = code.get(at) {
            at += 1;
            self.steps += 1;
            if self.done || self.steps > MAX_STEPS {
                self.done = true;
                return;
            }
            let value = match byte {
                32..=246 => Some(f64::from(byte) - 139.0),
                247..=250 => {
                    let next = f64::from(*code.get(at).unwrap_or(&0));
                    at += 1;
                    Some((f64::from(byte) - 247.0) * 256.0 + next + 108.0)
                }
                251..=254 => {
                    let next = f64::from(*code.get(at).unwrap_or(&0));
                    at += 1;
                    Some(-(f64::from(byte) - 251.0) * 256.0 - next - 108.0)
                }
                255 => {
                    let bytes = code.get(at..at + 4).unwrap_or(&[0; 4]);
                    at += 4;
                    Some(f64::from(i32::from_be_bytes([
                        bytes[0], bytes[1], bytes[2], bytes[3],
                    ])))
                }
                _ => None,
            };
            if let Some(value) = value {
                if self.stack.len() < MAX_STACK {
                    self.stack.push(value);
                }
                continue;
            }
            let operator = if byte == 12 {
                let next = *code.get(at).unwrap_or(&0);
                at += 1;
                1200 + u16::from(next)
            } else {
                u16::from(byte)
            };
            let arguments = std::mem::take(&mut self.stack);
            let argument = |index: usize| arguments.get(index).copied().unwrap_or(0.0);
            match operator {
                // hsbw: the left sidebearing point is where drawing starts.
                13 => (self.x, self.y) = (argument(0), 0.0),
                // sbw
                1207 => (self.x, self.y) = (argument(0), argument(1)),
                21 => self.move_by(argument(0), argument(1)),
                22 => self.move_by(argument(0), 0.0),
                4 => self.move_by(0.0, argument(0)),
                5 => self.line_by(argument(0), argument(1)),
                6 => self.line_by(argument(0), 0.0),
                7 => self.line_by(0.0, argument(0)),
                8 => self.curve_by([0, 1, 2, 3, 4, 5].map(argument)),
                30 => {
                    let [a, b, c, d] = [0, 1, 2, 3].map(argument);
                    self.curve_by([0.0, a, b, c, d, 0.0]);
                }
                31 => {
                    let [a, b, c, d] = [0, 1, 2, 3].map(argument);
                    self.curve_by([a, 0.0, b, c, 0.0, d]);
                }
                9 => {
                    if self.open {
                        self.segments.push(Segment::Close);
                        self.open = false;
                    }
                }
                10 => {
                    let subroutine =
                        usize::try_from(argument(arguments.len().saturating_sub(1)) as i64)
                            .ok()
                            .and_then(|index| self.font.subroutines.get(index));
                    // The arguments before the subroutine's number are its own.
                    self.stack = arguments[..arguments.len().saturating_sub(1)].to_vec();
                    if let Some(subroutine) = subroutine {
                        self.run(subroutine, depth + 1);
                    }
                }
                11 => {
                    self.stack = arguments;
                    return;
                }
                14 => {
                    self.done = true;
                    return;
                }
                // seac: an accented character, made of two standard ones.
                1206 => {
                    let [accent_bearing, dx, dy, base, accent] = [0, 1, 2, 3, 4].map(argument);
                    self.compose(base, self.origin, depth);
                    let origin = (self.origin.0 + dx - accent_bearing, self.origin.1 + dy);
                    self.compose(accent, origin, depth);
                    self.open = false;
                    self.done = true;
                    return;
                }
                1212 => {
                    let mut kept = arguments.clone();
                    let divisor = kept.pop().unwrap_or(1.0);
                    let dividend = kept.pop().unwrap_or(0.0);
                    kept.push(if divisor == 0.0 {
                        0.0
                    } else {
                        dividend / divisor
                    });
                    self.stack = kept;
                }
                // callothersubr
                1216 => {
                    let mut kept = arguments.clone();
                    let number = kept.pop().unwrap_or(0.0) as i64;
                    let count = (kept.pop().unwrap_or(0.0).max(0.0) as usize).min(kept.len());
                    let given = kept.split_off(kept.len() - count);
                    self.stack = kept;
                    self.other_subroutine(number, &given);
                }
                // pop
                1217 => {
                    self.stack = arguments;
                    let value = self.results.pop().unwrap_or(0.0);
                    if self.stack.len() < MAX_STACK {
                        self.stack.push(value);
                    }
                }
                // setcurrentpoint
                1233 => (self.x, self.y) = (argument(0), argument(1)),
                // Hints, dot sections, and unknown operators draw nothing.
                _ => {}
            }
        }
    }

    /// The other subroutines the standard ones need: flex (0 to 2) and
    /// hint replacement (3). Any other leaves its arguments for `pop`.
    fn other_subroutine(&mut self, number: i64, given: &[f64]) {
        match number {
            1 => self.flex = Some(Vec::new()),
            2 => {}
            0 => {
                let points = self.flex.take().unwrap_or_default();
                // The first point is a reference; the next six make two
                // curves.
                if points.len() >= 7 {
                    let shift = |(x, y): (f64, f64)| (x + self.origin.0, y + self.origin.1);
                    let [p1, p2, p3, p4, p5, p6] =
                        [1, 2, 3, 4, 5, 6].map(|index| shift(points[index]));
                    self.segments
                        .push(Segment::Cubic(p1.0, p1.1, p2.0, p2.1, p3.0, p3.1));
                    self.segments
                        .push(Segment::Cubic(p4.0, p4.1, p5.0, p5.1, p6.0, p6.1));
                }
                // `pop pop setcurrentpoint` then reads the end point.
                let end_y = given.get(2).copied().unwrap_or(self.y);
                let end_x = given.get(1).copied().unwrap_or(self.x);
                self.results.extend([end_y, end_x]);
            }
            _ => self.results.extend(given.iter().rev()),
        }
    }
}
