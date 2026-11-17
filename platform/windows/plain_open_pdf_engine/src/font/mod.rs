//! Fonts: what is needed to turn the character codes of shown text into
//! Unicode.

mod cff;
mod encoding;
mod standard;
mod standard_metrics;
mod tables;
mod to_unicode;
mod truetype;
mod type1;

pub use cff::CffFont;
pub use encoding::{SimpleEncoding, glyph_text};
pub use standard::StandardFont;
pub use to_unicode::ToUnicodeMap;
pub use truetype::{Segment, TrueTypeFont};
pub use type1::Type1Font;
