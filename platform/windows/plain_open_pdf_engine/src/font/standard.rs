//! The standard 14 fonts that every PDF viewer has (ISO 32000-2 section
//! 9.6.2.2), with Adobe's metrics for them.

use super::SimpleEncoding;
use super::standard_metrics::{self as metrics, Metrics};

/// One of the standard 14 fonts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StandardFont {
    /// Helvetica.
    Helvetica,
    /// Helvetica-Bold.
    HelveticaBold,
    /// Helvetica-Oblique.
    HelveticaOblique,
    /// Helvetica-BoldOblique.
    HelveticaBoldOblique,
    /// Times-Roman.
    TimesRoman,
    /// Times-Bold.
    TimesBold,
    /// Times-Italic.
    TimesItalic,
    /// Times-BoldItalic.
    TimesBoldItalic,
    /// Courier.
    Courier,
    /// Courier-Bold.
    CourierBold,
    /// Courier-Oblique.
    CourierOblique,
    /// Courier-BoldOblique.
    CourierBoldOblique,
    /// Symbol.
    Symbol,
    /// ZapfDingbats.
    ZapfDingbats,
}

impl StandardFont {
    /// All 14, in the order the enumeration lists them.
    pub const ALL: [Self; 14] = [
        Self::Helvetica,
        Self::HelveticaBold,
        Self::HelveticaOblique,
        Self::HelveticaBoldOblique,
        Self::TimesRoman,
        Self::TimesBold,
        Self::TimesItalic,
        Self::TimesBoldItalic,
        Self::Courier,
        Self::CourierBold,
        Self::CourierOblique,
        Self::CourierBoldOblique,
        Self::Symbol,
        Self::ZapfDingbats,
    ];

