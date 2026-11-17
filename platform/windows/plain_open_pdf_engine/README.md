# Plain Open PDF Engine

An independent Rust library for Plain Open PDF, licensed under MIT. The Windows
app uses it in place of PyMuPDF; the Linux and macOS implementations still use
PyMuPDF. This crate opens PDFs, including password-protected ones, and reads
their pages, document information, outline, links, embedded files, and stream
data. It extracts text with positions, searches, renders vector graphics,
text, and most images, and saves documents with the app's edits.

## Current capability

`Document::open(Vec<u8>)` opens a PDF held in memory and lists its pages in
order, each with its media box, crop box, and rotation.
`Document::open_with_password` does the same with a user or owner password.

```rust
use plain_open_pdf_engine::Document;

let document = Document::open(std::fs::read("file.pdf").unwrap()).unwrap();
for page in document.pages() {
    println!("{} x {}", page.media_box.width(), page.media_box.height());
}
```

Opening a document covers:

- Cross-reference tables, cross-reference streams, hybrid files, and
  incremental updates, with file offsets counted from the header.
- Objects stored at file offsets and inside object streams.
- Stream filters `FlateDecode` (the crate's own zlib decoder), `LZWDecode`,
  `ASCIIHexDecode`, `ASCII85Decode`, and `RunLengthDecode`, alone or chained,
  with PNG predictors and the TIFF predictor for 8- and 16-bit components.
- `DCTDecode` for baseline, extended sequential, and progressive JPEG
  images, decoded to one byte for each sample: gray, RGB, or CMYK, with
  subsampled color components smoothly stretched. On real progressive
  photographs the samples are within a few levels of libjpeg's. YCCK images
  give CMYK as libjpeg does: the reverse of the decoded color for cyan,
  magenta, and yellow, and black as stored.
- `CCITTFaxDecode` for Group 3 one-dimensional, Group 3 two-dimensional,
  and Group 4 fax data, decoded to one bit for each pixel. Group 3 and
  Group 4 data from the Windows TIFF encoder decode exactly as PyMuPDF
  decodes them; Group 3 two-dimensional data is not yet tested against
  another decoder.
- `JPXDecode` and `JBIG2Decode` report `Unsupported`; they are left out of
  the first complete version.
- Page attributes inherited through the page tree. A page with no usable media
  box is US Letter, and the crop box is clipped to the media box.
- Repair: when the cross-reference data is missing, damaged, or leads to no
  pages, object locations are rebuilt by scanning the file for definitions.
- A stream whose `/Length` is wrong is read up to its `endstream` keyword.

Beyond pages, a document provides:

- `information()`: title, author, subject, keywords, creator, producer, and
  the creation and modification dates, decoded from PDFDocEncoding, UTF-16BE,
  or UTF-8 text strings. `decode_text_string` exposes that decoding.
  `information_entries()` gives every text entry by key, custom ones
  included.
- `outline()`: the bookmarks in reading order, each with its level, title,
  the index of the page it leads to, and the `View` of that page. Explicit,
  named, and go-to action destinations are resolved; items that lead outside
  the document have no page.
- `links(page_index)`: the link annotations on a page, each with its rectangle
  and target: a page of this document, a URI, another PDF file, a file to
  launch, or a named viewer operation. Links with other actions are left out.
  A link to a page carries the `View` its destination gives.
- `named_destination(name)`: the page a named destination leads to and its
  `View`, looked up in the `/Names` tree and then in the older `/Dests`
  dictionary.
- A `View` is how a viewer shows a destination page (ISO 32000-2 section
  12.3.2.2): `XYZ` with a left, a top, and a zoom, any of which may be left
  to the viewer; `Fit`, `FitH`, `FitV`, `FitR`, `FitB`, `FitBH`, or `FitBV`.
  `View::point` gives the point it puts at the window's top left corner and
  `View::zoom` its magnification. Coordinates are in the unrotated page's
  space; `Page::to_view` turns them into displayed ones.
- `attachments()`: the files in the embedded files name tree, each with its
  key, file name, description, and the stream that holds its bytes, and
  `attachment_data(name)` the bytes of one.
- `xmp_metadata()`: the catalog's XMP metadata stream, decoded.
- `page_content(page_index)`: the page's decoded content stream, with the
  streams of a `/Contents` array joined. `parse_content` turns it into
  operations, each an operator with its operands; an inline image's data
  is the last operand of its `BI` operation.
- `stream(reference)`: the decoded data of a stream.
- `object(reference)` and `trailer()`: any object, and the trailer dictionary.

`ToUnicodeMap::parse` reads a font's `/ToUnicode` CMap (codespace ranges,
`bfchar`, and `bfrange`), and its `decode` turns a string of character codes
into text.

