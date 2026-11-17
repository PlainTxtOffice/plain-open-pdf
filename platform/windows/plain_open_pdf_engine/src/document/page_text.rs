//! Text extraction: the glyphs a page shows, each with its Unicode text and
//! its position (ISO 32000-2 sections 9.3, 9.4, and 9.10), and the text
//! assembled from them.

use super::DocumentError;
use super::pages::Rectangle;
use super::reader::{Reader, entry};
use crate::font::{CffFont, SimpleEncoding, StandardFont, ToUnicodeMap, TrueTypeFont, Type1Font};
use crate::{Dictionary, ObjectReference, Operation, PdfObject, parse_content};
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

/// How many ancestors are searched for inherited page resources.
const MAX_ANCESTORS: usize = 32;
/// How deeply form XObjects may nest before their text is left out.
const MAX_FORM_DEPTH: usize = 8;
/// Most glyphs read from one page, to bound memory on hostile input.
const MAX_GLYPHS: usize = 2_000_000;
/// Glyph width, in thousandths of the font size, when a font gives none.
const DEFAULT_WIDTH: f64 = 500.0;
/// A gap along a line wider than this many font sizes parts what comes
/// before it from what comes after, as columns are parted.
const WIDE_GAP: f64 = 0.8;

/// A transformation matrix `[a b c d e f]`, applied to row vectors.
pub(super) type Matrix = [f64; 6];

pub(super) const IDENTITY: Matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// The matrix that applies `first` and then `second`.
pub(super) fn multiply(first: &Matrix, second: &Matrix) -> Matrix {
    [
        first[0] * second[0] + first[1] * second[2],
        first[0] * second[1] + first[1] * second[3],
        first[2] * second[0] + first[3] * second[2],
        first[2] * second[1] + first[3] * second[3],
        first[4] * second[0] + first[5] * second[2] + second[4],
        first[4] * second[1] + first[5] * second[3] + second[5],
    ]
}

/// One glyph a page shows.
///
/// Positions are in the page's default user space: points, with the origin
/// at the bottom left of the media box and before the page's rotation.
#[derive(Debug, Clone, PartialEq)]
pub struct TextGlyph {
    /// The text the glyph stands for; U+FFFD when it is not known.
    pub text: String,
    /// Horizontal position of the glyph's origin, on the baseline.
    pub x: f64,
    /// Vertical position of the glyph's origin.
    pub y: f64,
    /// Horizontal position of the end of the glyph's own width, before
    /// any character or word spacing.
    pub end_x: f64,
    /// Vertical position of the end of the glyph's own width.
    pub end_y: f64,
    /// The font size in points, as it appears on the page.
    pub size: f64,
    /// The name of the glyph's font, without a subset prefix; empty when
    /// the page names no usable font.
    pub font: Arc<str>,
    /// The fill color the glyph is shown in, as red, green, and blue.
    pub color: [u8; 3],
}

/// A word: glyphs that follow one another without a gap or white space.
#[derive(Debug, Clone, PartialEq)]
pub struct TextWord {
    /// The word's text.
    pub text: String,
    /// The box around the word's glyphs, in the coordinates of glyphs.
    pub bounds: Rectangle,
    /// The number of the block of lines the word is in, from 0.
    pub block: usize,
    /// The number of the word's line within its block, from 0.
    pub line: usize,
    /// The number of the word within its line, from 0.
    pub word: usize,
}

/// A run of text on one line in one font, size, and color.
#[derive(Debug, Clone, PartialEq)]
pub struct TextSpan {
    /// The run's text, with a space where words are apart.
    pub text: String,
    /// The box around the run's glyphs, in the coordinates of glyphs.
    pub bounds: Rectangle,
    /// Horizontal position of the first glyph's origin, on the baseline.
    pub x: f64,
    /// Vertical position of the first glyph's origin.
    pub y: f64,
    /// The name of the font, without a subset prefix.
    pub font: Arc<str>,
    /// The font size in points, as it appears on the page.
    pub size: f64,
    /// The fill color, as red, green, and blue.
    pub color: [u8; 3],
}

/// How the character codes shown with a font become text.
enum Decoder {
    /// A composite font: through its ToUnicode map, if it has one.
    Composite {
        map: Option<ToUnicodeMap>,
        /// Whether every code has two bytes, as with the identity CMaps.
        two_byte: bool,
    },
    /// A simple font, whose codes are single bytes: through its ToUnicode
    /// map and, for codes the map lacks, through its encoding.
    Simple {
        map: Option<ToUnicodeMap>,
        encoding: SimpleEncoding,
    },
}

/// Glyph widths in text space units for a font size of 1.
enum Widths {
    /// By character code, from `/FirstChar` on.
    Simple {
        first: usize,
        widths: Vec<f64>,
        missing: f64,
    },
    /// By CID, which is taken to equal the character code.
    Composite {
        widths: BTreeMap<u32, f64>,
        default: f64,
    },
}

/// A Type 3 font's glyphs, each a content stream (ISO 32000-2 section
/// 9.6.4).
#[derive(Debug)]
pub(super) struct Type3Font {
    /// From glyph space to text space.
    pub matrix: Matrix,
    /// The stream of each glyph, by glyph name.
    pub procedures: Dictionary,
    /// The resources the streams name things in; without them, those of
    /// the page or form that shows the text.
    pub resources: Option<Dictionary>,
}

pub(super) struct Font {
    /// The font's name, without a subset prefix.
    name: Arc<str>,
    decoder: Decoder,
    widths: Widths,
    /// The embedded TrueType program, when it was asked for and is there.
    pub(super) program: Option<Rc<TrueTypeFont>>,
    /// The embedded CFF program, when it was asked for and is there.
    pub(super) cff: Option<Rc<CffFont>>,
    /// The embedded Type 1 program, when it was asked for and is there.
    pub(super) type1: Option<Rc<Type1Font>>,
    /// A Type 3 font's glyph procedures.
    pub(super) type3: Option<Rc<Type3Font>>,
    /// Whether the font dictionary gives the font an encoding.
    has_encoding: bool,
    /// For a composite font, the glyph index of each CID, two bytes each;
    /// without it, a CID is its own glyph index.
    cid_to_glyph: Option<Vec<u8>>,
}

