//! Opening a PDF file: cross-reference lookup, object loading, and pages.

mod add_text;
mod attachments;
mod color;
mod destination;
mod function;
mod image;
mod links;
mod outline;
mod page_text;
mod pages;
mod prune;
mod reader;
mod redact;
mod render;
mod save;
mod security;
mod shading;
mod text;
mod xref;

use crate::font::{StandardFont, TrueTypeFont};
use crate::header::locate_header;
use crate::raster::Pixmap;
use crate::{Dictionary, HeaderError, ObjectReference, ParseError, PdfObject, PdfVersion};
use reader::Reader;
use std::fmt;
use std::sync::Arc;

pub use attachments::Attachment;
pub use destination::View;
pub use links::{Link, LinkTarget};
pub use outline::OutlineItem;
pub use page_text::{TextGlyph, TextSpan, TextWord};
pub use pages::{Page, Rectangle};
pub use redact::Redaction;
pub use security::Permissions;
pub use text::{decode_text_string, encode_text_string};

/// Why a document could not be opened or one of its objects read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentError {
    /// The file has no recognizable PDF header.
    Header(HeaderError),
    /// Object syntax is malformed. The offset is measured from the start of
    /// the file, or of the decoded object stream that holds the object.
    Syntax(ParseError),
    /// The cross-reference data at or near this file offset is missing or
    /// malformed.
    CrossReference {
        /// File offset where cross-reference data was expected.
        offset: usize,
    },
    /// The stream whose data starts at this file offset cannot be decoded.
    Stream {
        /// File offset of the first byte of stream data.
        offset: usize,
    },
    /// The document's object structure is unusable, for the stated reason.
    Structure(&'static str),
    /// The document is encrypted and no password, or an empty one, was
    /// given.
    PasswordRequired,
    /// The document is encrypted and the password given is wrong.
    WrongPassword,
    /// The document needs a feature this engine does not implement yet.
    Unsupported(&'static str),
}

impl fmt::Display for DocumentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Header(error) => error.fmt(formatter),
            Self::Syntax(error) => error.fmt(formatter),
            Self::CrossReference { offset } => {
                write!(formatter, "invalid cross-reference data near byte {offset}")
            }
            Self::Stream { offset } => {
                write!(formatter, "cannot decode stream data at byte {offset}")
            }
            Self::Structure(reason) => write!(formatter, "invalid document: {reason}"),
            Self::PasswordRequired => formatter.write_str("a password is required"),
            Self::WrongPassword => formatter.write_str("the password is wrong"),
            Self::Unsupported(feature) => write!(formatter, "unsupported: {feature}"),
        }
    }
}

impl std::error::Error for DocumentError {}

/// Read the page list reachable from a trailer, decrypting if it says to.
fn read_pages(
    reader: &mut Reader,
    trailer: Dictionary,
    password: &[u8],
) -> Result<(Dictionary, Vec<Page>), DocumentError> {
    security::install(reader, &trailer, password)?;
    let pages = pages::collect(reader, &trailer)?;
    Ok((trailer, pages))
}

/// A rectangle of a page image, in pixels from its top left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelRegion {
    /// The first column of pixels, counted from the image's left edge.
    pub left: usize,
    /// The first row of pixels, counted from the image's top edge.
    pub top: usize,
    /// The number of columns.
    pub width: usize,
    /// The number of rows.
    pub height: usize,
}