`SimpleEncoding` maps the one-byte codes of a simple font to text. It provides
`StandardEncoding`, `MacRomanEncoding`, `WinAnsiEncoding`, and the built-in
encodings of Symbol and ZapfDingbats, transcribed from ISO 32000-1 Annex D,
and applies a `/Differences` array. `glyph_text` gives the text of a glyph
name: the names of those character sets, a few ligatures, and names that spell
out code points, such as `uni00E9`, `u1F600`, and `f_i`. The rest of the Adobe
Glyph List and `MacExpertEncoding` are not included.

`page_glyphs(page_index)` lists the glyphs a page shows, in content order,
including those inside form XObjects. Each `TextGlyph` has its text, the
position of its origin on the baseline, the end of its own width (before
character and word spacing), and its font size, in points from the bottom
left of the unrotated page. `page_text(page_index)` assembles text from
them: a line ends where the next glyph leaves the baseline or after a gap
wider than 0.8 of the font size, as between columns, and a space is added
where a smaller gap is wide enough to separate words. Only spaces part
words; other blank characters stay in the word they are in. On the
specification, 91% of the lines are the same as PyMuPDF's `get_text`
lines, 98.7% of the words are the same text in the same place, and the
spans of a page match PyMuPDF's. `page_words(page_index)` groups the
glyphs into words, each with its box and its block, line, and word numbers,
and `page_spans(page_index)` into runs that share a line, a font, a size, and
a fill color, each with its box and the origin of its first glyph. A glyph
carries its font's name and its fill color. `Page::to_view` and
`Page::from_view` turn these positions into the space a viewer draws in, from
the top left of the page as displayed, and back; `Page::view_size` is the
displayed size. `search_page(page_index, needle)` finds
text in it, ignoring case, and gives each place found as one rectangle for
each line it touches; their heights are estimated from the font size. Text
extraction is not ready to replace PyMuPDF's:

- Glyph widths come from the font dictionary (`/Widths`, or `/W` and `/DW`
  for a composite font, whose codes are taken to be CIDs). A standard 14
  font without them, or one under a common name for the same design such as
  `Arial,Bold` or `TimesNewRomanPSMT`, gets Adobe's metrics for it
  (`StandardFont`). Any other font without them gets half its size for every
  glyph, so its positions and spaces are approximate.
- Text is not reordered by position, and vertical writing is laid out as
  horizontal.
- A glyph outside a clipping rectangle is left out, which is what keeps the
  two parts of a broken page apart. Clipping by other shapes is not
  followed, so text hidden that way is still extracted.
- A simple font is decoded through its ToUnicode map and, for codes the map
  lacks, through its encoding: `/Encoding` with any `/Differences`, or else
  the built-in encoding of Symbol or ZapfDingbats, or `StandardEncoding`.
- Font programs are not read, so an embedded font that relies on its own
  built-in encoding is read as `StandardEncoding`, and a glyph name outside
  the known ones gives U+FFFD.
- A composite font is decoded through its ToUnicode map; without one, each
  code gives U+FFFD.