impl Font {
    /// The font used for text shown without a usable font.
    fn fallback() -> Self {
        Self {
            name: Arc::from(""),
            decoder: Decoder::Simple {
                map: None,
                encoding: SimpleEncoding::standard(),
            },
            widths: Widths::Simple {
                first: 0,
                widths: Vec::new(),
                missing: DEFAULT_WIDTH / 1000.0,
            },
            program: None,
            cff: None,
            type1: None,
            type3: None,
            has_encoding: false,
            cid_to_glyph: None,
        }
    }

    /// The stream of the Type 3 glyph a character code selects, by the
    /// glyph name its encoding gives it.
    pub(super) fn type3_procedure(&self, code: &[u8]) -> Option<ObjectReference> {
        let font = self.type3.as_ref()?;
        let Decoder::Simple { encoding, .. } = &self.decoder else {
            return None;
        };
        let name = encoding.glyph_name(*code.first()?)?;
        match font.procedures.get(name.as_bytes())? {
            PdfObject::Reference(reference) => Some(*reference),
            _ => None,
        }
    }

    /// The glyph of the embedded Type 1 program that a character code
    /// selects: by the glyph name the font's encoding gives it, or by the
    /// program's own encoding, whichever the font dictionary puts first.
    pub(super) fn type1_glyph(&self, code: &[u8]) -> Option<u16> {
        let program = self.type1.as_ref()?;
        let Decoder::Simple { encoding, .. } = &self.decoder else {
            return None;
        };
        let code = *code.first()?;
        let named = || program.glyph_by_name(encoding.glyph_name(code)?);
        let own = || program.glyph_by_code(code);
        if self.has_encoding {
            named().or_else(own)
        } else {
            own().or_else(named)
        }
    }

    /// The glyph of the embedded CFF program that a character code
    /// selects: by CID in a composite font, and otherwise by glyph name or
    /// by the program's own encoding, whichever the font dictionary puts
    /// first (ISO 32000-2 section 9.6.5.2).
    pub(super) fn cff_glyph(&self, code: &[u8]) -> Option<u16> {
        let program = self.cff.as_ref()?;
        match &self.decoder {
            Decoder::Composite { .. } => {
                let value = code
                    .iter()
                    .fold(0_u32, |value, &byte| value << 8 | u32::from(byte));
                program.glyph_by_cid(u16::try_from(value).ok()?)
            }
            Decoder::Simple { encoding, .. } => {
                let code = *code.first()?;
                let named = || program.glyph_by_name(encoding.glyph_name(code)?);
                let own = || program.glyph_by_code(code);
                if self.has_encoding {
                    named().or_else(own)
                } else {
                    own().or_else(named)
                }
            }
        }
    }

    /// The glyph of the embedded font program that a character code
    /// selects (ISO 32000-2 sections 9.6.5.4 and 9.7.4.2).
    pub(super) fn glyph(&self, code: &[u8]) -> Option<u16> {
        let program = self.program.as_ref()?;
        let value = code
            .iter()
            .fold(0_u32, |value, &byte| value << 8 | u32::from(byte));
        match &self.decoder {
            Decoder::Composite { .. } => match &self.cid_to_glyph {
                None => u16::try_from(value).ok(),
                Some(map) => {
                    let index = usize::try_from(value).ok()?.checked_mul(2)?;
                    Some(u16::from_be_bytes([*map.get(index)?, *map.get(index + 1)?]))
                }
            },
            Decoder::Simple { encoding, .. } => {
                // By the character's Unicode value, by the code itself in a
                // symbol font's map, or by the code in the Macintosh map.
                let unicode = || {
                    let character = encoding.text(*code.first()?)?.chars().next()?;
                    program.glyph_index(3, 1, u32::from(character))
                };
                let symbol = || {
                    [0, 0xF000, 0xF100, 0xF200]
                        .iter()
                        .find_map(|base| program.glyph_index(3, 0, base + value))
                };
                let macintosh = || program.glyph_index(1, 0, value);
                if self.has_encoding {
                    unicode().or_else(macintosh).or_else(symbol)
                } else {
                    symbol().or_else(macintosh).or_else(unicode)
                }
            }
        }
    }

    /// How many bytes the character code at the start of `rest` has.
    fn code_length(&self, rest: &[u8]) -> usize {
        let length = match &self.decoder {
            Decoder::Simple { .. } => 1,
            Decoder::Composite { two_byte: true, .. } => 2,
            Decoder::Composite { map: Some(map), .. } => map.code_length(rest),
            Decoder::Composite { map: None, .. } => 2,
        };
        length.clamp(1, rest.len().max(1))
    }

    pub(super) fn text(&self, code: &[u8]) -> String {
        let known = match &self.decoder {
            Decoder::Composite { map, .. } => map.as_ref().and_then(|map| map.lookup(code)),
            Decoder::Simple { map, encoding } => map
                .as_ref()
                .and_then(|map| map.lookup(code))
                .or_else(|| Some(encoding.text(*code.first()?)?.to_owned())),
        };
        known.unwrap_or_else(|| char::REPLACEMENT_CHARACTER.to_string())
    }

    pub(super) fn width(&self, code: &[u8]) -> f64 {
        let value = code
            .iter()
            .fold(0_u32, |value, &byte| value << 8 | u32::from(byte));
        match &self.widths {
            Widths::Simple {
                first,
                widths,
                missing,
            } => (value as usize)
                .checked_sub(*first)
                .and_then(|index| widths.get(index))
                .copied()
                .unwrap_or(*missing),
            Widths::Composite { widths, default } => {
                widths.get(&value).copied().unwrap_or(*default)
            }
        }
    }
}

