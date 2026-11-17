//! Python bindings for the Plain Open PDF engine, as the `plain_open_pdf_engine` module.

use plain_open_pdf_engine as engine;
use pyo3::exceptions::{PyException, PyIndexError, PyOSError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

pyo3::create_exception!(
    plain_open_pdf_engine,
    PdfError,
    PyException,
    "A PDF could not be opened or read."
);
pyo3::create_exception!(
    plain_open_pdf_engine,
    PasswordRequired,
    PdfError,
    "The PDF is encrypted and the password is missing or wrong."
);
pyo3::create_exception!(
    plain_open_pdf_engine,
    WrongPassword,
    PasswordRequired,
    "The PDF is encrypted and the password given is wrong."
);

fn to_python_error(error: engine::DocumentError) -> PyErr {
    match error {
        engine::DocumentError::PasswordRequired => PasswordRequired::new_err(error.to_string()),
        engine::DocumentError::WrongPassword => WrongPassword::new_err(error.to_string()),
        _ => PdfError::new_err(error.to_string()),
    }
}

/// Refuse a pixel mode that rendered pages are not given in.
fn check_mode(mode: &str) -> PyResult<()> {
    if matches!(mode, "RGBA" | "RGB" | "L") {
        return Ok(());
    }
    let message = format!("mode is \"RGBA\", \"RGB\", or \"L\", not {mode:?}");
    Err(PyValueError::new_err(message))
}

/// A rendered image as the tuple (width, height, pixels) that the
/// rendering methods return, with its pixels in a mode `check_mode` allows.
fn pixels<'py>(
    py: Python<'py>,
    image: &engine::Pixmap,
    mode: &str,
) -> (usize, usize, Bound<'py, PyBytes>) {
    let pixels = match mode {
        "RGB" => PyBytes::new(py, &image.rgb()),
        "L" => PyBytes::new(py, &image.gray()),
        _ => PyBytes::new(py, image.data()),
    };
    (image.width(), image.height(), pixels)
}

/// The standard font a name gives: one of the 14 PostScript names, a
/// common name for the same design such as "Arial,Bold", or one of
/// PyMuPDF's short names such as "helv" or "tiro".
fn standard_font(name: &str) -> PyResult<engine::StandardFont> {
    use engine::StandardFont as Font;
    let short = match name.to_ascii_lowercase().as_str() {
        "helv" | "helvetica" => Some(Font::Helvetica),
        "heit" => Some(Font::HelveticaOblique),
        "hebo" => Some(Font::HelveticaBold),
        "hebi" => Some(Font::HelveticaBoldOblique),
        "tiro" | "times" | "times-roman" => Some(Font::TimesRoman),
        "tiit" => Some(Font::TimesItalic),
        "tibo" => Some(Font::TimesBold),
        "tibi" => Some(Font::TimesBoldItalic),
        "cour" | "courier" => Some(Font::Courier),
        "coit" => Some(Font::CourierOblique),
        "cobo" => Some(Font::CourierBold),
        "cobi" => Some(Font::CourierBoldOblique),
        "symb" | "symbol" => Some(Font::Symbol),
        "zadb" | "zapfdingbats" => Some(Font::ZapfDingbats),
        _ => None,
    };
    short
        .or_else(|| Font::from_name(name))
        .ok_or_else(|| PyValueError::new_err(format!("not a standard font: {name:?}")))
}

/// The width in points of text shown in a standard 14 font at a size, as
/// `Document.insert_text` shows it: a character the font's encoding lacks
/// counts as the question mark it is shown as.
#[pyfunction]
#[pyo3(signature = (text, font="Helvetica", size=11.0))]
fn text_width(text: &str, font: &str, size: f64) -> PyResult<f64> {
    Ok(standard_font(font)?.text_width(text, size))
}

/// The names of the permissions an encrypted document can grant, in the
/// order `permission_flags` gives them.
const PERMISSION_NAMES: [&str; 8] = [
    "print",
    "modify",
    "copy",
    "annotate",
    "fill_forms",
    "extract_for_accessibility",
    "assemble",
    "print_high_quality",
];

fn permission_flags(permissions: engine::Permissions) -> [bool; 8] {
    [
        permissions.print,
        permissions.modify,
        permissions.copy,
        permissions.annotate,
        permissions.fill_forms,
        permissions.extract_for_accessibility,
        permissions.assemble,
        permissions.print_high_quality,
    ]
}

/// A rectangle as (left, bottom, right, top) in points, origin bottom-left.
type Rectangle = (f64, f64, f64, f64);

fn rectangle(rectangle: engine::Rectangle) -> Rectangle {
    (
        rectangle.left,
        rectangle.bottom,
        rectangle.right,
        rectangle.top,
    )
}

