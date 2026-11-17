//! An independent PDF engine under development for Plain Open PDF.
//!
//! Opens documents and reads their pages, text, and structure; see
//! [`Document`]. Saving and rendering are first versions: rendering draws
//! vector graphics, text, and most images. [`parse_header`] and [`parse_object`] expose the
//! header and object syntax parsers on their own.
//!
//! ```
//! use plain_open_pdf_engine::{PdfVersion, parse_header};
//!
//! let version = parse_header(b"%PDF-1.7\n")?;
//! assert_eq!(version, PdfVersion { major: 1, minor: 7 });
//! # Ok::<(), plain_open_pdf_engine::HeaderError>(())
//! ```

mod crypto;
mod document;
mod filter;
mod font;
mod header;
mod object;
mod parse_error;
mod parser;
mod raster;
mod writer;

pub use document::{
    Attachment, Document, DocumentError, Information, Link, LinkTarget, OutlineItem, Page,
    Permissions, PixelRegion, Rectangle, Redaction, TextGlyph, TextSpan, TextWord, View,
    decode_text_string, encode_text_string,
};
pub use font::{
    CffFont, Segment, SimpleEncoding, StandardFont, ToUnicodeMap, TrueTypeFont, Type1Font,
    glyph_text,
};
pub use header::{HeaderError, PdfVersion, parse_header};
pub use object::{Dictionary, ObjectReference, ParsedObject, PdfObject};
pub use parse_error::{ParseError, ParseErrorKind};
pub use parser::{Operation, ParseLimits, parse_content, parse_object};
pub use raster::Pixmap;
pub use writer::write_object;