`save()` writes the document as a new file: every object that can be reached
from the catalog and the information dictionary, uncompressed, with one
cross-reference table. Stream data is copied as stored. An encrypted
document stays encrypted the way it was, with the same passwords and
permissions, whichever password opened it. `save_decrypted()` writes it
without encryption, and `save_encrypted(user, owner, permissions)` protects
it with AES-256 (revision 6) and new passwords. `encryption()` describes how
a document is encrypted, as "Standard V4 R4 128-bit AES", and
`permissions()` gives what its user may do. Changes are held in memory
until then:

- `set_object`, `set_stream`, and `add_object` replace or add objects, and
  `set_stream_data` compresses the data it is given with `FlateDecode`.
  `write_object` writes any object in PDF syntax, and `encode_text_string`
  encodes text for a string.
- `set_information(key, value)` sets or removes an information entry.
- `set_outline(items)` replaces the outline (bookmarks) with the given
  items, each with a level, a title, and perhaps a page and a view. An item
  with a page and no view leads to the top of the page. Items are written
  closed, and the old outline's colors and styles are not kept.
- `set_page_rotation(page_index, degrees)` turns a page by a multiple of 90
  degrees.
- `insert_blank_page(page_index, width, height)` inserts an empty page, and
  `Document::new()` starts a document without pages.
- `add_link(page_index, rectangle, target, view)` adds a borderless link to
  a page of this document, a URI, another PDF file, a file to launch, or a
  named viewer operation.
- `break_page(page_index, height)` breaks a page in two, as a page break
  does in a word processor: what lies below the height moves to the top of
  a new page that follows. Nothing is rewritten; both pages show the same
  content, each clipped to its part, so the break is a straight line and
  the text is not reflowed. Annotations move with their part. Outline items
  and links elsewhere that lead to the moved part still lead to the
  original page.
- `select_pages(indexes)` keeps the given pages in the given order, which
  reorders, removes, and repeats pages. When pages are removed, what leads
  to them is removed too, so that saving drops them from the file: links on
  the remaining pages, named destinations, and outline items, with the
  outline written again as `set_outline` writes it. The structure tree of a
  tagged document and its article threads are removed whole. A form field
  on a removed page can still hold that page in the file.
- `redact(page_index, areas, fill)` removes what a page shows inside the
  given rectangles, as applying redaction annotations does, and paints each
  with `fill` unless it is `None`. The page's content is written again
  without each glyph whose middle lies inside an area, visible or not, with
  the glyphs after it kept in place; without each path, image, and form
  XObject wholly inside one area; and with the pixels inside the areas of an
  image partly inside one made white, the image stored again as RGB. A form
  partly inside an area is copied and redacted in turn. Replaced images and
  forms are dropped from the page's resources, so saving leaves them out of
  the file unless another page uses them. A path partly inside an area stays
  whole, as do shadings; inline images partly inside an area and images in
  filters the engine cannot decode stay too, and the returned `Redaction`
  counts them. Annotations are not changed. Content streams that are parsed
  for this keep inline image data: `parse_content` gives it as the last
  operand of `BI`.
- `add_attachment(name, data, file_name, description)` embeds a file,
  compressed, replacing any listed under the same name, and
  `remove_attachment(name)` removes one. The embedded files name tree is
  written again as one sorted node; the name dictionary's other trees stay.
- `set_xmp_metadata(data)` sets the XMP metadata, stored uncompressed, or
  removes it.
- `insert_text(page_index, origin, text, font, size, color)` shows text in
  one of the standard 14 fonts from a baseline origin in default user space,
  with a line feed starting a new line 1.2 times the size lower. The page
  gains a font resource that gives the font's widths and a content stream
  after its own, which is enclosed in `q` and `Q` so that it cannot move
  the text. The text is written in `WinAnsiEncoding`, or the font's own for
  Symbol and ZapfDingbats, and a character the encoding lacks shows as a
  question mark. `StandardFont::text_width` measures text as it is shown;
  for ASCII text its widths match PyMuPDF's `get_text_length` to a
  millionth of a point.