/// An opened PDF document.
///
/// ```
/// use plain_open_pdf_engine::Document;
///
/// # fn example(bytes: Vec<u8>) -> Result<(), plain_open_pdf_engine::DocumentError> {
/// let document = Document::open(bytes)?;
/// for page in document.pages() {
///     println!("{} x {}", page.media_box.width(), page.media_box.height());
/// }
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct Document {
    version: PdfVersion,
    reader: Reader,
    trailer: Dictionary,
    pages: Vec<Page>,
    /// Draws text whose own font cannot be drawn yet.
    fallback_font: Option<TrueTypeFont>,
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

impl Document {
    /// Open a document held in memory and read its page list.
    ///
    /// An encrypted document opens this way when its user password is
    /// empty, as it is for files that only restrict printing or copying.
    ///
    /// # Errors
    ///
    /// Returns a [`DocumentError`] when the header, cross-reference data, or
    /// page tree cannot be read, or when a password is needed.
    pub fn open(bytes: Vec<u8>) -> Result<Self, DocumentError> {
        Self::open_with_password(bytes, b"")
    }

    /// A new document without pages, to add pages to with
    /// [`Document::insert_blank_page`].
    pub fn new() -> Self {
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [] /Count 0 >>",
        ];
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(bytes.len());
            bytes.extend(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
        }
        let table = bytes.len();
        bytes.extend(b"xref\n0 3\n0000000000 65535 f \n");
        for offset in offsets {
            bytes.extend(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend(
            format!("trailer\n<< /Size 3 /Root 1 0 R >>\nstartxref\n{table}\n%%EOF\n").as_bytes(),
        );
        // The file above is well formed, so opening it cannot fail.
        Self::open(bytes).unwrap_or_else(|_| unreachable!("a new document opens"))
    }

    /// Open a document with its user or owner password.
    ///
    /// The password is ignored when the document is not encrypted.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentError::WrongPassword`] when a password is given and
    /// is wrong, [`DocumentError::PasswordRequired`] when an empty one is
    /// given and the document needs one, and otherwise fails as
    /// [`Document::open`] does.
    pub fn open_with_password(bytes: Vec<u8>, password: &[u8]) -> Result<Self, DocumentError> {
        Self::open_shared(Arc::new(bytes), password)
    }

    /// Open a document as [`Document::open_with_password`] does, from bytes
    /// that other documents may share: the document only reads them, so
    /// several documents opened from one file need only one copy of it.
    ///
    /// # Errors
    ///
    /// Fails as [`Document::open_with_password`] does.
    pub fn open_shared(bytes: Arc<Vec<u8>>, password: &[u8]) -> Result<Self, DocumentError> {
        let (base, version) = locate_header(&bytes).map_err(DocumentError::Header)?;
        let mut reader = Reader::new(bytes, base);
        let loaded = xref::load(&mut reader);
        let mut opened = match loaded.trailer.clone() {
            Some(trailer) if loaded.intact => read_pages(&mut reader, trailer, password),
            _ => Err(DocumentError::CrossReference { offset: 0 }),
        };
        // Cross-reference data that is damaged, or that leads to no pages, is
        // rebuilt from the object definitions and the page tree read again.
        let rebuild = match &opened {
            Ok((_, pages)) => pages.is_empty(),
            Err(
                DocumentError::Unsupported(_)
                | DocumentError::PasswordRequired
                | DocumentError::WrongPassword,
            ) => false,
            Err(_) => true,
        };
        if rebuild {
            let repaired = xref::repair(&mut reader, loaded.trailer, password)
                .transpose()
                .map(|trailer| read_pages(&mut reader, trailer?, password));
            if let Some(repaired) = repaired
                && (repaired.is_ok() || opened.is_err())
            {
                opened = repaired;
            }
        }
        let (trailer, pages) = opened?;
        Ok(Self {
            version,
            reader,
            trailer,
            pages,
            fallback_font: None,
        })
    }

    /// The version declared in the file header.
    pub fn version(&self) -> PdfVersion {
        self.version
    }

    /// The number of pages.
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// The pages in document order.
    pub fn pages(&self) -> &[Page] {
        &self.pages
    }

    /// The trailer dictionary, which names the catalog and document information.
    pub fn trailer(&self) -> &Dictionary {
        &self.trailer
    }

    /// Load the object a reference names; a stream yields its dictionary.
    ///
    /// A reference to an object the file does not define yields null.
    ///
    /// # Errors
    ///
    /// Returns a [`DocumentError`] when the object cannot be parsed.
    pub fn object(&mut self, reference: ObjectReference) -> Result<PdfObject, DocumentError> {
        self.reader.resolve(&PdfObject::Reference(reference))
    }

    /// Decode the data of the stream a reference names.
    ///
    /// Returns `None` when the object is not a stream.
    ///
    /// # Errors
    ///
    /// Returns a [`DocumentError`] when the object cannot be parsed or its
    /// data cannot be decoded. A `DCTDecode` stream yields its samples,
    /// one byte each; progressive JPEG and the other image filters
    /// (`JPXDecode`, `CCITTFaxDecode`, `JBIG2Decode`) report `Unsupported`.
    pub fn stream(&mut self, reference: ObjectReference) -> Result<Option<Vec<u8>>, DocumentError> {
        self.reader.stream(reference.number)
    }

    /// Read the document information dictionary.
    ///
    /// A document without one yields empty information.
    ///
    /// # Errors
    ///
    /// Returns a [`DocumentError`] when the dictionary cannot be parsed.
    pub fn information(&mut self) -> Result<Information, DocumentError> {
        let PdfObject::Dictionary(info) = self.reader.resolve_entry(&self.trailer, "Info")? else {
            return Ok(Information::default());
        };
        let mut text = |key| match self.reader.resolve_entry(&info, key) {
            Ok(PdfObject::String(bytes)) => Some(decode_text_string(&bytes)),
            _ => None,
        };
        Ok(Information {
            title: text("Title"),
            author: text("Author"),
            subject: text("Subject"),
            keywords: text("Keywords"),
            creator: text("Creator"),
            producer: text("Producer"),
            creation_date: text("CreationDate"),
            modification_date: text("ModDate"),
        })
    }

    /// Every text entry of the document information dictionary, by key,
    /// including custom ones that [`Document::information`] leaves out.
    /// Entries whose values are not strings are left out.
    ///
    /// # Errors
    ///
    /// Returns a [`DocumentError`] when the dictionary cannot be parsed.
    pub fn information_entries(&mut self) -> Result<Vec<(String, String)>, DocumentError> {
        let PdfObject::Dictionary(info) = self.reader.resolve_entry(&self.trailer, "Info")? else {
            return Ok(Vec::new());
        };
        let mut entries = Vec::new();
        for (key, value) in &info {
            if let PdfObject::String(bytes) = self.reader.resolve(value)? {
                let key = String::from_utf8_lossy(key).into_owned();
                entries.push((key, decode_text_string(&bytes)));
            }
        }
        Ok(entries)
    }

    /// Read the document outline (bookmarks) in reading order.
    ///
    /// A document without an outline yields an empty list.
    ///
    /// # Errors
    ///
    /// Returns a [`DocumentError`] when the catalog cannot be parsed.
    pub fn outline(&mut self) -> Result<Vec<OutlineItem>, DocumentError> {
        outline::read(&mut self.reader, &self.trailer, &self.pages)
    }

    /// Read the links on a page, given its zero-based index.
    ///
    /// A page index beyond the last page yields an empty list.
    ///
    /// # Errors
    ///
    /// Returns a [`DocumentError`] when the page cannot be parsed.
    pub fn links(&mut self, page_index: usize) -> Result<Vec<Link>, DocumentError> {
        match self.pages.get(page_index) {
            Some(page) => links::read(&mut self.reader, &self.trailer, &self.pages, page),
            None => Ok(Vec::new()),
        }
    }

    /// The page a named destination leads to, and the view of it when the
    /// destination gives one; `None` when no destination has the name.
    ///
    /// The name is looked up in the catalog's `/Names` tree and then in its
    /// `/Dests` dictionary, the older form.
    ///
    /// # Errors
    ///
    /// Returns a [`DocumentError`] when the catalog cannot be parsed.
    pub fn named_destination(
        &mut self,
        name: &[u8],
    ) -> Result<Option<(usize, Option<View>)>, DocumentError> {
        let Some(mut destinations) =
            destination::Destinations::new(&mut self.reader, &self.trailer, &self.pages)?
        else {
            return Ok(None);
        };
        let by_string = destinations.resolve(&PdfObject::String(name.to_vec()));
        Ok(by_string.or_else(|| destinations.resolve(&PdfObject::Name(name.to_vec()))))
    }

    /// List the files embedded in the document.
    ///
    /// # Errors
    ///
    /// Returns a [`DocumentError`] when the catalog cannot be parsed.
    pub fn attachments(&mut self) -> Result<Vec<Attachment>, DocumentError> {
        attachments::read(&mut self.reader, &self.trailer)
    }

    /// The text a page shows, in the order its content stream shows it.
    ///
    /// The text is assembled from [`Document::page_glyphs`]: a line ends
    /// where the next glyph leaves the baseline or after a gap wider than
    /// 0.8 of the font size, and a space is added where a smaller gap
    /// between glyphs is wide enough to separate words. Text is not
    /// reordered by position.
    ///
    /// # Errors
    ///
    /// Returns a [`DocumentError`] when the page or its content cannot be
    /// read.
    pub fn page_text(&mut self, page_index: usize) -> Result<String, DocumentError> {
        Ok(page_text::assemble(&self.page_glyphs(page_index)?))
    }

    /// The glyphs a page shows, in the order its content stream shows them,
    /// each with its text and position. Glyphs inside form XObjects are
    /// included.
    ///
    /// Font programs are not read: widths come from the font dictionary,
    /// and a font without them, such as a standard 14 font, gets one
    /// typical width for every glyph, so its positions are approximate.
    ///
    /// # Errors
    ///
    /// Returns a [`DocumentError`] when the page or its content cannot be
    /// read.
    pub fn page_glyphs(&mut self, page_index: usize) -> Result<Vec<TextGlyph>, DocumentError> {
        let Some(reference) = self.pages.get(page_index).and_then(|page| page.reference) else {
            return Ok(Vec::new());
        };
        let content = self.page_content(page_index)?;
        page_text::glyphs(&mut self.reader, reference, &content)
    }

    /// The words of a page, each with the box around it and its block,
    /// line, and word numbers, in the order the page shows them. Positions
    /// are those of [`Document::page_glyphs`]; [`Page::to_view`] turns them
    /// into the space a viewer draws in.
    ///
    /// A word ends at white space or at a gap wide enough to separate
    /// words. A block is a group of lines that follow one another closely.
    ///
    /// # Errors
    ///
    /// Returns a [`DocumentError`] when the page or its content cannot be
    /// read.
    pub fn page_words(&mut self, page_index: usize) -> Result<Vec<TextWord>, DocumentError> {
        Ok(page_text::words(&self.page_glyphs(page_index)?))
    }

    /// The runs of text on a page that share a line, a font, a size, and a
    /// fill color, in the order the page shows them: what is needed to
    /// replace a piece of text with other text that looks the same.
    ///
    /// # Errors
    ///
    /// Returns a [`DocumentError`] when the page or its content cannot be
    /// read.
    pub fn page_spans(&mut self, page_index: usize) -> Result<Vec<TextSpan>, DocumentError> {
        Ok(page_text::spans(&self.page_glyphs(page_index)?))
    }

    /// Find text on a page, ignoring case. A line end in the page's text
    /// matches a space in `needle`.
    ///
    /// Each place found is a list of rectangles, one for each line the
    /// match touches, in the same coordinates as [`Document::page_glyphs`].
    /// Their heights are estimated from the font size, since font programs
    /// are not read. The search runs over [`Document::page_text`], so it
    /// shares its limits: text is matched in content order.
    ///
    /// # Errors
    ///
    /// Returns a [`DocumentError`] when the page or its content cannot be
    /// read.
    pub fn search_page(
        &mut self,
        page_index: usize,
        needle: &str,
    ) -> Result<Vec<Vec<Rectangle>>, DocumentError> {
        Ok(page_text::search(&self.page_glyphs(page_index)?, needle))
    }

    /// The decoded content stream of a page, given its zero-based index.
    ///
    /// A page whose `/Contents` is an array of streams yields them joined
    /// with a line feed, since each ends at a token boundary. Parse the
    /// result with [`parse_content`](crate::parse_content). A page without
    /// content, or an index beyond the last page, yields no bytes.
    ///
    /// # Errors
    ///
    /// Returns a [`DocumentError`] when the page or a stream cannot be read.
    pub fn page_content(&mut self, page_index: usize) -> Result<Vec<u8>, DocumentError> {
        let Some(reference) = self.pages.get(page_index).and_then(|page| page.reference) else {
            return Ok(Vec::new());
        };
        let page = self.reader.resolve(&PdfObject::Reference(reference))?;
        let Some(contents) = page
            .as_dictionary()
            .and_then(|page| reader::entry(page, "Contents"))
        else {
            return Ok(Vec::new());
        };
        // `/Contents` is a stream, or an array of streams, usually indirect.
        if let PdfObject::Reference(stream) = contents
            && let Some(data) = self.reader.stream(stream.number)?
        {
            return Ok(data);
        }
        let PdfObject::Array(streams) = self.reader.resolve(contents)? else {
            return Ok(Vec::new());
        };
        let mut content = Vec::new();
        for stream in streams {
            if let PdfObject::Reference(stream) = stream
                && let Some(data) = self.reader.stream(stream.number)?
            {
                content.extend(data);
                content.push(b'\n');
            }
        }
        Ok(content)
    }

    /// Render a page's vector graphics to pixels, at `scale` pixels for
    /// each point: 1 gives 72 pixels an inch. The image covers the crop box,
    /// turned by the page's rotation, on a white background.
    ///
    /// This is a first, limited renderer: it fills, strokes, and clips
    /// paths, in page content and in form XObjects, and draws text from
    /// embedded TrueType and CFF fonts and, for other fonts, from the font
    /// given to
    /// [`Document::set_fallback_font`], and images in the filters this
    /// engine decodes. Shadings and patterns are not drawn.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentError::Structure`] for a page index beyond the last
    /// page, [`DocumentError::Unsupported`] when the image would be empty or
    /// too large, and a [`DocumentError`] when the page cannot be read.
    pub fn render_page(&mut self, page_index: usize, scale: f64) -> Result<Pixmap, DocumentError> {
        let Some(page) = self.pages.get(page_index).cloned() else {
            return Err(DocumentError::Structure("a page index is out of range"));
        };
        let content = self.page_content(page_index)?;
        let fallback = self.fallback_font.as_ref();
        render::render(&mut self.reader, &page, &content, scale, fallback, None)
    }

    /// Render one rectangle of a page's image, at `scale` pixels for each
    /// point. `region` is in pixels of the image [`Document::render_page`]
    /// gives at that scale, and the result holds exactly the pixels of
    /// that image inside it. A region that reaches past the image is cut
    /// to it, so the result may be smaller than the region asked for.
    ///
    /// The page is read and laid out as a whole; only the pixels inside
    /// the region are drawn, which is what saves time on a large image.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Document::render_page`], which depend on the
    /// whole page and not on the region, and
    /// [`DocumentError::Unsupported`] when no pixel of the region is in
    /// the image.
    pub fn render_page_region(
        &mut self,
        page_index: usize,
        scale: f64,
        region: PixelRegion,
    ) -> Result<Pixmap, DocumentError> {
        let Some(page) = self.pages.get(page_index).cloned() else {
            return Err(DocumentError::Structure("a page index is out of range"));
        };
        let content = self.page_content(page_index)?;
        let fallback = self.fallback_font.as_ref();
        let region = Some(region);
        render::render(&mut self.reader, &page, &content, scale, fallback, region)
    }

    /// Give the renderer a TrueType font to draw text with when a page's
    /// own font cannot be drawn: one that is not embedded, or one embedded
    /// in a format this engine does not read. Returns
    /// whether the data is a usable TrueType font; if not, any earlier
    /// fallback font stays.
    pub fn set_fallback_font(&mut self, data: Vec<u8>) -> bool {
        match TrueTypeFont::parse(data) {
            Some(font) => {
                self.fallback_font = Some(font);
                true
            }
            None => false,
        }
    }

    /// Write the document, with every change made to it, as a new file.
    ///
    /// The file holds each object that can be reached from the catalog and
    /// the information dictionary, uncompressed, with one cross-reference
    /// table. Stream data is copied as stored. Objects that nothing refers
    /// to any more, such as pages left out by [`Document::select_pages`],
    /// are not written.
    ///
    /// An encrypted document stays encrypted the way it was, with the same
    /// passwords and permissions, whichever password opened it. Use
    /// [`Document::save_decrypted`] to remove the protection and
    /// [`Document::save_encrypted`] to set new protection.
    ///
    /// # Errors
    ///
    /// Returns a [`DocumentError`] when the file cannot be written.
    pub fn save(&mut self) -> Result<Vec<u8>, DocumentError> {
        let encryption = self.reader.decryptor().map(|cipher| save::Encryption {
            cipher: cipher.clone(),
            dictionary: None,
        });
        save::write(&mut self.reader, self.version, &self.trailer, encryption)
    }

    /// Write the document as [`Document::save`] does, but without any
    /// encryption, so the saved file has neither passwords nor permissions.
    ///
    /// # Errors
    ///
    /// Returns a [`DocumentError`] when the file cannot be written.
    pub fn save_decrypted(&mut self) -> Result<Vec<u8>, DocumentError> {
        save::write(&mut self.reader, self.version, &self.trailer, None)
    }

    /// Write the document as [`Document::save`] does, protected with
    /// AES-256 (the standard security handler's revision 6) and the given
    /// passwords, in place of any protection it had.
    ///
    /// The user password opens the document with `permissions`; an empty
    /// one lets anyone open it, so that only the permissions apply. The
    /// owner password opens it with every permission and should differ from
    /// the user password. Passwords longer than 127 bytes of UTF-8 are cut
    /// there.
    ///
    /// # Errors
    ///
    /// Returns a [`DocumentError`] when the file cannot be written.
    pub fn save_encrypted(
        &mut self,
        user_password: &str,
        owner_password: &str,
        permissions: Permissions,
    ) -> Result<Vec<u8>, DocumentError> {
        let (cipher, dictionary) =
            security::Decryptor::protect(user_password, owner_password, permissions);
        let encryption = save::Encryption {
            cipher,
            dictionary: Some(dictionary),
        };
        save::write(
            &mut self.reader,
            self.version,
            &self.trailer,
            Some(encryption),
        )
    }

    /// How the document is encrypted, such as "Standard V4 R4 128-bit
    /// AES", or `None` when it is not.
    pub fn encryption(&self) -> Option<&str> {
        let cipher = self.reader.decryptor()?;
        Some(cipher.description.as_str())
    }

    /// What an encrypted document lets its user do, or `None` when it is
    /// not encrypted.
    pub fn permissions(&self) -> Option<Permissions> {
        let cipher = self.reader.decryptor()?;
        Some(Permissions::from_bits(cipher.permissions))
    }

    /// Define or replace the object a reference names. The change is held
    /// in memory until the document is saved.
    pub fn set_object(&mut self, reference: ObjectReference, value: PdfObject) {
        self.reader.set_object(reference.number, value, None);
        self.refresh_pages();
    }

    /// Define or replace a stream. `data` is stored as given, so the
    /// dictionary must name the filters it is encoded with, or none for
    /// plain data. `/Length` is set when the document is saved.
    pub fn set_stream(
        &mut self,
        reference: ObjectReference,
        dictionary: Dictionary,
        data: Vec<u8>,
    ) {
        let value = PdfObject::Dictionary(dictionary);
        self.reader.set_object(reference.number, value, Some(data));
        self.refresh_pages();
    }

    /// Define or replace a stream from its plain data, which is compressed
    /// with `FlateDecode`. Any `/Filter` and `/DecodeParms` in the
    /// dictionary are replaced.
    pub fn set_stream_data(
        &mut self,
        reference: ObjectReference,
        mut dictionary: Dictionary,
        data: &[u8],
    ) {
        dictionary.remove(b"DecodeParms".as_slice());
        let filter = PdfObject::Name(b"FlateDecode".to_vec());
        dictionary.insert(b"Filter".to_vec(), filter);
        self.set_stream(reference, dictionary, crate::filter::deflate_zlib(data));
    }

    /// Add an object under a number the document does not use yet.
    pub fn add_object(&mut self, value: PdfObject) -> ObjectReference {
        let highest = self.reader.object_numbers().last().copied().unwrap_or(0);
        let reference = ObjectReference {
            number: highest.saturating_add(1),
            generation: 0,
        };
        self.reader.set_object(reference.number, value, None);
        reference
    }

    /// Set an entry of the document information dictionary, such as `Title`
    /// or `Author`, or remove it with `None`.
    ///
    /// # Errors
    ///
    /// Returns a [`DocumentError`] when the dictionary cannot be parsed.
    pub fn set_information(&mut self, key: &str, value: Option<&str>) -> Result<(), DocumentError> {
        let mut info = match self.reader.resolve_entry(&self.trailer, "Info")? {
            PdfObject::Dictionary(info) => info,
            _ => Dictionary::new(),
        };
        match value {
            Some(text) => {
                let text = PdfObject::String(encode_text_string(text));
                info.insert(key.as_bytes().to_vec(), text);
            }
            None => {
                info.remove(key.as_bytes());
            }
        }
        let info = PdfObject::Dictionary(info);
        match reader::entry(&self.trailer, "Info") {
            Some(PdfObject::Reference(reference)) => {
                self.reader.set_object(reference.number, info, None);
            }
            _ => {
                let reference = PdfObject::Reference(self.add_object(info));
                self.trailer.insert(b"Info".to_vec(), reference);
            }
        }
        Ok(())
    }

    /// Replace the document outline (bookmarks) with the given items, in
    /// reading order; an empty list removes the outline.
    ///
    /// An item's level places it under the nearest earlier item of a lower
    /// level; a level that skips ahead is taken as one deeper than the item
    /// before it. An item with a page leads to the top of that page, and
    /// any other item leads nowhere. An item with a page and a view shows
    /// the page that way; one without a view leads to the top of the page.
    /// Items are written closed, and the old outline's colors and styles
    /// are not kept.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentError::Structure`] when the catalog is not an
    /// indirect object, and a [`DocumentError`] when it cannot be parsed.
    pub fn set_outline(&mut self, items: &[OutlineItem]) -> Result<(), DocumentError> {
        const CATALOG: DocumentError =
            DocumentError::Structure("the catalog is not an indirect object");
        let Some(&PdfObject::Reference(catalog_reference)) = reader::entry(&self.trailer, "Root")
        else {
            return Err(CATALOG);
        };
        let PdfObject::Dictionary(mut catalog) = self.object(catalog_reference)? else {
            return Err(CATALOG);
        };
        if items.is_empty() {
            catalog.remove(b"Outlines".as_slice());
            self.set_object(catalog_reference, PdfObject::Dictionary(catalog));
            return Ok(());
        }
        let root = self.add_object(PdfObject::Null);
        let references: Vec<ObjectReference> = items
            .iter()
            .map(|_| self.add_object(PdfObject::Null))
            .collect();

        // Each item's parent, from the levels, and each parent's children.
        let mut parents: Vec<Option<usize>> = Vec::new();
        let mut ancestors: Vec<usize> = Vec::new();
        for (index, item) in items.iter().enumerate() {
            let level = item.level.clamp(1, ancestors.len() + 1);
            ancestors.truncate(level - 1);
            parents.push(ancestors.last().copied());
            ancestors.push(index);
        }
        let children = |parent: Option<usize>| -> Vec<usize> {
            let indexes = 0..items.len();
            indexes.filter(|&index| parents[index] == parent).collect()
        };
        let mut descendants = vec![0_i64; items.len()];
        for index in (0..items.len()).rev() {
            if let Some(parent) = parents[index] {
                descendants[parent] += 1 + descendants[index];
            }
        }

        let reference = |index: usize| PdfObject::Reference(references[index]);
        let ends = |entries: &mut Dictionary, children: &[usize]| {
            if let (Some(&first), Some(&last)) = (children.first(), children.last()) {
                entries.insert(b"First".to_vec(), reference(first));
                entries.insert(b"Last".to_vec(), reference(last));
            }
        };
        for (index, item) in items.iter().enumerate() {
            let mut entries = Dictionary::new();
            let title = PdfObject::String(encode_text_string(&item.title));
            entries.insert(b"Title".to_vec(), title);
            let parent = parents[index].map_or(PdfObject::Reference(root), reference);
            entries.insert(b"Parent".to_vec(), parent);
            let siblings = children(parents[index]);
            let position = siblings.iter().position(|&sibling| sibling == index);
            let position = position.unwrap_or(0);
            if position > 0 {
                entries.insert(b"Prev".to_vec(), reference(siblings[position - 1]));
            }
            if let Some(&next) = siblings.get(position + 1) {
                entries.insert(b"Next".to_vec(), reference(next));
            }
            ends(&mut entries, &children(Some(index)));
            if descendants[index] > 0 {
                // A negative count marks the item as closed.
                let count = PdfObject::Integer(-descendants[index]);
                entries.insert(b"Count".to_vec(), count);
            }
            let page = item.page.and_then(|page| self.pages.get(page));
            if let Some(page) = page
                && let Some(target) = page.reference
            {
                let view = item.view.unwrap_or(View::Xyz {
                    left: None,
                    top: Some(page.crop_box.top),
                    zoom: None,
                });
                let mut destination = vec![PdfObject::Reference(target)];
                destination.extend(view.elements());
                entries.insert(b"Dest".to_vec(), PdfObject::Array(destination));
            }
            self.reader.set_object(
                references[index].number,
                PdfObject::Dictionary(entries),
                None,
            );
        }
        let mut entries = Dictionary::new();
        entries.insert(b"Type".to_vec(), PdfObject::Name(b"Outlines".to_vec()));
        let top = children(None);
        ends(&mut entries, &top);
        let count = PdfObject::Integer(i64::try_from(top.len()).unwrap_or(0));
        entries.insert(b"Count".to_vec(), count);
        self.reader
            .set_object(root.number, PdfObject::Dictionary(entries), None);
        catalog.insert(b"Outlines".to_vec(), PdfObject::Reference(root));
        self.set_object(catalog_reference, PdfObject::Dictionary(catalog));
        Ok(())
    }

    /// Remove what a page shows inside the given areas, as applying
    /// redaction annotations does, and paint each area with `fill`, a
    /// color of red, green, and blue from 0 to 1, unless it is `None`.
    ///
    /// The page's content is written again without:
    ///
    /// - each glyph whose middle lies inside an area, visible or not; the
    ///   glyphs after it on its line keep their places;
    /// - each path, image, and form XObject that lies wholly inside one area;
    /// - the pixels inside the areas of an image partly inside one, which is
    ///   stored again as RGB with those pixels white;
    /// - what lies inside the areas of a form XObject partly inside one,
    ///   which is copied and redacted in turn.
    ///
    /// A path partly inside an area stays whole, as do shadings, inline
    /// images partly inside an area, and images in filters this engine
    /// cannot decode; the result counts those images. Annotations are not
    /// changed. Areas are in the page's default user space. Once the
    /// document is saved, what was removed is no longer in the file, unless
    /// another page still uses the same content.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentError::Structure`] when the index is out of range
    /// or the page is not an indirect object, and a [`DocumentError`] when
    /// the page's content or a form it overlaps cannot be read; the page is
    /// then left as it was.
    pub fn redact(
        &mut self,
        page_index: usize,
        areas: &[Rectangle],
        fill: Option<[f64; 3]>,
    ) -> Result<Redaction, DocumentError> {
        let Some(page) = self.pages.get(page_index) else {
            return Err(DocumentError::Structure("a page index is out of range"));
        };
        let Some(reference) = page.reference else {
            return Err(DocumentError::Structure(
                "the page is not an indirect object",
            ));
        };
        let content = self.page_content(page_index)?;
        let result = redact::redact(&mut self.reader, reference, &content, areas, fill)?;
        self.refresh_pages();
        Ok(result)
    }

    /// Show text on a page in one of the standard 14 fonts, and return the
    /// width of its widest line.
    ///
    /// `origin` is where the first line's baseline starts, in the page's
    /// default user space; the text runs along the page's x axis. Each line
    /// feed starts a new line 1.2 times the size below the last. `color` is
    /// red, green, and blue from 0 to 1. The text is written in the font's
    /// [`encoding`](StandardFont::encoding), so a character it lacks shows
    /// as a question mark, and [`StandardFont::text_width`] measures it as
    /// it is shown. The page gains a font resource with the font's widths,
    /// reused by later text in the same font, and a content stream after
    /// its own, which is enclosed in `q` and `Q` so that it cannot change
    /// where the text goes.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentError::Structure`] when the index is out of range,
    /// the page is not an indirect object, or the size or origin is not a
    /// finite positive number and point, and a [`DocumentError`] when the
    /// page cannot be read.
    pub fn insert_text(
        &mut self,
        page_index: usize,
        origin: (f64, f64),
        text: &str,
        font: StandardFont,
        size: f64,
        color: [f64; 3],
    ) -> Result<f64, DocumentError> {
        let Some(page) = self.pages.get(page_index) else {
            return Err(DocumentError::Structure("a page index is out of range"));
        };
        let Some(reference) = page.reference else {
            return Err(DocumentError::Structure(
                "the page is not an indirect object",
            ));
        };
        let width =
            add_text::insert_text(&mut self.reader, reference, origin, text, font, size, color)?;
        self.refresh_pages();
        Ok(width)
    }

    /// Set a page's display rotation, clockwise in degrees. Any multiple of
    /// 90 is accepted and stored as 0, 90, 180, or 270.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentError::Structure`] when the index is out of range,
    /// the rotation is not a multiple of 90, or the page is not an indirect
    /// object, and a [`DocumentError`] when the page cannot be parsed.
    pub fn set_page_rotation(
        &mut self,
        page_index: usize,
        degrees: i32,
    ) -> Result<(), DocumentError> {
        if degrees % 90 != 0 {
            return Err(DocumentError::Structure(
                "a page rotation is a multiple of 90 degrees",
            ));
        }
        let page = self.pages.get(page_index);
        let page = page.ok_or(DocumentError::Structure("a page index is out of range"))?;
        const INDIRECT: DocumentError =
            DocumentError::Structure("the page is not an indirect object");
        let reference = page.reference.ok_or(INDIRECT)?;
        let PdfObject::Dictionary(mut entries) = self.object(reference)? else {
            return Err(INDIRECT);
        };
        let rotation = PdfObject::Integer(i64::from(degrees.rem_euclid(360)));
        entries.insert(b"Rotate".to_vec(), rotation);
        self.set_object(reference, PdfObject::Dictionary(entries));
        Ok(())
    }

    /// Make the page tree one node that lists every page, and give that
    /// node and its list of pages.
    fn page_list(&mut self) -> Result<(ObjectReference, Vec<PdfObject>), DocumentError> {
        const TREE: DocumentError =
            DocumentError::Structure("the page tree is not made of indirect objects");
        // Flatten the tree into one node first; a document without pages
        // has nothing to flatten.
        let all: Vec<usize> = (0..self.pages.len()).collect();
        if !all.is_empty() {
            self.select_pages(&all)?;
        }
        let catalog = self.reader.resolve_entry(&self.trailer, "Root")?;
        let Some(PdfObject::Reference(root)) = catalog
            .as_dictionary()
            .and_then(|catalog| reader::entry(catalog, "Pages"))
            .cloned()
        else {
            return Err(TREE);
        };
        let PdfObject::Dictionary(node) = self.object(root)? else {
            return Err(TREE);
        };
        match reader::entry(&node, "Kids") {
            Some(PdfObject::Array(kids)) => Ok((root, kids.clone())),
            _ => Err(TREE),
        }
    }

    /// Replace the list of pages of the page tree's one node.
    fn set_page_list(&mut self, root: ObjectReference, kids: Vec<PdfObject>) {
        let mut node = Dictionary::new();
        node.insert(b"Type".to_vec(), PdfObject::Name(b"Pages".to_vec()));
        let count = PdfObject::Integer(i64::try_from(kids.len()).unwrap_or(0));
        node.insert(b"Count".to_vec(), count);
        node.insert(b"Kids".to_vec(), PdfObject::Array(kids));
        self.set_object(root, PdfObject::Dictionary(node));
    }

    /// Insert a blank page before the page with the given zero-based
    /// index, or after the last page when the index is the page count. The
    /// size is in points.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentError::Structure`] when the index is beyond the
    /// page count, the size is not positive, or the page tree is not made
    /// of indirect objects, and a [`DocumentError`] when a page cannot be
    /// parsed.
    pub fn insert_blank_page(
        &mut self,
        page_index: usize,
        width: f64,
        height: f64,
    ) -> Result<(), DocumentError> {
        if page_index > self.pages.len() {
            return Err(DocumentError::Structure("a page index is out of range"));
        }
        let usable = |side: f64| side.is_finite() && side > 0.0;
        if !usable(width) || !usable(height) {
            return Err(DocumentError::Structure("a page size is positive"));
        }
        let (root, mut kids) = self.page_list()?;
        let mut page = Dictionary::new();
        page.insert(b"Type".to_vec(), PdfObject::Name(b"Page".to_vec()));
        page.insert(b"Parent".to_vec(), PdfObject::Reference(root));
        let corners = [0.0, 0.0, width, height].map(PdfObject::Real);
        page.insert(b"MediaBox".to_vec(), PdfObject::Array(corners.to_vec()));
        page.insert(
            b"Resources".to_vec(),
            PdfObject::Dictionary(Dictionary::new()),
        );
        let page = self.add_object(PdfObject::Dictionary(page));
        kids.insert(page_index.min(kids.len()), PdfObject::Reference(page));
        self.set_page_list(root, kids);
        Ok(())
    }

    /// Break a page in two at a height, as a page break does in a word
    /// processor: what lies below the height moves to the top of a new
    /// page that follows, and the original page keeps what lies above.
    ///
    /// `height` is measured in points from the bottom of the unrotated
    /// page, as glyph positions are, and must lie inside the page's crop
    /// box. Nothing on the page is rewritten: both pages show the same
    /// content, each clipped to its part, so the break is a straight line
    /// and what it crosses is cut in two. Links and other annotations
    /// whose middle lies below the break move to the new page. Outline
    /// items and links elsewhere that lead to the moved part still lead to
    /// the original page.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentError::Structure`] when the index is out of range,
    /// the height is not inside the page, or the page tree is not made of
    /// indirect objects, and a [`DocumentError`] when the page cannot be
    /// parsed.
    pub fn break_page(&mut self, page_index: usize, height: f64) -> Result<(), DocumentError> {
        const INDIRECT: DocumentError =
            DocumentError::Structure("the page is not an indirect object");
        let Some(area) = self.pages.get(page_index).map(|page| page.crop_box) else {
            return Err(DocumentError::Structure("a page index is out of range"));
        };
        if !(height > area.bottom && height < area.top) {
            return Err(DocumentError::Structure("the break is not inside the page"));
        }
        let (root, mut kids) = self.page_list()?;
        let Some(&PdfObject::Reference(upper)) = kids.get(page_index) else {
            return Err(INDIRECT);
        };
        let PdfObject::Dictionary(mut page) = self.object(upper)? else {
            return Err(INDIRECT);
        };

        // The page's content streams, in order.
        let streams: Vec<PdfObject> = match reader::entry(&page, "Contents") {
            None => Vec::new(),
            Some(contents) => match self.reader.resolve(contents)? {
                PdfObject::Array(streams) => streams,
                _ => vec![contents.clone()],
            },
        };
        // Each part draws all the content inside a clip; the lower part is
        // first moved up so that the break lands at the top of the page.
        let rise = area.top - height;
        let (left, width) = (area.left, area.width());
        let upper_clip = format!("q {left} {height} {width} {rise} re W n\n");
        let lower_clip = format!(
            "q 1 0 0 1 0 {rise} cm {left} {} {width} {} re W n\n",
            area.bottom,
            height - area.bottom
        );
        let part = |document: &mut Self, opening: String| {
            let mut contents = Vec::new();
            for wrapper in [opening.into_bytes(), b"\nQ\n".to_vec()] {
                let stream = document.add_object(PdfObject::Null);
                document.reader.set_object(
                    stream.number,
                    PdfObject::Dictionary(Dictionary::new()),
                    Some(wrapper),
                );
                contents.push(PdfObject::Reference(stream));
            }
            contents.splice(1..1, streams.iter().cloned());
            PdfObject::Array(contents)
        };
        let upper_contents = part(self, upper_clip);
        let lower_contents = part(self, lower_clip);
        let lower = self.add_object(PdfObject::Null);

        // Annotations go with the part their middle lies in.
        let mut stay = Vec::new();
        let mut moved = Vec::new();
        if let PdfObject::Array(annotations) = self.reader.resolve_entry(&page, "Annots")? {
            for annotation in annotations {
                let PdfObject::Dictionary(mut entries) = self.reader.resolve(&annotation)? else {
                    stay.push(annotation);
                    continue;
                };
                let numbers = |value: PdfObject| -> Vec<f64> {
                    let values = value.as_array().unwrap_or_default();
                    values.iter().filter_map(PdfObject::as_number).collect()
                };
                let rectangle = numbers(self.reader.resolve_entry(&entries, "Rect")?);
                let below = rectangle.len() == 4 && (rectangle[1] + rectangle[3]) / 2.0 < height;
                if !below {
                    stay.push(annotation);
                    continue;
                }
                // Every second number of these entries is a height.
                for key in ["Rect", "QuadPoints"] {
                    let raised: Vec<PdfObject> = numbers(self.reader.resolve_entry(&entries, key)?)
                        .iter()
                        .enumerate()
                        .map(|(index, &value)| {
                            PdfObject::Real(if index % 2 == 1 { value + rise } else { value })
                        })
                        .collect();
                    if !raised.is_empty() {
                        entries.insert(key.as_bytes().to_vec(), PdfObject::Array(raised));
                    }
                }
                if entries.contains_key(b"P".as_slice()) {
                    entries.insert(b"P".to_vec(), PdfObject::Reference(lower));
                }
                let entries = PdfObject::Dictionary(entries);
                match annotation {
                    PdfObject::Reference(reference) => {
                        self.reader.set_object(reference.number, entries, None);
                        moved.push(annotation);
                    }
                    _ => moved.push(entries),
                }
            }
        }

        let mut second = page.clone();
        for (entries, contents, annotations) in [
            (&mut page, upper_contents, stay),
            (&mut second, lower_contents, moved),
        ] {
            entries.insert(b"Contents".to_vec(), contents);
            if annotations.is_empty() {
                entries.remove(b"Annots".as_slice());
            } else {
                entries.insert(b"Annots".to_vec(), PdfObject::Array(annotations));
            }
        }
        self.reader
            .set_object(upper.number, PdfObject::Dictionary(page), None);
        self.reader
            .set_object(lower.number, PdfObject::Dictionary(second), None);
        kids.insert(page_index + 1, PdfObject::Reference(lower));
        self.set_page_list(root, kids);
        Ok(())
    }

    /// Keep only the pages with the given zero-based indexes, in the given
    /// order: this reorders, removes, and repeats pages.
    ///
    /// The page tree becomes one node that lists the pages, and each page
    /// takes on the attributes it inherited.
    ///
    /// When pages are left out, what leads to them goes too, so that saving
    /// drops them from the file: links on the remaining pages, named
    /// destinations, and outline items. The outline is written again as
    /// [`Document::set_outline`] writes it. The structure tree of a tagged
    /// document and its article threads are removed whole, since they
    /// describe the document page by page. A form field on a removed page
    /// can still hold that page in the file.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentError::Structure`] when an index is out of range,
    /// no page is selected, or the page tree is not made of indirect
    /// objects, and a [`DocumentError`] when a page cannot be parsed.
    pub fn select_pages(&mut self, indexes: &[usize]) -> Result<(), DocumentError> {
        const INDIRECT: DocumentError =
            DocumentError::Structure("the page tree is not made of indirect objects");
        if indexes.is_empty() {
            return Err(DocumentError::Structure(
                "a document needs at least one page",
            ));
        }
        let catalog = self.reader.resolve_entry(&self.trailer, "Root")?;
        let Some(PdfObject::Reference(root)) = catalog
            .as_dictionary()
            .and_then(|catalog| reader::entry(catalog, "Pages"))
            .cloned()
        else {
            return Err(INDIRECT);
        };
        // The pages taken out, and the outline as it is before they go.
        let removed: std::collections::BTreeSet<u32> = self
            .pages
            .iter()
            .enumerate()
            .filter(|(index, _)| !indexes.contains(index))
            .filter_map(|(_, page)| Some(page.reference?.number))
            .collect();
        let outline = if removed.is_empty() {
            Vec::new()
        } else {
            self.outline()?
        };
        let mut selected = Vec::new();
        for &index in indexes {
            let page = self.pages.get(index);
            let page = page.ok_or(DocumentError::Structure("a page index is out of range"))?;
            let reference = page.reference.ok_or(INDIRECT)?;
            let PdfObject::Dictionary(mut page) =
                self.reader.resolve(&PdfObject::Reference(reference))?
            else {
                return Err(INDIRECT);
            };
            for key in ["Resources", "MediaBox", "CropBox", "Rotate"] {
                if !page.contains_key(key.as_bytes())
                    && let Some(value) = self.inherited(&page, key)?
                {
                    page.insert(key.as_bytes().to_vec(), value);
                }
            }
            page.insert(b"Parent".to_vec(), PdfObject::Reference(root));
            selected.push((reference, page));
        }
        let mut kids = Vec::new();
        let mut used = Vec::new();
        for (reference, page) in selected {
            let page = PdfObject::Dictionary(page);
            // A page selected again becomes a copy under a new number.
            let reference = if used.contains(&reference.number) {
                self.add_object(page)
            } else {
                used.push(reference.number);
                self.reader.set_object(reference.number, page, None);
                reference
            };
            kids.push(PdfObject::Reference(reference));
        }
        let mut node = Dictionary::new();
        node.insert(b"Type".to_vec(), PdfObject::Name(b"Pages".to_vec()));
        let count = PdfObject::Integer(i64::try_from(kids.len()).unwrap_or(0));
        node.insert(b"Count".to_vec(), count);
        node.insert(b"Kids".to_vec(), PdfObject::Array(kids));
        self.reader
            .set_object(root.number, PdfObject::Dictionary(node), None);
        self.refresh_pages();
        if removed.is_empty() {
            return Ok(());
        }
        let kept: Vec<ObjectReference> = self
            .pages
            .iter()
            .filter_map(|page| page.reference)
            .collect();
        prune::prune(&mut self.reader, &self.trailer, &kept, &removed)?;
        if outline.is_empty() {
            return Ok(());
        }
        // The outline is written again for the pages that remain. An item
        // for a removed page stays, leading nowhere, only when an item
        // under it is kept.
        let moved = |item: &OutlineItem| match item.page {
            Some(page) => indexes.iter().position(|&index| index == page).map(Some),
            None => Some(None),
        };
        let mut items = Vec::new();
        for (position, item) in outline.iter().enumerate() {
            let under = outline[position + 1..]
                .iter()
                .take_while(|later| later.level > item.level);
            let page = match moved(item) {
                Some(page) => page,
                None if under.clone().any(|later| moved(later).is_some()) => None,
                None => continue,
            };
            items.push(OutlineItem {
                level: item.level,
                title: item.title.clone(),
                page,
                view: item.view.filter(|_| page.is_some()),
            });
        }
        self.set_outline(&items)
    }

    /// The value a page inherits for a key from the nearest ancestor that
    /// has it.
    fn inherited(
        &mut self,
        page: &Dictionary,
        key: &str,
    ) -> Result<Option<PdfObject>, DocumentError> {
        let mut node = self.reader.resolve_entry(page, "Parent")?;
        // Deeper trees than this are taken to be cyclic.
        for _ in 0..32 {
            let Some(dictionary) = node.as_dictionary() else {
                break;
            };
            if let Some(value) = reader::entry(dictionary, key) {
                return Ok(Some(value.clone()));
            }
            node = self.reader.resolve_entry(dictionary, "Parent")?;
        }
        Ok(None)
    }

    /// Read the page list again after a change that may affect it; a tree
    /// that can no longer be read keeps the list as it was.
    fn refresh_pages(&mut self) {
        if let Ok(pages) = pages::collect(&mut self.reader, &self.trailer) {
            self.pages = pages;
        }
    }
}

/// The entries of the document information dictionary, as text.
///
/// Dates keep the form the file stores, such as `D:20240131120000+01'00'`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Information {
    /// The document's title.
    pub title: Option<String>,
    /// The person who created the document.
    pub author: Option<String>,
    /// The document's subject.
    pub subject: Option<String>,
    /// Keywords associated with the document.
    pub keywords: Option<String>,
    /// The application that created the original document.
    pub creator: Option<String>,
    /// The application that produced the PDF.
    pub producer: Option<String>,
    /// When the document was created.
    pub creation_date: Option<String>,
    /// When the document was last modified.
    pub modification_date: Option<String>,
}