    /// The font's PostScript name, as a `/BaseFont` gives it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Helvetica => "Helvetica",
            Self::HelveticaBold => "Helvetica-Bold",
            Self::HelveticaOblique => "Helvetica-Oblique",
            Self::HelveticaBoldOblique => "Helvetica-BoldOblique",
            Self::TimesRoman => "Times-Roman",
            Self::TimesBold => "Times-Bold",
            Self::TimesItalic => "Times-Italic",
            Self::TimesBoldItalic => "Times-BoldItalic",
            Self::Courier => "Courier",
            Self::CourierBold => "Courier-Bold",
            Self::CourierOblique => "Courier-Oblique",
            Self::CourierBoldOblique => "Courier-BoldOblique",
            Self::Symbol => "Symbol",
            Self::ZapfDingbats => "ZapfDingbats",
        }
    }

    /// The standard font a `/BaseFont` name stands for: one of the 14
    /// names, or a common name for the same design, such as `Arial,Bold`,
    /// `TimesNewRomanPS-ItalicMT`, or `CourierNew`. Spaces are ignored.
    /// Narrow, condensed, light, and black styles have other metrics and
    /// give `None`.
    pub fn from_name(name: &str) -> Option<Self> {
        let name: String = name.chars().filter(|&c| c != ' ').collect();
        if let Some(&font) = Self::ALL.iter().find(|font| font.name() == name) {
            return Some(font);
        }
        let (family, style) = match name.find([',', '-']) {
            Some(at) => (&name[..at], &name[at + 1..]),
            None => (name.as_str(), ""),
        };
        let family = family.trim_end_matches("MT").trim_end_matches("PS");
        let style = style.trim_end_matches("MT");
        let bold = style.contains("Bold");
        let slanted = style.contains("Italic") || style.contains("Oblique");
        let rest = [
            "Bold", "Italic", "Oblique", "Roman", "Regular", "Normal", "Medium", "Book",
        ]
        .iter()
        .fold(style.to_string(), |rest, word| rest.replace(word, ""));
        if !rest.is_empty() {
            return None;
        }
        let pick = |plain, bold_font, slanted_font, both| match (bold, slanted) {
            (false, false) => plain,
            (true, false) => bold_font,
            (false, true) => slanted_font,
            (true, true) => both,
        };
        match family {
            "Helvetica" | "Arial" => Some(pick(
                Self::Helvetica,
                Self::HelveticaBold,
                Self::HelveticaOblique,
                Self::HelveticaBoldOblique,
            )),
            "Times" | "TimesNewRoman" => Some(pick(
                Self::TimesRoman,
                Self::TimesBold,
                Self::TimesItalic,
                Self::TimesBoldItalic,
            )),
            "Courier" | "CourierNew" => Some(pick(
                Self::Courier,
                Self::CourierBold,
                Self::CourierOblique,
                Self::CourierBoldOblique,
            )),
            "Symbol" if style.is_empty() => Some(Self::Symbol),
            "ZapfDingbats" | "Dingbats" if style.is_empty() => Some(Self::ZapfDingbats),
            _ => None,
        }
    }

    fn metrics(self) -> &'static Metrics {
        match self {
            Self::Helvetica => &metrics::HELVETICA,
            Self::HelveticaBold => &metrics::HELVETICA_BOLD,
            Self::HelveticaOblique => &metrics::HELVETICA_OBLIQUE,
            Self::HelveticaBoldOblique => &metrics::HELVETICA_BOLD_OBLIQUE,
            Self::TimesRoman => &metrics::TIMES_ROMAN,
            Self::TimesBold => &metrics::TIMES_BOLD,
            Self::TimesItalic => &metrics::TIMES_ITALIC,
            Self::TimesBoldItalic => &metrics::TIMES_BOLD_ITALIC,
            Self::Courier => &metrics::COURIER,
            Self::CourierBold => &metrics::COURIER_BOLD,
            Self::CourierOblique => &metrics::COURIER_OBLIQUE,
            Self::CourierBoldOblique => &metrics::COURIER_BOLD_OBLIQUE,
            Self::Symbol => &metrics::SYMBOL,
            Self::ZapfDingbats => &metrics::ZAPF_DINGBATS,
        }
    }

    /// The width of a glyph, by its name, in thousandths of the font
    /// size; `None` for a glyph the font does not have.
    pub fn width(self, glyph: &str) -> Option<u16> {
        let metrics = self.metrics();
        let index = metrics.names.binary_search(&glyph).ok()?;
        metrics.widths.get(index).copied()
    }

    /// The font's bounding box in thousandths of the font size: left,
    /// bottom, right, and top.
    pub fn bounding_box(self) -> [i16; 4] {
        self.metrics().bounding_box
    }

    /// How far the font's tallest letters rise above the baseline, in
    /// thousandths of the font size.
    pub fn ascender(self) -> i16 {
        self.metrics().ascender
    }

    /// How far the font's descenders reach below the baseline, as a
    /// negative number of thousandths of the font size.
    pub fn descender(self) -> i16 {
        self.metrics().descender
    }

    /// The height of the font's capital letters, in thousandths of the font
    /// size.
    pub fn cap_height(self) -> i16 {
        self.metrics().cap_height
    }

    /// The height of the font's lowercase x, in thousandths of the font size.
    pub fn x_height(self) -> i16 {
        self.metrics().x_height
    }

    /// The slant of the font's upright strokes, in degrees counterclockwise
    /// from vertical: negative for a font that leans right.
    pub fn italic_angle(self) -> f32 {
        self.metrics().italic_angle
    }

    /// Whether the font has its own symbols rather than Latin letters, as
    /// Symbol and ZapfDingbats do.
    pub fn is_symbolic(self) -> bool {
        matches!(self, Self::Symbol | Self::ZapfDingbats)
    }

    /// The encoding text is written in with this font: `WinAnsiEncoding`
    /// for the Latin fonts, and the font's own for Symbol and ZapfDingbats.
    pub fn encoding(self) -> SimpleEncoding {
        match self {
            Self::Symbol => SimpleEncoding::symbol(),
            Self::ZapfDingbats => SimpleEncoding::zapf_dingbats(),
            _ => SimpleEncoding::win_ansi(),
        }
    }

    /// The codes that show text in this font's [`encoding`](Self::encoding),
    /// one byte for each character. In Symbol and ZapfDingbats, a character
    /// below U+0100 that the encoding lacks is taken as a code itself, so
    /// that "a" shows Symbol's alpha, as it does in other PDF libraries.
    /// Any other character the encoding lacks becomes a question mark, or a
    /// space in ZapfDingbats, which has none.
    pub fn encode(self, text: &str) -> Vec<u8> {
        let encoding = self.encoding();
        let mut codes = std::collections::BTreeMap::new();
        for code in 0..=255_u8 {
            // WinAnsiEncoding gives its unused codes a bullet, whose own
            // code is 0x95.
            if encoding.glyph_name(code) == Some("bullet") && code != 0x95 && !self.is_symbolic() {
                continue;
            }
            if let Some(character) = encoding.text(code).and_then(|text| text.chars().next()) {
                codes.entry(character).or_insert(code);
            }
        }
        let missing = if self == Self::ZapfDingbats {
            b' '
        } else {
            b'?'
        };
        let raw = |character: char| {
            let code = u8::try_from(u32::from(character)).ok()?;
            let shown = self.is_symbolic() && encoding.glyph_name(code).is_some();
            shown.then_some(code)
        };
        text.chars()
            .map(|character| {
                let code = codes.get(&character).copied();
                code.or_else(|| raw(character)).unwrap_or(missing)
            })
            .collect()
    }

    /// The width of text shown in this font at a size, as [`encode`]
    /// (Self::encode) writes it, in the same units as the size.
    pub fn text_width(self, text: &str, size: f64) -> f64 {
        let encoding = self.encoding();
        let total: u32 = self
            .encode(text)
            .into_iter()
            .filter_map(|code| self.width(encoding.glyph_name(code)?))
            .map(u32::from)
            .sum();
        f64::from(total) / 1000.0 * size
    }
}