Not yet available: compressing streams and objects that the file stored
uncompressed, incremental updates, and editing annotations.

`render_page(page_index, scale)` draws a page into a `Pixmap` of RGBA pixels at
`scale` pixels for each point, covering the crop box, turned by the page's
rotation, on white. Each side of the image is rounded up to a whole pixel and
the page is stretched to fill it, by less than a pixel, so no pale line is left
at the right or bottom edge. `Pixmap::to_png` encodes it, and `Pixmap::rgb` and
`Pixmap::gray` give its pixels without alpha or as gray levels.
`render_page_region(page_index, scale, PixelRegion)` draws one rectangle of that
image, given in its pixels from the top left, and returns exactly the pixels
`render_page` puts there; a rectangle that reaches past the image is cut to it,
and one wholly outside it is an error. The page is still read as a whole, but
only the rectangle's pixels are drawn, which is what a viewer zoomed far in
needs. This is a first, limited renderer:

- It fills, strokes, and clips paths with anti-aliasing, in page content and
  in form XObjects, with both fill rules, dash patterns, line caps, and the
  fill and stroke opacity of a graphics state dictionary.
- Text is drawn from a font's embedded TrueType program (`/FontFile2`),
  CFF program (`/FontFile3`, bare or inside an OpenType file), or Type 1
  program (`/FontFile`). `TrueTypeFont` reads a TrueType program: its glyph
  outlines, character maps, and advances. `CffFont` reads a CFF program:
  its Type 2 charstrings with subroutines, glyph names, its own encoding,
  and the charset and font dictionaries of a CID-keyed font. `Type1Font`
  reads a Type 1 program, binary or hexadecimal and with or without PFB
  segment headers: its encoding and font matrix, and its eexec-encrypted
  subroutines and charstrings, run with flex, accented characters
  (`seac`), and hint replacement. Type 1 support is tested with fonts built
  by the tests, not yet with fonts from real files.
- A simple font's CFF or Type 1 glyph is found by the glyph name its
  encoding gives the code, or by the program's own encoding when the font
  names none. The expert glyph names among the standard CFF strings are not
  included.
- Any other font, one that is not embedded, is
  drawn with the font given to `set_fallback_font`, by Unicode value, so
  its text is readable but not in its own typeface, weight, or style.
  Without a fallback font that text is not drawn. A font that gives no
  widths is spaced by the fallback font.
- Text is always filled; the outline modes use the stroke color, and text
  is not added to the clipping path.
- Type 3 fonts are drawn by running each glyph's stream under the font
  matrix; a glyph that starts with `d1` is painted in the text's color.
- Image XObjects are drawn, with 1 to 16 bits for each component, in
  gray, RGB, CMYK, Indexed, and ICC-based color spaces, with `/Decode`
  ranges, stencil masks painted in the fill color, and soft masks and
  stencil masks that hide parts of an image. An image larger than its
  place on the page is averaged down; one that is smaller keeps hard pixel
  edges, without smoothing. A JPEG image shown at half its size or less is
  decoded at a half, a quarter, or an eighth of its size, from each
  block's lowest frequencies, which keeps it at least as large as it is
  shown and saves most of the work.
- Decoded images are kept, up to 256 MiB across every document, by the file
  they come from, their object number, and what their size was divided by,
  so that drawing a page again, at another zoom or on another thread,
  reuses them; documents opened from one file share them. A document with
  objects set since it was opened draws without them, and so does an image
  whose color space names a page resource.
- Inline images are drawn as image XObjects are, with their abbreviated
  keys, filter names, and color spaces.
- Images in a filter that is not supported are not drawn, and masking by a
  range of colors is ignored.