/// How a viewer shows the page a link or outline item leads to.
///
/// `fit` is "XYZ", "Fit", "FitH", "FitV", "FitR", "FitB", "FitBH", or
/// "FitBV" (ISO 32000-2 section 12.3.2.2). `left` and `top` give the point
/// to put at the window's top left corner and `zoom` the magnification,
/// where 1 is 100 percent; each is None when the view keeps the viewer's
/// value. `area` is the rectangle a "FitR" view fits, as (left, bottom,
/// right, top). Coordinates are in points from the bottom left of the
/// unrotated page.
#[pyclass(frozen, get_all, from_py_object)]
#[derive(Clone)]
struct View {
    /// The kind of view, such as "XYZ" or "Fit".
    fit: &'static str,
    /// Horizontal coordinate of the window's left edge, or None.
    left: Option<f64>,
    /// Vertical coordinate of the window's top edge, or None.
    top: Option<f64>,
    /// Magnification of an "XYZ" view, or None.
    zoom: Option<f64>,
    /// The rectangle of a "FitR" view, or None.
    area: Option<Rectangle>,
}

impl From<engine::View> for View {
    fn from(view: engine::View) -> Self {
        let (left, top) = view.point();
        let area = match view {
            engine::View::FitR(area) => Some(rectangle(area)),
            _ => None,
        };
        Self {
            fit: view.name(),
            left,
            top,
            zoom: view.zoom(),
            area,
        }
    }
}

impl View {
    /// The engine's form of the view; the constructor has checked it.
    fn to_engine(&self) -> engine::View {
        let (left, top, zoom) = (self.left, self.top, self.zoom);
        match (self.fit, self.area) {
            ("XYZ", _) => engine::View::Xyz { left, top, zoom },
            ("FitH", _) => engine::View::FitH { top },
            ("FitV", _) => engine::View::FitV { left },
            ("FitR", Some((left, bottom, right, top))) => engine::View::FitR(engine::Rectangle {
                left: left.min(right),
                bottom: bottom.min(top),
                right: left.max(right),
                top: bottom.max(top),
            }),
            ("FitB", _) => engine::View::FitB,
            ("FitBH", _) => engine::View::FitBH { top },
            ("FitBV", _) => engine::View::FitBV { left },
            _ => engine::View::Fit,
        }
    }
}

#[pymethods]
impl View {
    /// A view of the given kind. A "FitR" view needs its `area`; the
    /// values a kind does not use are ignored.
    #[new]
    #[pyo3(signature = (fit, left=None, top=None, zoom=None, area=None))]
    fn new(
        fit: &str,
        left: Option<f64>,
        top: Option<f64>,
        zoom: Option<f64>,
        area: Option<Rectangle>,
    ) -> PyResult<Self> {
        const KINDS: [&str; 8] = [
            "XYZ", "Fit", "FitH", "FitV", "FitR", "FitB", "FitBH", "FitBV",
        ];
        let Some(&fit) = KINDS.iter().find(|&&kind| kind == fit) else {
            return Err(PyValueError::new_err(format!("unknown view {fit:?}")));
        };
        if fit == "FitR" && area.is_none() {
            return Err(PyValueError::new_err("a FitR view needs an area"));
        }
        let given = Self {
            fit,
            left,
            top,
            zoom: zoom.filter(|&zoom| zoom > 0.0),
            area,
        };
        Ok(given.to_engine().into())
    }

    fn __eq__(&self, other: &Self) -> bool {
        self.to_engine() == other.to_engine()
    }

    fn __repr__(&self) -> String {
        match self.area {
            Some(area) => format!("View(fit={:?}, area={area:?})", self.fit),
            None => format!(
                "View(fit={:?}, left={:?}, top={:?}, zoom={:?})",
                self.fit, self.left, self.top, self.zoom
            ),
        }
    }
}

/// One page's geometry, before rotation is applied.
#[pyclass(frozen, get_all)]
#[derive(Clone)]
struct Page {
    /// The physical medium as (left, bottom, right, top) in points.
    media_box: Rectangle,
    /// The visible region as (left, bottom, right, top) in points.
    crop_box: Rectangle,
    /// Clockwise display rotation in degrees: 0, 90, 180, or 270.
    rotation: u16,
}

#[pymethods]
impl Page {
    fn __repr__(&self) -> String {
        format!(
            "Page(media_box={:?}, crop_box={:?}, rotation={})",
            self.media_box, self.crop_box, self.rotation
        )
    }
}

/// One entry of the document outline (bookmarks).
#[pyclass(frozen, get_all, from_py_object)]
#[derive(Clone)]
struct OutlineItem {
    /// Nesting depth; top-level items are level 1.
    level: usize,
    /// The item's title.
    title: String,
    /// Zero-based index of the page the item leads to, or None.
    page: Option<usize>,
    /// How the page is shown, or None when the item gives no view.
    view: Option<View>,
}

/// An outline item to write: an `OutlineItem`, or a (level, title, page)
/// or (level, title, page, view) tuple.
#[derive(FromPyObject)]
enum NewOutlineItem {
    Item(OutlineItem),
    WithView((usize, String, Option<usize>, Option<View>)),
    Plain((usize, String, Option<usize>)),
}

impl From<NewOutlineItem> for engine::OutlineItem {
    fn from(item: NewOutlineItem) -> Self {
        let (level, title, page, view) = match item {
            NewOutlineItem::Item(item) => (item.level, item.title, item.page, item.view),
            NewOutlineItem::WithView(item) => item,
            NewOutlineItem::Plain((level, title, page)) => (level, title, page, None),
        };
        let view = view.map(|view| view.to_engine());
        Self {
            level,
            title,
            page,
            view,
        }
    }
}