/// The text part of the graphics state; `q` saves it.
#[derive(Clone)]
pub(super) struct TextState {
    pub font: Option<Rc<Font>>,
    pub size: f64,
    pub character_spacing: f64,
    pub word_spacing: f64,
    /// Horizontal scaling as a factor, 1 for the normal 100.
    pub scale: f64,
    pub leading: f64,
    pub rise: f64,
    /// The text rendering mode, 0 to 7.
    pub mode: i64,
}

impl TextState {
    pub(super) fn new() -> Self {
        Self {
            font: None,
            size: 0.0,
            character_spacing: 0.0,
            word_spacing: 0.0,
            scale: 1.0,
            leading: 0.0,
            rise: 0.0,
            mode: 0,
        }
    }
}

/// The matrices of a text object; `BT` resets them.
pub(super) struct TextObject {
    matrix: Matrix,
    line: Matrix,
}

impl TextObject {
    pub(super) fn new() -> Self {
        Self {
            matrix: IDENTITY,
            line: IDENTITY,
        }
    }

    /// Start a new line, offset from the start of the current one.
    fn next_line(&mut self, x: f64, y: f64) {
        self.line = multiply(&[1.0, 0.0, 0.0, 1.0, x, y], &self.line);
        self.matrix = self.line;
    }
}

/// The fonts of one resource dictionary, each loaded when first selected.
pub(super) struct Fonts<'a> {
    named: Dictionary,
    loaded: BTreeMap<Vec<u8>, Option<Rc<Font>>>,
    /// Whether to read embedded font programs, which drawing needs.
    programs: bool,
    /// The font used for text shown without a usable font.
    fallback: Rc<Font>,
    /// The font that draws text whose own font cannot be drawn; it lends
    /// its glyph widths to a font that gives none.
    substitute: Option<&'a TrueTypeFont>,
}

impl<'a> Fonts<'a> {
    pub(super) fn new(
        reader: &mut Reader,
        resources: &Dictionary,
        programs: bool,
        substitute: Option<&'a TrueTypeFont>,
    ) -> Result<Self, DocumentError> {
        let named = match reader.resolve_entry(resources, "Font")? {
            PdfObject::Dictionary(fonts) => fonts,
            _ => Dictionary::new(),
        };
        Ok(Self {
            named,
            loaded: BTreeMap::new(),
            programs,
            fallback: Rc::new(Font::fallback()),
            substitute,
        })
    }

    fn select(&mut self, reader: &mut Reader, name: &[u8]) -> Option<Rc<Font>> {
        if !self.loaded.contains_key(name) {
            let font =
                load_font(reader, &self.named, name, self.programs, self.substitute).map(Rc::new);
            self.loaded.insert(name.to_vec(), font);
        }
        self.loaded.get(name)?.clone()
    }
}

/// One glyph as a text-showing operator shows it.
pub(super) struct ShownGlyph<'a> {
    pub font: &'a Font,
    /// The glyph's character code.
    pub code: &'a [u8],
    /// The matrix from glyph space, scaled so that one unit is the font
    /// size, to user space, with the glyph's origin at the origin.
    pub start: Matrix,
}