- Fill and stroke colors are read in their color space: gray, RGB, CMYK,
  Lab, Indexed, ICC-based (by its alternate space or component count), and
  Separation and DeviceN through their tint transforms, without color
  management. Functions of every type are evaluated: sampled, exponential,
  stitching, and PostScript calculator.
- Shadings are drawn by `sh` and as shading patterns: function-based,
  axial, and radial, with their extents, bounding boxes, and a pattern's
  background. Mesh shadings (types 4 to 7) are not drawn.
- Tiling patterns fill, stroke, and paint text, colored or uncolored. Each
  cell is drawn through the area being painted but not clipped to its own
  bounding box, and a fill that would need more than 4096 cells is left
  out.
- Each annotation that is not hidden draws its normal appearance, or the
  one its appearance state picks, stretched onto its rectangle as ISO
  32000-2 section 12.5.5 describes. Popup annotations are not drawn. On
  annotations PyMuPDF makes, the page differs from PyMuPDF's by 0.4 levels
  on average.
- The Normal, Multiply, Screen, Darken, and Lighten blend modes of a
  graphics state dictionary are applied, so highlights show the text under
  them; other modes paint as Normal.
- Corners of strokes are always rounded, soft masks are ignored, and a
  transparency group is drawn straight onto the page.

Decoded streams are limited to 256 MiB.

Encrypted documents use the standard security handler:

- RC4 with 40- to 128-bit keys, AES-128, and AES-256 (revisions 2 through 6),
  with the ciphers and hashes implemented in this crate.
- `Document::open` succeeds when the user password is empty, as it is for files
  that only restrict printing or copying. Otherwise it reports
  `DocumentError::PasswordRequired`; `open_with_password` with a password
  that is wrong reports `DocumentError::WrongPassword`.
- Strings and streams are decrypted as objects are loaded, and encrypted
  again when the document is saved; AES initialization vectors and new keys
  and salts come from bytes the operating system seeds.
- Other security handlers, such as certificate encryption, report
  `Unsupported`. Passwords are used as the bytes given: text is not normalized
  for AES-256. Permission flags are read and written, not enforced.

RC4 and MD5 are broken as security primitives; the `crypto` module only uses
them to keep a file's existing protection, and new protection is AES-256.
Nothing in it is hardened against timing attacks.

The `page_info` example prints the page count and page boxes of the files named
on its command line. `--password TEXT` supplies a password, and `--info`,
`--outline`, `--links`, `--attachments`, and `--streams` also print the
information entries, the outline, each page's links, the embedded files, and a
checksum of each decoded stream:

```powershell
cargo run --offline --release --example page_info -- file.pdf
```

`parse_header(&[u8])` reads the version from the PDF header. It recognizes PDF
1.0 through 1.7 and 2.0, accepts a `%PDF-` signature that begins anywhere in
the first 1024 bytes, ignores the bytes after the version, and returns typed
errors for missing, malformed, or unrecognized headers. Recognition
of a version does not mean the engine supports that version's document features.
The parser operates on bytes without requiring UTF-8, allocating a document,
or reading files. A valid header alone does not establish a valid PDF.

`parse_object(&[u8], ParseLimits)` parses one object prefix: null, booleans,
integers, real numbers, names, literal and hexadecimal strings, arrays,
dictionaries, and unresolved indirect references. Names and strings preserve
decoded bytes without assuming UTF-8. Whitespace, comments, name escapes,
string escapes, and nested containers follow PDF lexical rules.

```rust
use plain_open_pdf_engine::{ParseLimits, PdfObject, parse_object};

let parsed = parse_object(b"[0 0 612 792]", ParseLimits::default()).unwrap();
assert!(matches!(parsed.value, PdfObject::Array(_)));
assert_eq!(parsed.consumed, 13);
```

The result reports bytes consumed, including leading comments and whitespace;
trailing bytes are not validated or consumed. For example, parsing
`<< /Length 0 >> stream` returns only the dictionary, not a stream object.
Errors report a category and zero-based byte offset in the supplied slice.