#[pymethods]
impl OutlineItem {
    fn __repr__(&self) -> String {
        format!(
            "OutlineItem(level={}, title={:?}, page={:?})",
            self.level, self.title, self.page
        )
    }
}

/// A clickable region of a page and where it leads.
///
/// `kind` is "page", "uri", "remote", "launch", or "named". A "page" link
/// sets `page`; "uri" sets `uri`; "remote" sets `file` and perhaps `page`,
/// the page in that file; "launch" sets `file`; "named" sets `name`.
/// "page" and "remote" links set `view` when their destination gives one.
#[pyclass(frozen, get_all)]
#[derive(Clone)]
struct Link {
    /// The region as (left, bottom, right, top) in points.
    rectangle: Rectangle,
    /// What kind of target the link has.
    kind: &'static str,
    /// Zero-based page index, for "page" and some "remote" links.
    page: Option<usize>,
    /// The address of a "uri" link.
    uri: Option<String>,
    /// The file of a "remote" or "launch" link.
    file: Option<String>,
    /// The viewer operation of a "named" link, such as "NextPage".
    name: Option<String>,
    /// How the target page is shown, or None.
    view: Option<View>,
}

impl From<engine::Link> for Link {
    fn from(link: engine::Link) -> Self {
        let mut result = Self {
            rectangle: rectangle(link.rectangle),
            kind: "page",
            page: None,
            uri: None,
            file: None,
            name: None,
            view: link.view.map(View::from),
        };
        match link.target {
            engine::LinkTarget::Page(page) => result.page = Some(page),
            engine::LinkTarget::Uri(uri) => {
                result.kind = "uri";
                result.uri = Some(uri);
            }
            engine::LinkTarget::RemoteDocument { file, page } => {
                result.kind = "remote";
                result.file = Some(file);
                result.page = page;
            }
            engine::LinkTarget::Launch(file) => {
                result.kind = "launch";
                result.file = Some(file);
            }
            engine::LinkTarget::Named(name) => {
                result.kind = "named";
                result.name = Some(name);
            }
        }
        result
    }
}

#[pymethods]
impl Link {
    fn __repr__(&self) -> String {
        let target = [&self.uri, &self.file, &self.name];
        let target = target.into_iter().flatten().next();
        format!(
            "Link(kind={:?}, page={:?}, target={:?}, rectangle={:?})",
            self.kind, self.page, target, self.rectangle
        )
    }
}

/// A file embedded in the document.
#[pyclass(frozen, get_all)]
#[derive(Clone)]
struct Attachment {
    /// The key the document lists the file under.
    name: String,
    /// The file's own name, or None.
    file_name: Option<String>,
    /// A description of the file, or None.
    description: Option<String>,
    /// Object number of the stream holding the file's bytes, or None; pass
    /// it to `Document.stream`.
    stream: Option<u32>,
}

#[pymethods]
impl Attachment {
    fn __repr__(&self) -> String {
        format!(
            "Attachment(name={:?}, file_name={:?})",
            self.name, self.file_name
        )
    }
}

/// An opened PDF document.
///
/// Create one from bytes with `Document(data, password=None)` or from a file
/// with `Document.open(path, password=None)`. A document may be used from
/// several threads; calls on one document run one at a time.
#[pyclass(frozen)]
struct Document {
    inner: Mutex<engine::Document>,
}

/// A file `read_shared` has read, and its bytes while a document still
/// holds them.
type SharedFile = (PathBuf, Weak<Vec<u8>>);

/// The files documents now open were read from.
static SHARED_FILES: Mutex<Vec<SharedFile>> = Mutex::new(Vec::new());

/// Read a file, or share the bytes an open document already holds for it
/// when the file still holds exactly those bytes.
///
/// Sizes and times of change cannot tell every change apart (some file
/// systems keep times to two seconds), so the file is compared with the
/// held bytes, a piece at a time, without keeping a second copy. A file no
/// document holds is read with the list locked, so that threads opening it
/// at the same moment read it once; comparisons run unlocked, side by side.
fn read_shared(path: &Path) -> std::io::Result<Arc<Vec<u8>>> {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let held: Vec<Arc<Vec<u8>>> = {
        let mut files = SHARED_FILES.lock().unwrap_or_else(PoisonError::into_inner);
        files.retain(|(_, bytes)| bytes.strong_count() > 0);
        let held: Vec<Arc<Vec<u8>>> = files
            .iter()
            .filter(|(known, _)| *known == path)
            .filter_map(|(_, bytes)| bytes.upgrade())
            .collect();
        if held.is_empty() {
            let bytes = Arc::new(std::fs::read(&path)?);
            files.push((path, Arc::downgrade(&bytes)));
            return Ok(bytes);
        }
        held
    };
    for bytes in held {
        if same_contents(&path, &bytes)? {
            return Ok(bytes);
        }
    }
    // The file changed since the held bytes were read.
    let bytes = Arc::new(std::fs::read(&path)?);
    let mut files = SHARED_FILES.lock().unwrap_or_else(PoisonError::into_inner);
    files.push((path, Arc::downgrade(&bytes)));
    Ok(bytes)
}