/// Apply a text operator to the text state and report each glyph it shows.
/// Returns whether the operator was a text operator.
pub(super) fn text_operation(
    reader: &mut Reader,
    fonts: &mut Fonts<'_>,
    state: &mut TextState,
    object: &mut TextObject,
    operation: &Operation,
    emit: &mut dyn FnMut(ShownGlyph<'_>),
) -> bool {
    let operands = &operation.operands;
    let number = |index: usize| operands.get(index).and_then(PdfObject::as_number);
    let value = |index: usize| number(index).unwrap_or(0.0);
    match operation.operator.as_slice() {
        b"BT" => *object = TextObject::new(),
        b"ET" => {}
        b"Tf" => {
            state.size = value(1);
            let name = operands.first().and_then(PdfObject::as_name);
            state.font = name.and_then(|name| fonts.select(reader, name));
        }
        b"Tc" => state.character_spacing = value(0),
        b"Tw" => state.word_spacing = value(0),
        b"Tz" => state.scale = number(0).unwrap_or(100.0) / 100.0,
        b"TL" => state.leading = value(0),
        b"Ts" => state.rise = value(0),
        b"Tr" => state.mode = value(0) as i64,
        b"Td" => object.next_line(value(0), value(1)),
        b"TD" => {
            state.leading = -value(1);
            object.next_line(value(0), value(1));
        }
        b"Tm" => {
            let mut matrix = IDENTITY;
            for (index, entry) in matrix.iter_mut().enumerate() {
                match number(index) {
                    Some(number) => *entry = number,
                    None => return true,
                }
            }
            object.line = matrix;
            object.matrix = matrix;
        }
        b"T*" => object.next_line(0.0, -state.leading),
        // Tj, TJ, the quote, and the double quote, which is byte 0x22.
        b"Tj" | b"TJ" | b"'" | [0x22] => {
            if operation.operator.len() == 1 {
                if operation.operator == [0x22] {
                    state.word_spacing = value(0);
                    state.character_spacing = value(1);
                }
                object.next_line(0.0, -state.leading);
            }
            let font = state
                .font
                .clone()
                .unwrap_or_else(|| Rc::clone(&fonts.fallback));
            // The string is the last operand; `TJ` holds strings in an
            // array between position adjustments.
            match operands.last() {
                Some(PdfObject::String(shown)) => show(state, object, &font, shown, emit),
                Some(PdfObject::Array(parts)) => {
                    for part in parts {
                        if let PdfObject::String(shown) = part {
                            show(state, object, &font, shown, emit);
                        } else if let Some(adjustment) = part.as_number() {
                            let x = -adjustment / 1000.0 * state.size * state.scale;
                            object.matrix = multiply(&[1.0, 0.0, 0.0, 1.0, x, 0.0], &object.matrix);
                        }
                    }
                }
                _ => {}
            }
        }
        _ => return false,
    }
    true
}

/// Report the glyphs of a shown string and move the text matrix past each.
fn show(
    state: &TextState,
    object: &mut TextObject,
    font: &Font,
    shown: &[u8],
    emit: &mut dyn FnMut(ShownGlyph<'_>),
) {
    // A unit of glyph space at the text matrix's origin, raised by the
    // text rise.
    let place = |matrix: &Matrix| {
        let scaled = [
            state.size * state.scale,
            0.0,
            0.0,
            state.size,
            0.0,
            state.rise,
        ];
        multiply(&scaled, matrix)
    };
    let mut rest = shown;
    while !rest.is_empty() {
        let (code, tail) = rest.split_at(font.code_length(rest));
        rest = tail;
        let start = place(&object.matrix);
        let mut advance = font.width(code) * state.size + state.character_spacing;
        if code == b" " {
            advance += state.word_spacing;
        }
        let moved = [1.0, 0.0, 0.0, 1.0, advance * state.scale, 0.0];
        object.matrix = multiply(&moved, &object.matrix);
        emit(ShownGlyph { font, code, start });
    }
}

/// The graphics state that places text; `q` saves it.
#[derive(Clone)]
struct State {
    ctm: Matrix,
    text: TextState,
    /// The rectangle that clipping rectangles leave, as left, bottom,
    /// right, and top; clipping by other shapes is not followed.
    clip: Option<[f64; 4]>,
    /// The fill color, as red, green, and blue.
    fill: [u8; 3],
}

/// The shape of the path being built, as far as clipping needs it.
#[derive(Clone, Copy)]
enum PathShape {
    Empty,
    /// One rectangle, as its bounds on the page.
    Rectangle([f64; 4]),
    Other,
}

struct Interpreter<'a> {
    reader: &'a mut Reader,
    glyphs: Vec<TextGlyph>,
}

/// Read the glyphs of a page from its decoded content stream.
pub(super) fn glyphs(
    reader: &mut Reader,
    page: ObjectReference,
    content: &[u8],
) -> Result<Vec<TextGlyph>, DocumentError> {
    let resources = page_resources(reader, page)?;
    let mut interpreter = Interpreter {
        reader,
        glyphs: Vec::new(),
    };
    let state = State {
        ctm: IDENTITY,
        text: TextState::new(),
        clip: None,
        fill: [0; 3],
    };
    interpreter.run(&resources, content, state, 0)?;
    Ok(interpreter.glyphs)
}

impl Interpreter<'_> {
    /// Interpret one content stream with the resources it names things in.
    fn run(
        &mut self,
        resources: &Dictionary,
        content: &[u8],
        mut state: State,
        depth: usize,
    ) -> Result<(), DocumentError> {
        let mut fonts = Fonts::new(self.reader, resources, false, None)?;
        let mut saved: Vec<State> = Vec::new();
        let mut object = TextObject::new();
        let mut path = PathShape::Empty;
        let mut clipping = false;

        let operations = parse_content(content).map_err(DocumentError::Syntax)?;
        for operation in &operations {
            let (ctm, clip, fill) = (state.ctm, state.clip, state.fill);
            let glyphs = &mut self.glyphs;
            let mut emit = |shown: ShownGlyph<'_>| {
                let start = multiply(&shown.start, &ctm);
                // The glyph ends after its own width; spacing set by `Tc`
                // and `Tw` is a gap after it, as other readers measure.
                let width = shown.font.width(shown.code);
                let own = multiply(&[1.0, 0.0, 0.0, 1.0, width, 0.0], &shown.start);
                let end = multiply(&own, &ctm);
                let glyph = TextGlyph {
                    text: shown.font.text(shown.code),
                    x: start[4],
                    y: start[5],
                    end_x: end[4],
                    end_y: end[5],
                    size: start[2].hypot(start[3]),
                    font: Arc::clone(&shown.font.name),
                    color: fill,
                };
                let finite = [glyph.x, glyph.y, glyph.end_x, glyph.end_y, glyph.size]
                    .iter()
                    .all(|value| value.is_finite());
                // A glyph whose origin a clipping rectangle cuts off is
                // not shown; the edge itself counts as inside.
                let shown = clip.is_none_or(|[left, bottom, right, top]| {
                    let slack = 0.01;
                    (left - slack..=right + slack).contains(&glyph.x)
                        && (bottom - slack..=top + slack).contains(&glyph.y)
                });
                if finite && shown && glyphs.len() < MAX_GLYPHS {
                    glyphs.push(glyph);
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
                continue;
            }
            let operands = &operation.operands;
            match operation.operator.as_slice() {
                b"q" => saved.push(state.clone()),
                b"Q" => {
                    if let Some(previous) = saved.pop() {
                        state = previous;
                    }
                }
                b"cm" => {
                    let numbers: Vec<f64> =
                        operands.iter().filter_map(PdfObject::as_number).collect();
                    if let Ok(matrix) = <Matrix>::try_from(numbers) {
                        state.ctm = multiply(&matrix, &state.ctm);
                    }
                }
                // The fill color: gray, RGB, or CMYK by the number of
                // components. Colors of other kinds leave it as it was.
                b"g" | b"rg" | b"k" | b"sc" | b"scn" => {
                    let numbers: Vec<f64> =
                        operands.iter().filter_map(PdfObject::as_number).collect();
                    let level = |value: f64| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
                    let color = match numbers[..] {
                        [gray] => Some([gray; 3]),
                        [red, green, blue] => Some([red, green, blue]),
                        [cyan, magenta, yellow, black] => {
                            Some([cyan, magenta, yellow].map(|ink| (1.0 - ink) * (1.0 - black)))
                        }
                        _ => None,
                    };
                    if let Some(color) = color
                        && numbers.len() == operands.len()
                    {
                        state.fill = color.map(level);
                    }
                }
                b"cs" => state.fill = [0; 3],
                b"re" => {
                    let numbers: Vec<f64> =
                        operands.iter().filter_map(PdfObject::as_number).collect();
                    path = match (path, <[f64; 4]>::try_from(numbers)) {
                        (PathShape::Empty, Ok([x, y, width, height])) => {
                            // The bounds of the rectangle's corners on the page.
                            let corners = [
                                (x, y),
                                (x + width, y),
                                (x, y + height),
                                (x + width, y + height),
                            ]
                            .map(|(x, y)| multiply(&[1.0, 0.0, 0.0, 1.0, x, y], &state.ctm));
                            let across = corners.map(|corner| corner[4]);
                            let up = corners.map(|corner| corner[5]);
                            PathShape::Rectangle([
                                across.iter().copied().fold(f64::INFINITY, f64::min),
                                up.iter().copied().fold(f64::INFINITY, f64::min),
                                across.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                                up.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                            ])
                        }
                        _ => PathShape::Other,
                    };
                }
                b"m" | b"l" | b"c" | b"v" | b"y" | b"h" => path = PathShape::Other,
                b"W" | b"W*" => clipping = true,
                b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"n" => {
                    if clipping && let PathShape::Rectangle(added) = path {
                        state.clip = Some(match state.clip {
                            None => added,
                            Some(clip) => [
                                clip[0].max(added[0]),
                                clip[1].max(added[1]),
                                clip[2].min(added[2]),
                                clip[3].min(added[3]),
                            ],
                        });
                    }
                    clipping = false;
                    path = PathShape::Empty;
                }
                b"Do" if depth < MAX_FORM_DEPTH => {
                    if let Some(name) = operands.first().and_then(PdfObject::as_name) {
                        // A form that cannot be read leaves its text out.
                        let _ = self.form(resources, name, &state, depth);
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Interpret the form XObject a resource name selects, if it is one.
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
        if entry(&form, "Subtype").and_then(PdfObject::as_name) != Some(b"Form") {
            return Ok(());
        }
        let Some(content) = self.reader.stream(reference.number)? else {
            return Ok(());
        };
        let mut matrix = IDENTITY;
        if let PdfObject::Array(values) = self.reader.resolve_entry(&form, "Matrix")?
            && values.len() == 6
        {
            for (value, entry) in matrix.iter_mut().zip(&values) {
                *value = entry.as_number().unwrap_or(*value);
            }
        }
        let mut inner = state.clone();
        inner.ctm = multiply(&matrix, &state.ctm);
        // A form without resources of its own uses those of its parent.
        match self.reader.resolve_entry(&form, "Resources")? {
            PdfObject::Dictionary(own) => self.run(&own, &content, inner, depth + 1),
            _ => self.run(resources, &content, inner, depth + 1),
        }
    }
}

/// Assemble text from glyphs in the order they were shown.
///
/// A line ends where the next glyph leaves the baseline of the one before
/// it or starts well to its left, and a space is added where a gap on the
/// baseline is wide enough to separate words.
pub(super) fn assemble(glyphs: &[TextGlyph]) -> String {
    assemble_mapped(glyphs).0
}

/// Assemble text as [`assemble`] does, and tell for each of its characters
/// the index of the glyph it came from; added spaces and line ends have
/// none.
fn assemble_mapped(glyphs: &[TextGlyph]) -> (String, Vec<Option<usize>>) {
    let mut text = String::new();
    let mut sources = Vec::new();
    let mut previous: Option<&TextGlyph> = None;
    for (index, glyph) in glyphs.iter().enumerate() {
        if let Some(previous) = previous {
            match boundary(previous, glyph) {
                // A gap much wider than a space, as between columns or
                // before a page number, ends the line too.
                Boundary::Line { .. } | Boundary::Space { wide: true } => {
                    if !text.ends_with('\n') {
                        text.push('\n');
                        sources.push(None);
                    }
                }
                Boundary::Space { wide: false }
                    if !text.ends_with([' ', '\n']) && !glyph.text.starts_with(' ') =>
                {
                    text.push(' ');
                    sources.push(None);
                }
                _ => {}
            }
        }
        text.push_str(&glyph.text);
        sources.extend(glyph.text.chars().map(|_| Some(index)));
        previous = Some(glyph);
    }
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
        sources.push(None);
    }
    (text, sources)
}

/// How a glyph follows the one shown before it.
enum Boundary {
    /// Straight after it, in the same word.
    Same,
    /// After a gap on the same line; `wide` when the gap is much more than
    /// a space.
    Space { wide: bool },
    /// On another line, which may start another block of lines.
    Line { new_block: bool },
}

fn boundary(previous: &TextGlyph, glyph: &TextGlyph) -> Boundary {
    let (run_x, run_y) = (previous.end_x - previous.x, previous.end_y - previous.y);
    let length = run_x.hypot(run_y);
    // A glyph without an advance is taken to run to the right.
    let (unit_x, unit_y) = if length > 1e-6 {
        (run_x / length, run_y / length)
    } else {
        (1.0, 0.0)
    };
    let (gap_x, gap_y) = (glyph.x - previous.end_x, glyph.y - previous.end_y);
    let along = gap_x * unit_x + gap_y * unit_y;
    let across = gap_y * unit_x - gap_x * unit_y;
    let size = previous.size.max(glyph.size);
    if across.abs() > 0.5 * size || along < -size {
        // The next line of a block lies just below; anything else, above
        // or further down, starts a new block.
        Boundary::Line {
            new_block: across > 0.5 * size || across < -1.6 * size,
        }
    } else if along > 0.2 * size {
        Boundary::Space {
            wide: along > WIDE_GAP * size,
        }
    } else {
        Boundary::Same
    }
}

fn union(first: Rectangle, second: Rectangle) -> Rectangle {
    Rectangle {
        left: first.left.min(second.left),
        bottom: first.bottom.min(second.bottom),
        right: first.right.max(second.right),
        top: first.top.max(second.top),
    }
}

/// Group glyphs into words, numbered by block, line, and place in the line.
pub(super) fn words(glyphs: &[TextGlyph]) -> Vec<TextWord> {
    let mut words: Vec<TextWord> = Vec::new();
    let (mut block, mut line, mut word) = (0, 0, 0);
    // Whether the next glyph may join the last word.
    let mut open = false;
    let mut previous: Option<&TextGlyph> = None;
    for glyph in glyphs {
        if let Some(previous) = previous {
            match boundary(previous, glyph) {
                Boundary::Same => {}
                Boundary::Space { .. } => open = false,
                Boundary::Line { new_block } => {
                    open = false;
                    word = 0;
                    if new_block {
                        block += 1;
                        line = 0;
                    } else {
                        line += 1;
                    }
                }
            }
        }
        previous = Some(glyph);
        // Only spaces part words; other blank characters, such as a figure
        // space, belong to the word they are in, as PyMuPDF has them.
        if glyph.text.is_empty() || glyph.text.chars().all(|character| character == ' ') {
            open = false;
            continue;
        }
        let area = glyph_box(glyph);
        match words.last_mut().filter(|_| open) {
            Some(last) => {
                last.text.push_str(&glyph.text);
                last.bounds = union(last.bounds, area);
            }
            None => {
                words.push(TextWord {
                    text: glyph.text.clone(),
                    bounds: area,
                    block,
                    line,
                    word,
                });
                word += 1;
                open = true;
            }
        }
    }
    words
}

/// Group glyphs into runs that share a line, a font, a size, and a color.
/// A gap much wider than a space ends a run too.
pub(super) fn spans(glyphs: &[TextGlyph]) -> Vec<TextSpan> {
    let mut spans: Vec<TextSpan> = Vec::new();
    let mut previous: Option<&TextGlyph> = None;
    for glyph in glyphs {
        let mut joins = false;
        let mut apart = false;
        if let Some(previous) = previous {
            let same_style = previous.font == glyph.font
                && (previous.size - glyph.size).abs() < 0.01
                && previous.color == glyph.color;
            match boundary(previous, glyph) {
                Boundary::Same => joins = same_style,
                Boundary::Space { wide } => {
                    joins = same_style && !wide;
                    apart = true;
                }
                Boundary::Line { .. } => {}
            }
        }
        previous = Some(glyph);
        let area = glyph_box(glyph);
        match spans.last_mut().filter(|_| joins) {
            Some(span) => {
                if apart && !span.text.ends_with(' ') && !glyph.text.starts_with(' ') {
                    span.text.push(' ');
                }
                span.text.push_str(&glyph.text);
                span.bounds = union(span.bounds, area);
            }
            None => spans.push(TextSpan {
                text: glyph.text.clone(),
                bounds: area,
                x: glyph.x,
                y: glyph.y,
                font: Arc::clone(&glyph.font),
                size: glyph.size,
                color: glyph.color,
            }),
        }
    }
    spans.retain(|span| !span.text.trim().is_empty());
    spans
}

/// The most places reported for one search.
const MAX_HITS: usize = 10_000;

/// The box around a glyph: from a quarter of its size below the baseline
/// to 0.85 of its size above, which fits most fonts, since font programs
/// are not read for their real ascent and descent.
fn glyph_box(glyph: &TextGlyph) -> Rectangle {
    let (run_x, run_y) = (glyph.end_x - glyph.x, glyph.end_y - glyph.y);
    let length = run_x.hypot(run_y);
    // The direction that is up for the text, at right angles to its run.
    let (up_x, up_y) = if length > 1e-6 {
        (-run_y / length, run_x / length)
    } else {
        (0.0, 1.0)
    };
    let mut area = Rectangle {
        left: f64::INFINITY,
        bottom: f64::INFINITY,
        right: f64::NEG_INFINITY,
        top: f64::NEG_INFINITY,
    };
    for (x, y) in [(glyph.x, glyph.y), (glyph.end_x, glyph.end_y)] {
        for reach in [-0.25 * glyph.size, 0.85 * glyph.size] {
            let (x, y) = (x + up_x * reach, y + up_y * reach);
            area.left = area.left.min(x);
            area.right = area.right.max(x);
            area.bottom = area.bottom.min(y);
            area.top = area.top.max(y);
        }
    }
    area
}

/// Find text among a page's glyphs, ignoring case and treating a line end
/// as a space and a run of spaces as one. Each place found is given as one
/// rectangle for each line it touches.
pub(super) fn search(glyphs: &[TextGlyph], needle: &str) -> Vec<Vec<Rectangle>> {
    // Lower the case of each character, keep one space for each run of
    // white space, and remember where each kept character was.
    let squeeze = |characters: &mut dyn Iterator<Item = char>| {
        let mut kept: Vec<(char, usize)> = Vec::new();
        for (position, character) in characters.enumerate() {
            if !character.is_whitespace() {
                let lowered = character.to_lowercase().next().unwrap_or(character);
                kept.push((lowered, position));
            } else if kept.last().is_none_or(|last| last.0 != ' ') {
                kept.push((' ', position));
            }
        }
        kept
    };
    let needle: Vec<char> = squeeze(&mut needle.chars())
        .into_iter()
        .map(|kept| kept.0)
        .collect();
    if needle.iter().all(|character| character.is_whitespace()) {
        return Vec::new();
    }
    let (text, sources) = assemble_mapped(glyphs);
    let original: Vec<char> = text.chars().collect();
    let folded = squeeze(&mut original.iter().copied());
    let mut hits = Vec::new();
    let mut start = 0;
    while start + needle.len() <= folded.len() && hits.len() < MAX_HITS {
        let end = start + needle.len();
        let found = folded[start..end].iter().map(|kept| kept.0);
        if !found.eq(needle.iter().copied()) {
            start += 1;
            continue;
        }
        let mut lines = Vec::new();
        let mut line: Option<Rectangle> = None;
        for position in folded[start].1..=folded[end - 1].1 {
            if original[position] == '\n' {
                lines.extend(line.take());
            }
            let Some(glyph) = sources[position].and_then(|index| glyphs.get(index)) else {
                continue;
            };
            let area = glyph_box(glyph);
            line = Some(match line {
                None => area,
                Some(line) => Rectangle {
                    left: line.left.min(area.left),
                    bottom: line.bottom.min(area.bottom),
                    right: line.right.max(area.right),
                    top: line.top.max(area.top),
                },
            });
        }
        lines.extend(line);
        hits.push(lines);
        start = end;
    }
    hits
}

/// The page's resources, inherited from an ancestor if need be.
pub(super) fn page_resources(
    reader: &mut Reader,
    page: ObjectReference,
) -> Result<Dictionary, DocumentError> {
    let mut node = reader.resolve(&PdfObject::Reference(page))?;
    for _ in 0..MAX_ANCESTORS {
        let Some(dictionary) = node.as_dictionary() else {
            break;
        };
        if let PdfObject::Dictionary(resources) = reader.resolve_entry(dictionary, "Resources")? {
            return Ok(resources);
        }
        node = reader.resolve_entry(dictionary, "Parent")?;
    }
    Ok(Dictionary::new())
}

/// Load the font a resource name selects, if the resources have it.
fn load_font(
    reader: &mut Reader,
    fonts: &Dictionary,
    name: &[u8],
    programs: bool,
    substitute: Option<&TrueTypeFont>,
) -> Option<Font> {
    let font = reader.resolve(fonts.get(name)?).ok()?;
    let font = font.as_dictionary()?;
    let map = to_unicode(reader, font);
    let name_entry = |key| entry(font, key).and_then(PdfObject::as_name);
    let subtype = name_entry("Subtype").unwrap_or_default();
    // A subset font's name starts with six letters and a plus sign.
    let name = name_entry("BaseFont").unwrap_or_default();
    let name = match name.get(6) {
        Some(b'+') => name.get(7..).unwrap_or_default(),
        _ => name,
    };
    let name: Arc<str> = Arc::from(String::from_utf8_lossy(name).as_ref());
    let encoding = reader.resolve_entry(font, "Encoding").ok()?;
    if subtype == b"Type0" {
        let two_byte = matches!(encoding.as_name(), Some(b"Identity-H" | b"Identity-V"));
        let descendant = match reader.resolve_entry(font, "DescendantFonts") {
            Ok(PdfObject::Array(fonts)) => fonts.first().and_then(|font| reader.resolve(font).ok()),
            _ => None,
        };
        let descendant = descendant.as_ref().and_then(PdfObject::as_dictionary);
        let program = descendant
            .filter(|_| programs)
            .and_then(|font| font_program(reader, font));
        let cid_to_glyph = match descendant.and_then(|font| entry(font, "CIDToGIDMap")) {
            Some(PdfObject::Reference(map)) if program.is_some() => {
                reader.stream(map.number).ok().flatten()
            }
            _ => None,
        };
        let cff = descendant
            .filter(|_| programs && program.is_none())
            .and_then(|font| cff_program(reader, font));
        return Some(Font {
            name,
            decoder: Decoder::Composite { map, two_byte },
            widths: composite_widths(reader, font),
            program,
            cff,
            type1: None,
            type3: None,
            has_encoding: true,
            cid_to_glyph,
        });
    }
    let base_font = name_entry("BaseFont").unwrap_or_default();
    let has_encoding = !matches!(encoding, PdfObject::Null);
    let encoding = match encoding {
        PdfObject::Name(encoding) => SimpleEncoding::for_font(subtype, base_font, Some(&encoding)),
        PdfObject::Dictionary(dictionary) => {
            let base = reader.resolve_entry(&dictionary, "BaseEncoding").ok()?;
            let mut encoding = SimpleEncoding::for_font(subtype, base_font, base.as_name());
            if let PdfObject::Array(differences) =
                reader.resolve_entry(&dictionary, "Differences").ok()?
            {
                encoding.apply_differences(&differences);
            }
            encoding
        }
        _ => SimpleEncoding::for_font(subtype, base_font, None),
    };
    let mut widths = simple_widths(reader, font, subtype, base_font);
    // A standard 14 font may give no widths; Adobe's metrics give them.
    if let Widths::Simple {
        first,
        widths: given,
        missing,
    } = &mut widths
        && given.is_empty()
        && subtype != b"Type3"
        && let Some(standard) = StandardFont::from_name(&name)
    {
        *first = 0;
        *given = (0..=255_u8)
            .map(|code| {
                let width = encoding
                    .glyph_name(code)
                    .and_then(|glyph| standard.width(glyph));
                width.map_or(*missing, |width| f64::from(width) / 1000.0)
            })
            .collect();
    }
    // Any other font that gives no widths is spaced by the font that will
    // draw it.
    if let Widths::Simple {
        first,
        widths: given,
        missing,
    } = &mut widths
        && given.is_empty()
        && let Some(substitute) = substitute
    {
        *first = 0;
        *given = (0..=255_u8)
            .map(|code| {
                let character = encoding.text(code).and_then(|text| text.chars().next());
                character
                    .and_then(|character| substitute.glyph_index(3, 1, u32::from(character)))
                    .and_then(|glyph| substitute.advance(glyph))
                    .map_or(*missing, |advance| {
                        f64::from(advance) / f64::from(substitute.units_per_em())
                    })
            })
            .collect();
    }
    let program = if programs {
        font_program(reader, font)
    } else {
        None
    };
    let cff = if programs && program.is_none() {
        cff_program(reader, font)
    } else {
        None
    };
    let type3 = if programs && subtype == b"Type3" {
        type3_font(reader, font).map(Rc::new)
    } else {
        None
    };
    let type1 = if programs && program.is_none() && cff.is_none() {
        font_file(reader, font, "FontFile")
            .and_then(Type1Font::parse)
            .map(Rc::new)
    } else {
        None
    };
    Some(Font {
        name,
        decoder: Decoder::Simple { map, encoding },
        widths,
        program,
        cff,
        type1,
        type3,
        has_encoding,
        cid_to_glyph: None,
    })
}

/// Read a Type 3 font's matrix, glyph procedures, and resources.
fn type3_font(reader: &mut Reader, font: &Dictionary) -> Option<Type3Font> {
    let matrix: Vec<f64> = match reader.resolve_entry(font, "FontMatrix").ok()? {
        PdfObject::Array(values) => values.iter().filter_map(PdfObject::as_number).collect(),
        _ => return None,
    };
    let matrix = <Matrix>::try_from(matrix.as_slice()).ok()?;
    let PdfObject::Dictionary(procedures) = reader.resolve_entry(font, "CharProcs").ok()? else {
        return None;
    };
    let resources = match reader.resolve_entry(font, "Resources").ok()? {
        PdfObject::Dictionary(resources) => Some(resources),
        _ => None,
    };
    Some(Type3Font {
        matrix,
        procedures,
        resources,
    })
}

/// The data of the font program a font descriptor embeds under a key.
fn font_file(reader: &mut Reader, font: &Dictionary, key: &str) -> Option<Vec<u8>> {
    let PdfObject::Dictionary(descriptor) = reader.resolve_entry(font, "FontDescriptor").ok()?
    else {
        return None;
    };
    let PdfObject::Reference(file) = entry(&descriptor, key)? else {
        return None;
    };
    reader.stream(file.number).ok()?
}

/// Read the TrueType program embedded for a font: its `/FontFile2`, or an
/// OpenType `/FontFile3` with TrueType outlines.
fn font_program(reader: &mut Reader, font: &Dictionary) -> Option<Rc<TrueTypeFont>> {
    let found = font_file(reader, font, "FontFile2").and_then(TrueTypeFont::parse);
    found
        .or_else(|| font_file(reader, font, "FontFile3").and_then(TrueTypeFont::parse))
        .map(Rc::new)
}

/// Read the CFF program embedded for a font: its `/FontFile3`, bare or as
/// an OpenType file with CFF outlines.
fn cff_program(reader: &mut Reader, font: &Dictionary) -> Option<Rc<CffFont>> {
    font_file(reader, font, "FontFile3")
        .and_then(CffFont::parse)
        .map(Rc::new)
}

/// Read a font's ToUnicode map, if it has a usable one.
fn to_unicode(reader: &mut Reader, font: &Dictionary) -> Option<ToUnicodeMap> {
    let PdfObject::Reference(stream) = entry(font, "ToUnicode")? else {
        return None;
    };
    let data = reader.stream(stream.number).ok()??;
    ToUnicodeMap::parse(&data)
        .ok()
        .filter(|map| !map.is_empty())
}

/// The widths of a simple font, from `/Widths` and the font descriptor.
fn simple_widths(
    reader: &mut Reader,
    font: &Dictionary,
    subtype: &[u8],
    base_font: &[u8],
) -> Widths {
    // Glyph space is a thousandth of text space, except in a Type 3 font,
    // whose font matrix sets it.
    let mut unit = 0.001;
    if subtype == b"Type3"
        && let Ok(PdfObject::Array(matrix)) = reader.resolve_entry(font, "FontMatrix")
        && let Some(scale) = matrix.first().and_then(PdfObject::as_number)
    {
        unit = scale.abs();
    }
    let widths: Vec<f64> = match reader.resolve_entry(font, "Widths") {
        Ok(PdfObject::Array(widths)) => widths
            .iter()
            .map(|width| match width {
                PdfObject::Reference(_) => reader.resolve(width).ok().and_then(|w| w.as_number()),
                _ => width.as_number(),
            })
            .map(|width| width.unwrap_or(0.0) * unit)
            .collect(),
        _ => Vec::new(),
    };
    let first = match reader.resolve_entry(font, "FirstChar") {
        Ok(value) => value.as_integer().and_then(|v| usize::try_from(v).ok()),
        Err(_) => None,
    };
    // Without widths, as for the standard 14 fonts, whose metrics are not
    // included, every glyph gets one typical width.
    let missing = match reader.resolve_entry(font, "FontDescriptor") {
        Ok(PdfObject::Dictionary(descriptor)) if !widths.is_empty() => {
            match reader.resolve_entry(&descriptor, "MissingWidth") {
                Ok(width) => width.as_number().unwrap_or(0.0),
                Err(_) => 0.0,
            }
        }
        _ if !widths.is_empty() => 0.0,
        _ if base_font.windows(7).any(|part| part == b"Courier") => 600.0,
        _ => DEFAULT_WIDTH,
    };
    Widths::Simple {
        first: first.unwrap_or(0),
        widths,
        missing: missing * unit,
    }
}

/// The widths of a composite font, from its descendant font's `/DW` and
/// `/W`.
fn composite_widths(reader: &mut Reader, font: &Dictionary) -> Widths {
    let mut result = BTreeMap::new();
    let mut default = 1.0;
    let descendant = match reader.resolve_entry(font, "DescendantFonts") {
        Ok(PdfObject::Array(fonts)) => fonts.first().and_then(|font| reader.resolve(font).ok()),
        _ => None,
    };
    if let Some(PdfObject::Dictionary(descendant)) = descendant {
        if let Ok(width) = reader.resolve_entry(&descendant, "DW")
            && let Some(width) = width.as_number()
        {
            default = width / 1000.0;
        }
        if let Ok(PdfObject::Array(entries)) = reader.resolve_entry(&descendant, "W") {
            // Either `first [w1 w2 ...]` or `first last w`.
            let mut entries = entries.iter();
            while let Some(first) = entries.next() {
                let Some(first) = first.as_integer().and_then(|v| u32::try_from(v).ok()) else {
                    continue;
                };
                match entries.next().map(|second| reader.resolve(second)) {
                    Some(Ok(PdfObject::Array(widths))) => {
                        for (cid, width) in (first..).zip(widths.iter().take(65_536)) {
                            if let Some(width) = width.as_number() {
                                result.insert(cid, width / 1000.0);
                            }
                        }
                    }
                    Some(Ok(last)) => {
                        let last = last.as_integer().and_then(|v| u32::try_from(v).ok());
                        let width = entries.next().and_then(PdfObject::as_number);
                        if let (Some(last), Some(width)) = (last, width) {
                            for cid in first..=last.min(first.saturating_add(65_535)) {
                                result.insert(cid, width / 1000.0);
                            }
                        }
                    }
                    _ => break,
                }
            }
        }
    }
    Widths::Composite {
        widths: result,
        default,
    }
}