The parser repairs defects that are common in real files, as other readers do,
instead of rejecting them:

- The last of several duplicate dictionary keys wins.
- A reference that cannot name an object, such as `0 0 R` or one whose numbers
  are out of range, reads as null.
- A `#` in a name that is not followed by two hexadecimal digits stays literal.
- A malformed number made only of digits, signs, and periods is read leniently:
  `--5` is -5, `4.-5` is 4.5, `1.2.3` is 1.2, and a lone sign or period is 0.
- An integer beyond `i64` becomes a real number.

Default limits allow a 16 MiB input prefix, 500,000 objects (including
dictionary keys), and nesting depth 32 for arrays and dictionaries. Parentheses
nested inside a literal string are not limited. The maximum configurable depth
is 64. Input limits include reference lookahead, so lookahead may reach a limit
even when the first integer fits. Integers use `i64`; real numbers use finite
`f64` approximations. References use unsigned decimal tokens with a nonzero
`u32` object number and `u16` generation. These are implementation limits, not
claims of complete PDF conformance.

The crate has no third-party dependencies.

## Build and validate

Run these commands from `platform/windows/plain_open_pdf_engine/` with the
stable Rust MSVC toolchain and its Windows linker and SDK installed:

```powershell
cargo build --offline
cargo test --offline
cargo fmt --check
cargo clippy --offline --all-targets -- -D warnings
```

Cargo writes generated outputs to the ignored `target/` directory. Commit
`Cargo.lock` to keep development builds reproducible. The crate is not enabled
for publication to a package registry.

## Python bindings