/// Whether the file at `path` holds exactly `bytes`.
fn same_contents(path: &Path, bytes: &[u8]) -> std::io::Result<bool> {
    let mut file = std::fs::File::open(path)?;
    if file.metadata()?.len() != bytes.len() as u64 {
        return Ok(false);
    }
    let mut buffer = vec![0; 1 << 20];
    let mut offset = 0;
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            return Ok(offset == bytes.len());
        }
        if bytes.get(offset..offset + count) != Some(&buffer[..count]) {
            return Ok(false);
        }
        offset += count;
    }
}

impl Document {
    fn lock(&self) -> MutexGuard<'_, engine::Document> {
        // The engine holds no invariant that a panic could leave broken.
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn from_bytes(py: Python<'_>, data: Vec<u8>, password: Option<&str>) -> PyResult<Self> {
        let password = password.unwrap_or_default().as_bytes();
        // Parsing does not touch Python objects, so other threads may run.
        let document = py
            .detach(|| engine::Document::open_with_password(data, password))
            .map_err(to_python_error)?;
        Ok(Self {
            inner: Mutex::new(document),
        })
    }
}

#[pymethods]
impl Document {
    /// Open a document from its bytes.
    #[new]
    #[pyo3(signature = (data, password=None))]
    fn new(py: Python<'_>, data: &[u8], password: Option<&str>) -> PyResult<Self> {
        Self::from_bytes(py, data.to_vec(), password)
    }

    /// A new document without pages, to add pages to with
    /// `insert_blank_page`.
    #[staticmethod]
    #[pyo3(name = "new")]
    fn empty() -> Self {
        Self {
            inner: Mutex::new(engine::Document::new()),
        }
    }

    /// Open the document stored in a file.
    ///
    /// Documents open at the same time from the same unchanged file share
    /// one copy of its bytes, so that a handle for each thread does not
    /// cost a copy of the file each.
    #[staticmethod]
    #[pyo3(signature = (path, password=None))]
    fn open(py: Python<'_>, path: PathBuf, password: Option<&str>) -> PyResult<Self> {
        let password = password.unwrap_or_default().as_bytes();
        // Reading and parsing do not touch Python objects.
        let document = py.detach(|| {
            let bytes = read_shared(&path)
                .map_err(|error| PyOSError::new_err(format!("{}: {error}", path.display())))?;
            engine::Document::open_shared(bytes, password).map_err(to_python_error)
        })?;
        Ok(Self {
            inner: Mutex::new(document),
        })
    }

    /// One page's geometry, by zero-based index.
    fn page(&self, page_index: usize) -> PyResult<Page> {
        let document = self.lock();
        let page = document
            .pages()
            .get(page_index)
            .ok_or_else(|| PyIndexError::new_err(format!("no page {page_index}")))?;
        Ok(Page {
            media_box: rectangle(page.media_box),
            crop_box: rectangle(page.crop_box),
            rotation: page.rotation,
        })
    }

    /// The number of pages.
    #[getter]
    fn page_count(&self) -> usize {
        self.lock().page_count()
    }

    fn __len__(&self) -> usize {
        self.lock().page_count()
    }

    /// The PDF version declared in the file header, such as "1.7".
    #[getter]
    fn version(&self) -> String {
        self.lock().version().to_string()
    }

    /// The pages in document order.
    #[getter]
    fn pages(&self) -> Vec<Page> {
        let pages = self.lock().pages().to_vec();
        pages
            .into_iter()
            .map(|page| Page {
                media_box: rectangle(page.media_box),
                crop_box: rectangle(page.crop_box),
                rotation: page.rotation,
            })
            .collect()
    }

    /// The document information entries that are present, by name: title,
    /// author, subject, keywords, creator, producer, creation_date, and
    /// modification_date.
    fn information(&self) -> PyResult<BTreeMap<&'static str, String>> {
        let information = self.lock().information().map_err(to_python_error)?;
        let entries = [
            ("title", information.title),
            ("author", information.author),
            ("subject", information.subject),
            ("keywords", information.keywords),
            ("creator", information.creator),
            ("producer", information.producer),
            ("creation_date", information.creation_date),
            ("modification_date", information.modification_date),
        ];
        Ok(entries
            .into_iter()
            .filter_map(|(name, value)| Some((name, value?)))
            .collect())
    }

    /// Every text entry of the information dictionary, custom ones
    /// included, by its key as the file spells it, such as "Title".
    fn information_entries(&self) -> PyResult<BTreeMap<String, String>> {
        let entries = self.lock().information_entries().map_err(to_python_error)?;
        Ok(entries.into_iter().collect())
    }

    /// Add a link to a page over a (left, bottom, right, top) rectangle.
    /// `kind` is "page" (with `page`, a zero-based index), "uri" (with
    /// `uri`), "remote" (with `file`, and `page` in that file), "launch"
    /// (with `file`), or "named" (with `name`, such as "NextPage"). `view`
    /// sets how a target page is shown.
    #[pyo3(signature = (page_index, rectangle, kind, page=None, uri=None, file=None, name=None, view=None))]
    #[allow(clippy::too_many_arguments)]
    fn add_link(
        &self,
        page_index: usize,
        rectangle: Rectangle,
        kind: &str,
        page: Option<usize>,
        uri: Option<String>,
        file: Option<String>,
        name: Option<String>,
        view: Option<View>,
    ) -> PyResult<()> {
        let missing = |what: &str| PyValueError::new_err(format!("a {kind:?} link needs {what}"));
        let target = match kind {
            "page" => engine::LinkTarget::Page(page.ok_or_else(|| missing("a page"))?),
            "uri" => engine::LinkTarget::Uri(uri.ok_or_else(|| missing("a uri"))?),
            "remote" => engine::LinkTarget::RemoteDocument {
                file: file.ok_or_else(|| missing("a file"))?,
                page,
            },
            "launch" => engine::LinkTarget::Launch(file.ok_or_else(|| missing("a file"))?),
            "named" => engine::LinkTarget::Named(name.ok_or_else(|| missing("a name"))?),
            _ => return Err(PyValueError::new_err(format!("unknown link kind {kind:?}"))),
        };
        let (left, bottom, right, top) = rectangle;
        let rectangle = engine::Rectangle {
            left,
            bottom,
            right,
            top,
        };
        let view = view.map(|view| view.to_engine());
        let result = self.lock().add_link(page_index, rectangle, &target, view);
        result.map_err(to_python_error)
    }

    /// The outline (bookmarks) in reading order.
    fn outline(&self) -> PyResult<Vec<OutlineItem>> {
        let items = self.lock().outline().map_err(to_python_error)?;
        Ok(items
            .into_iter()
            .map(|item| OutlineItem {
                level: item.level,
                title: item.title,
                page: item.page,
                view: item.view.map(View::from),
            })
            .collect())
    }

    /// The links on the page with the given zero-based index.
    fn links(&self, page_index: usize) -> PyResult<Vec<Link>> {
        let links = self.lock().links(page_index).map_err(to_python_error)?;
        Ok(links.into_iter().map(Link::from).collect())
    }

    /// The page a named destination leads to and the view of it, as
    /// (page, view) with view None when the destination gives none; None
    /// when no destination has the name.
    fn named_destination(&self, name: &str) -> PyResult<Option<(usize, Option<View>)>> {
        let found = self.lock().named_destination(name.as_bytes());
        let found = found.map_err(to_python_error)?;
        Ok(found.map(|(page, view)| (page, view.map(View::from))))
    }

    /// The files embedded in the document.
    fn attachments(&self) -> PyResult<Vec<Attachment>> {
        let attachments = self.lock().attachments().map_err(to_python_error)?;
        Ok(attachments
            .into_iter()
            .map(|attachment| Attachment {
                name: attachment.name,
                file_name: attachment.file_name,
                description: attachment.description,
                stream: attachment.stream.map(|stream| stream.number),
            })
            .collect())
    }

    /// The text the page with the given zero-based index shows, in content
    /// order, with spaces and line breaks taken from the glyph positions.
    fn page_text(&self, page_index: usize) -> PyResult<String> {
        self.lock().page_text(page_index).map_err(to_python_error)
    }

    /// The glyphs the page shows, in content order, each as a tuple
    /// (text, x, y, end_x, end_y, size): the glyph's origin on the baseline,
    /// where the next glyph's origin would fall, and the font size, in
    /// points from the bottom left of the unrotated page.
    #[allow(clippy::type_complexity)]
    fn page_glyphs(&self, page_index: usize) -> PyResult<Vec<(String, f64, f64, f64, f64, f64)>> {
        let glyphs = self.lock().page_glyphs(page_index);
        Ok(glyphs
            .map_err(to_python_error)?
            .into_iter()
            .map(|glyph| {
                let end = (glyph.end_x, glyph.end_y);
                (glyph.text, glyph.x, glyph.y, end.0, end.1, glyph.size)
            })
            .collect())
    }

    /// The words of the page, each as a tuple (left, bottom, right, top,
    /// text, block, line, word), in points from the bottom left of the
    /// unrotated page.
    #[allow(clippy::type_complexity)]
    fn page_words(
        &self,
        page_index: usize,
    ) -> PyResult<Vec<(f64, f64, f64, f64, String, usize, usize, usize)>> {
        let words = self.lock().page_words(page_index);
        Ok(words
            .map_err(to_python_error)?
            .into_iter()
            .map(|word| {
                let area = word.bounds;
                let numbers = (word.block, word.line, word.word);
                (
                    area.left,
                    area.bottom,
                    area.right,
                    area.top,
                    word.text,
                    numbers.0,
                    numbers.1,
                    numbers.2,
                )
            })
            .collect())
    }

    /// The runs of text of the page that share a line, a font, a size, and
    /// a color, each as a tuple (text, (left, bottom, right, top),
    /// (origin_x, origin_y), font, size, (red, green, blue)).
    #[allow(clippy::type_complexity)]
    fn page_spans(
        &self,
        page_index: usize,
    ) -> PyResult<Vec<(String, Rectangle, (f64, f64), String, f64, (u8, u8, u8))>> {
        let spans = self.lock().page_spans(page_index);
        Ok(spans
            .map_err(to_python_error)?
            .into_iter()
            .map(|span| {
                let color = (span.color[0], span.color[1], span.color[2]);
                let font = span.font.to_string();
                let origin = (span.x, span.y);
                (
                    span.text,
                    rectangle(span.bounds),
                    origin,
                    font,
                    span.size,
                    color,
                )
            })
            .collect())
    }

    /// Where a point of a page lies as the page is displayed: in points
    /// from the top left of the displayed page, with the vertical axis
    /// running down. Returns None for a page index beyond the last page.
    fn to_view(&self, page_index: usize, x: f64, y: f64) -> Option<(f64, f64)> {
        let page = *self.lock().pages().get(page_index)?;
        Some(page.to_view(x, y))
    }

    /// The reverse of `to_view`: the point of the page shown at a place.
    #[pyo3(name = "from_view")]
    fn page_point(&self, page_index: usize, x: f64, y: f64) -> Option<(f64, f64)> {
        let page = *self.lock().pages().get(page_index)?;
        Some(page.from_view(x, y))
    }

    /// The width and height of a page as displayed, or None.
    fn view_size(&self, page_index: usize) -> Option<(f64, f64)> {
        let page = *self.lock().pages().get(page_index)?;
        Some(page.view_size())
    }

    /// Find text on the page, ignoring case. Each place found is a list of
    /// rectangles (left, bottom, right, top), one for each line it touches.
    fn search_page(&self, page_index: usize, needle: &str) -> PyResult<Vec<Vec<Rectangle>>> {
        let hits = self.lock().search_page(page_index, needle);
        Ok(hits
            .map_err(to_python_error)?
            .into_iter()
            .map(|lines| lines.into_iter().map(rectangle).collect())
            .collect())
    }

    /// Give the renderer a TrueType font, as bytes, to draw text with when
    /// a page's own font cannot be drawn. Returns whether it is usable.
    fn set_fallback_font(&self, py: Python<'_>, data: &[u8]) -> bool {
        let data = data.to_vec();
        // Parsing the font does not touch Python objects.
        py.detach(|| self.lock().set_fallback_font(data))
    }

    /// Render the page at `scale` pixels per point, as a tuple (width,
    /// height, pixels) of 8-bit rows from the top, with no padding between
    /// rows. `mode` is "RGBA" (four bytes a pixel), "RGB" (three), or "L"
    /// (one, gray).
    #[pyo3(signature = (page_index, scale=1.0, mode="RGBA"))]
    fn render_page<'py>(
        &self,
        py: Python<'py>,
        page_index: usize,
        scale: f64,
        mode: &str,
    ) -> PyResult<(usize, usize, Bound<'py, PyBytes>)> {
        check_mode(mode)?;
        let image = py.detach(|| self.lock().render_page(page_index, scale));
        Ok(pixels(py, &image.map_err(to_python_error)?, mode))
    }

    /// Render one rectangle of the page's image at `scale` pixels per
    /// point, as `render_page` gives it: a tuple (width, height, pixels)
    /// holding exactly the pixels of that image from column `left` and row
    /// `top`, `width` columns across and `height` rows down. A rectangle
    /// that reaches past the image is cut to it, so the width and height
    /// returned may be smaller than those asked for; one with no pixel of
    /// the image raises `PdfError`.
    #[pyo3(signature = (page_index, scale, left, top, width, height, mode="RGBA"))]
    #[allow(clippy::too_many_arguments)]
    fn render_page_region<'py>(
        &self,
        py: Python<'py>,
        page_index: usize,
        scale: f64,
        left: usize,
        top: usize,
        width: usize,
        height: usize,
        mode: &str,
    ) -> PyResult<(usize, usize, Bound<'py, PyBytes>)> {
        check_mode(mode)?;
        let region = engine::PixelRegion {
            left,
            top,
            width,
            height,
        };
        let image = py.detach(|| self.lock().render_page_region(page_index, scale, region));
        Ok(pixels(py, &image.map_err(to_python_error)?, mode))
    }

    /// Render the page as `render_page` does and encode it as a PNG file.
    #[pyo3(signature = (page_index, scale=1.0))]
    fn render_page_png<'py>(
        &self,
        py: Python<'py>,
        page_index: usize,
        scale: f64,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let image = py.detach(|| self.lock().render_page(page_index, scale));
        let png = image.map_err(to_python_error)?.to_png();
        Ok(PyBytes::new(py, &png))
    }

    /// Replace the outline (bookmarks), in reading order. Each item is an
    /// `OutlineItem`, such as one from `outline()`, or a (level, title,
    /// page) or (level, title, page, view) tuple, where page is a
    /// zero-based index or None and view a `View` or None. An item with a
    /// page and no view leads to the top of the page. An empty list removes
    /// the outline.
    fn set_outline(&self, items: Vec<NewOutlineItem>) -> PyResult<()> {
        let items: Vec<engine::OutlineItem> = items.into_iter().map(Into::into).collect();
        let result = self.lock().set_outline(&items);
        result.map_err(to_python_error)
    }

    /// Set a page's display rotation, clockwise, to a multiple of 90
    /// degrees.
    fn set_page_rotation(&self, page_index: usize, degrees: i32) -> PyResult<()> {
        let result = self.lock().set_page_rotation(page_index, degrees);
        result.map_err(to_python_error)
    }

    /// Insert a blank page, sized in points, before the page with the
    /// given zero-based index, or at the end when it is the page count.
    fn insert_blank_page(&self, page_index: usize, width: f64, height: f64) -> PyResult<()> {
        let result = self.lock().insert_blank_page(page_index, width, height);
        result.map_err(to_python_error)
    }

    /// Remove what a page shows inside the given (left, bottom, right, top)
    /// areas, in points from the bottom left of the unrotated page, and
    /// paint each area with `fill`, a (red, green, blue) color from 0 to 1,
    /// or with nothing when it is None.
    ///
    /// Glyphs whose middle lies inside an area go, and the glyphs after them
    /// keep their places; paths, images, and forms wholly inside an area go;
    /// the pixels inside an area of an image partly inside one are made
    /// white. Returns how many of each were removed: a dictionary with
    /// "glyphs", "images", "graphics", and "kept_images", the images partly
    /// inside an area that were left as they were because they are inline
    /// or cannot be decoded.
    #[pyo3(signature = (page_index, areas, fill=Some((1.0, 1.0, 1.0))))]
    fn redact(
        &self,
        page_index: usize,
        areas: Vec<Rectangle>,
        fill: Option<(f64, f64, f64)>,
    ) -> PyResult<BTreeMap<&'static str, usize>> {
        let areas: Vec<engine::Rectangle> = areas
            .into_iter()
            .map(|(left, bottom, right, top)| engine::Rectangle {
                left,
                bottom,
                right,
                top,
            })
            .collect();
        let fill = fill.map(|(red, green, blue)| [red, green, blue]);
        let result = self.lock().redact(page_index, &areas, fill);
        let result = result.map_err(to_python_error)?;
        Ok(BTreeMap::from([
            ("glyphs", result.glyphs),
            ("images", result.images),
            ("graphics", result.graphics),
            ("kept_images", result.kept_images),
        ]))
    }

    /// Show text on a page in a standard 14 font, from (x, y), the start of
    /// its first baseline in points from the bottom left of the unrotated
    /// page, and return the width of its widest line.
    ///
    /// `font` is a standard font's name, a common name for one such as
    /// "Arial", or one of PyMuPDF's short names such as "helv". Each line
    /// feed starts a line 1.2 times the size lower. `color` is (red, green,
    /// blue) from 0 to 1. A character the font's encoding lacks shows as a
    /// question mark.
    #[pyo3(signature = (page_index, x, y, text, font="Helvetica", size=11.0, color=(0.0, 0.0, 0.0)))]
    #[allow(clippy::too_many_arguments)]
    fn insert_text(
        &self,
        page_index: usize,
        x: f64,
        y: f64,
        text: &str,
        font: &str,
        size: f64,
        color: (f64, f64, f64),
    ) -> PyResult<f64> {
        let font = standard_font(font)?;
        let color = [color.0, color.1, color.2];
        let result = self
            .lock()
            .insert_text(page_index, (x, y), text, font, size, color);
        result.map_err(to_python_error)
    }

    /// Break a page in two at a height in points from the bottom of the
    /// unrotated page: what lies below moves to the top of a new page that
    /// follows.
    fn break_page(&self, page_index: usize, height: f64) -> PyResult<()> {
        let result = self.lock().break_page(page_index, height);
        result.map_err(to_python_error)
    }

    /// Keep only the pages with the given zero-based indexes, in that
    /// order: this reorders, removes, and repeats pages.
    fn select_pages(&self, indexes: Vec<usize>) -> PyResult<()> {
        let result = self.lock().select_pages(&indexes);
        result.map_err(to_python_error)
    }

    /// Set a document information entry such as "Title", or remove it by
    /// passing None.
    #[pyo3(signature = (key, value=None))]
    fn set_information(&self, key: &str, value: Option<&str>) -> PyResult<()> {
        let result = self.lock().set_information(key, value);
        result.map_err(to_python_error)
    }

    /// The document, with the changes made to it, as the bytes of a new
    /// file. An encrypted document stays encrypted the way it was, with the
    /// same passwords and permissions.
    fn save<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        let data = py.detach(|| self.lock().save());
        Ok(PyBytes::new(py, &data.map_err(to_python_error)?))
    }

    /// The bytes of the embedded file listed under a name, or None.
    fn attachment_data<'py>(
        &self,
        py: Python<'py>,
        name: &str,
    ) -> PyResult<Option<Bound<'py, PyBytes>>> {
        let data = self.lock().attachment_data(name).map_err(to_python_error)?;
        Ok(data.map(|data| PyBytes::new(py, &data)))
    }

    /// Embed a file under a name, replacing any file listed under it.
    /// `file_name` is the name it is saved under, the listed name when
    /// None.
    #[pyo3(signature = (name, data, file_name=None, description=None))]
    fn add_attachment(
        &self,
        name: &str,
        data: &[u8],
        file_name: Option<&str>,
        description: Option<&str>,
    ) -> PyResult<()> {
        let result = self
            .lock()
            .add_attachment(name, data, file_name, description);
        result.map_err(to_python_error)
    }

    /// Remove the embedded file listed under a name; returns whether there
    /// was one.
    fn remove_attachment(&self, name: &str) -> PyResult<bool> {
        let result = self.lock().remove_attachment(name);
        result.map_err(to_python_error)
    }

    /// The document's XMP metadata as text, or None when it has none.
    /// Bytes that are not UTF-8 become U+FFFD.
    fn xmp_metadata(&self) -> PyResult<Option<String>> {
        let data = self.lock().xmp_metadata().map_err(to_python_error)?;
        Ok(data.map(|data| String::from_utf8_lossy(&data).into_owned()))
    }

    /// Set the document's XMP metadata from text, or remove it with None.
    #[pyo3(signature = (text=None))]
    fn set_xmp_metadata(&self, text: Option<&str>) -> PyResult<()> {
        let result = self.lock().set_xmp_metadata(text.map(str::as_bytes));
        result.map_err(to_python_error)
    }

    /// The document as `save` writes it, but without any encryption.
    fn save_decrypted<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        let data = py.detach(|| self.lock().save_decrypted());
        Ok(PyBytes::new(py, &data.map_err(to_python_error)?))
    }

    /// The document as `save` writes it, protected with AES-256 and the
    /// given passwords. The user password opens it with `permissions`, an
    /// iterable of permission names ("print", "modify", "copy",
    /// "annotate", "fill_forms", "extract_for_accessibility", "assemble",
    /// "print_high_quality"), or all of them when None; an empty user
    /// password lets anyone open it. The owner password opens it with every
    /// permission.
    #[pyo3(signature = (user_password="", owner_password="", permissions=None))]
    fn save_encrypted<'py>(
        &self,
        py: Python<'py>,
        user_password: &str,
        owner_password: &str,
        permissions: Option<Vec<String>>,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let granted = match permissions {
            None => engine::Permissions::ALL,
            Some(names) => {
                if let Some(unknown) = names
                    .iter()
                    .find(|name| !PERMISSION_NAMES.contains(&name.as_str()))
                {
                    let message = format!("unknown permission {unknown:?}");
                    return Err(PyValueError::new_err(message));
                }
                let has = |name: &str| names.iter().any(|given| given == name);
                engine::Permissions {
                    print: has("print"),
                    modify: has("modify"),
                    copy: has("copy"),
                    annotate: has("annotate"),
                    fill_forms: has("fill_forms"),
                    extract_for_accessibility: has("extract_for_accessibility"),
                    assemble: has("assemble"),
                    print_high_quality: has("print_high_quality"),
                }
            }
        };
        let data = py.detach(|| {
            self.lock()
                .save_encrypted(user_password, owner_password, granted)
        });
        Ok(PyBytes::new(py, &data.map_err(to_python_error)?))
    }

    /// How the document is encrypted, such as "Standard V5 R6 256-bit
    /// AES", or None when it is not.
    #[getter]
    fn encryption(&self) -> Option<String> {
        self.lock().encryption().map(str::to_owned)
    }

    /// What an encrypted document lets its user do, as a dictionary from
    /// each permission's name to whether it is granted, or None when the
    /// document is not encrypted.
    #[getter]
    fn permissions(&self) -> Option<BTreeMap<&'static str, bool>> {
        let permissions = self.lock().permissions()?;
        let flags = permission_flags(permissions);
        Some(PERMISSION_NAMES.into_iter().zip(flags).collect())
    }

    /// The decoded content stream of the page with the given zero-based
    /// index; empty for a page without content.
    fn page_content<'py>(
        &self,
        py: Python<'py>,
        page_index: usize,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let data = self
            .lock()
            .page_content(page_index)
            .map_err(to_python_error)?;
        Ok(PyBytes::new(py, &data))
    }

    /// The decoded data of a stream object, or None if it is not a stream.
    #[pyo3(signature = (number, generation=0))]
    fn stream<'py>(
        &self,
        py: Python<'py>,
        number: u32,
        generation: u16,
    ) -> PyResult<Option<Bound<'py, PyBytes>>> {
        let reference = engine::ObjectReference { number, generation };
        let data = self.lock().stream(reference).map_err(to_python_error)?;
        Ok(data.map(|data| PyBytes::new(py, &data)))
    }

    fn __repr__(&self) -> String {
        let document = self.lock();
        format!(
            "Document(version={}, page_count={})",
            document.version(),
            document.page_count()
        )
    }
}

/// Read PDF files with the Plain Open PDF engine.
#[pymodule(name = "plain_open_pdf_engine")]
mod python_module {
    #[pymodule_export]
    use super::{
        Attachment, Document, Link, OutlineItem, Page, PasswordRequired, PdfError, View,
        WrongPassword, text_width,
    };
}