The `python/` folder is a separate crate that exposes the engine to Python as
the `plain_open_pdf_engine` module. It depends on [PyO3](https://pyo3.rs), which
is licensed under MIT or Apache-2.0, and is separate because PyO3's generated
code needs `unsafe`, which the engine crate forbids. It builds against Python's
stable ABI: one build serves Python 3.14 and earlier, and a build made with
Python 3.15 or later serves those versions.

```python
import plain_open_pdf_engine as engine

document = engine.Document.open("file.pdf", password=None)
print(document.version, document.page_count)
for page in document.pages:
    print(page.media_box, page.crop_box, page.rotation)
print(document.information())
for item in document.outline():
    print(item.level, item.title, item.page, item.view)
for link in document.links(0):
    print(link.rectangle, link.kind, link.page, link.uri)
    if link.view:  # View(fit, left, top, zoom, area); None keeps the viewer's
        print(link.view.fit, link.view.left, link.view.top, link.view.zoom)
page, view = document.named_destination("chapter1") or (None, None)
for attachment in document.attachments():
    print(attachment.name, len(document.stream(attachment.stream)))
document.add_attachment("notes.txt", b"my notes", description="Notes")
notes = document.attachment_data("notes.txt")  # None when there is none
document.remove_attachment("notes.txt")
document.set_xmp_metadata("<x:xmpmeta xmlns:x='adobe:ns:meta/'/>")
xmp = document.xmp_metadata()  # text, or None
content = document.page_content(0)
text = document.page_text(0)  # limited; see above
for text, x, y, end_x, end_y, size in document.page_glyphs(0):
    print(text, x, y)
document.set_fallback_font(open("DejaVuSans.ttf", "rb").read())
width, height, pixels = document.render_page(0, scale=2.0)
# The 300 by 200 pixels of that image from column 100 and row 50:
width, height, part = document.render_page_region(0, 2.0, 100, 50, 300, 200)
png = document.render_page_png(0)
hits = document.search_page(0, "needle")  # [[(left, bottom, right, top)]]
for left, bottom, right, top, text, block, line, word in document.page_words(0):
    print(text, document.to_view(0, left, top))
for text, box, origin, font, size, color in document.page_spans(0):
    print(text, font, size, color)
document.set_information("Title", "New title")
document.select_pages([2, 0, 1])
document.set_outline([(1, "Chapter", 0), (2, "Section", 1, engine.View("XYZ", top=400))])
document.set_outline(document.outline())  # items read back keep their views
document.set_page_rotation(0, 90)
document.insert_blank_page(0, 612, 792)
document.break_page(1, 300)  # what is below 300 points moves to a new page
removed = document.redact(0, [(100, 700, 200, 720)])  # white fill by default
print(removed)  # {"glyphs": ..., "images": ..., "graphics": ..., "kept_images": ...}
size = 11
while engine.text_width("New text", "helv", size) > 80:  # PyMuPDF's names work too
    size -= 0.5
document.insert_text(0, 100, 700, "New text", font="Helvetica", size=size, color=(0, 0, 0))
data = document.save()  # an encrypted document stays encrypted
plain = document.save_decrypted()
protected = document.save_encrypted("user", "owner", permissions=["print"])
print(document.encryption, document.permissions)  # None for an unencrypted file
```

`Document(data, password=None)` opens a document from bytes. Boxes are
`(left, bottom, right, top)` tuples in points. Failures raise `PdfError`, and a
missing password raises its subclass `PasswordRequired`, and a wrong one
`WrongPassword`, a subclass of that. `Document.new()` starts an empty document,
and `render_page(page_index, scale, mode)` gives "RGBA", "RGB", or "L" (gray)
pixels, as does
`render_page_region(page_index, scale, left, top, width, height, mode)` for one
rectangle of the same image. A document may be shared between threads; calls on
it run one at a time. Documents that `Document.open(path)` opens from one file
share a single copy of its bytes while the file still holds exactly those bytes,
so that a document for each thread does not cost a copy of the file each.
`page(page_index)` gives one page's geometry without building the whole `pages`
list.

Build and test from `platform/windows/plain_open_pdf_engine/python/`. The first
build downloads PyO3 from crates.io, and `PYO3_PYTHON` selects the interpreter:

```powershell
$env:PYO3_PYTHON = "..\..\.venv\Scripts\python.exe"
cargo build --release
Copy-Item target\release\plain_open_pdf_engine_python.dll plain_open_pdf_engine.pyd
..\..\.venv\Scripts\python.exe -m pytest tests
```

After switching `PYO3_PYTHON` between Python 3.14 or earlier and 3.15 or
later, run `cargo clean` first: Cargo does not notice that the interpreter
behind the same path changed, and the stale build fails to import.

The Windows app imports the module through
`plain_open_pdf/adapters/document/engine/`. `../scripts/engine/build.ps1` builds
it into the app's virtual environment, and `plain_open_pdf_engine.pyi` beside
the bindings types it for mypy; keep the stub in step with `src/lib.rs`.

## Next milestone

[Replacing PyMuPDF](PYMUPDF_REPLACEMENT.md) lists everything the application
asks PyMuPDF to do and where the engine stands on each; work from it. In the
engine:

- Rendering: mesh shadings, soft masks, and the remaining blend modes;
  checking Type 1 support against fonts from real files.
- Text: reading order, and composite fonts without a ToUnicode map.
- Saving: editing annotations, and starting an empty document and adding
  links, which the application's tests need for their fixtures.

Object syntax follows ISO 32000 sections 7.2 and 7.3, with reference to the
[PDF Association syntax errata](https://pdf-issues.pdfa.org/32000-2-2020/clause07.html).
Implement from the
[PDF specifications](https://pdfa.org/resource/pdf-specification-archive/) and
original tests. Do not copy or translate MuPDF or other copyleft engine source.
Review dependencies before adding them; each retains its own license.

## License

See [LICENSE](LICENSE), matching the repository's MIT license.

`src/font/standard_metrics.rs` is generated from Adobe's Core 14 AFM files and
keeps Adobe's copyright notices and license, which allows using, copying, and
distributing them with that notice and with changes marked.
