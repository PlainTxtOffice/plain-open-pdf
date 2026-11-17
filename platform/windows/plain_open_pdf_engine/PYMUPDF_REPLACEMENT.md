# Replacing PyMuPDF: what the app needs

This is the checklist of everything the Plain Open PDF desktop app asked PyMuPDF
to do, with where the engine stands on each. The Windows app switched to the
engine on 2026-11-15 and no longer depends on PyMuPDF; the rows still Partial or
Missing are the gaps it accepted. It comes from an audit of
`platform/windows/plain_open_pdf/` on 2026-11-14; the Linux and macOS ports use
the same PyMuPDF calls, so it covers all three. Update the status column as the
engine changes.

Status: **Done**, **Partial** (works, with the gap named), or **Missing**.

## Opening and reading

| App feature | PyMuPDF calls | Engine | Gap |
| --- | --- | --- | --- |
| Open, password | `open`, `needs_pass`, `authenticate`, `close` | Done | A missing password raises `PasswordRequired` and a wrong one `WrongPassword` |
| Page count, page access | `page_count`, `load_page` | Done | |
| Page size for zoom and fit | `page.rect` (the rotated size) | Done | `Page::view_size` |
| Document properties | `metadata`, `set_metadata` | Done | `encryption` gives PyMuPDF's `metadata["encryption"]` text, such as "Standard V4 R4 128-bit AES" |
| Custom properties | `xref_get_key`, `xref_get_keys`, `xref_set_key`, `get_pdf_str` | Done | `information_entries` reads every text entry, and `set_information` sets or removes any key |
| XMP metadata | `get_xml_metadata`, `set_xml_metadata`, `del_xml_metadata` | Done | `xmp_metadata`, `set_xmp_metadata`; PyMuPDF reads what the engine writes |
| Outline | `get_toc`, `set_toc` | Partial | Items keep their page and view; setting the outline loses colours and styles |
| Notes, kept as an embedded file | `embfile_names`, `embfile_get`, `embfile_add`, `embfile_del` | Done | `attachments`, `attachment_data`, `add_attachment`, `remove_attachment`; files pass between the engine and PyMuPDF both ways. The engine does not record creation and change dates |
| Errors | `FileDataError`, `RuntimeError`, `ValueError`, `OSError` | Done | The module raises `PdfError`, which the app's error tuple includes |

## Rendering

The app renders with `get_pixmap(matrix=Matrix(s, s), alpha=False)` and reads
`samples`, `w`, `h`, `stride`, and `n`. Viewing, thumbnails, printing, and OCR
all go through it; printing and OCR only need pixels.

| What is drawn | Engine | Gap |
| --- | --- | --- |
| Paths, fills, strokes, clips | Done | Miter joins are rounded |
| Text in TrueType and CFF fonts | Done | |
| Text in Type 1 fonts | Done | Tested with fonts the tests build, not yet with fonts from real files |
| Text in fonts that are not embedded | Partial | Drawn with the fallback font, in one typeface; the standard 14 fonts are spaced by their own metrics |
| Text in Type 3 fonts | Done | |
| Images: JPEG, fax, Flate, LZW, masks, inline images | Done | Progressive JPEG matches libjpeg within a few levels on real photographs; fax data from the Windows encoder decodes exactly as PyMuPDF decodes it. JBIG2 and JPEG 2000 are left out of the first version, so those images are not drawn |
| Shadings and patterns | Partial | Function-based, axial, and radial shadings and tiling patterns are drawn, and spot colors through their tint transforms; mesh shadings are not |
| Annotation appearances | Done | Normal appearances, with Multiply and the other separable blend modes, so highlights show their text; on annotations PyMuPDF makes the page differs by 0.4 levels on average |
| Pixel format | Done | `render_page` gives "RGBA", "RGB", or "L" rows with no padding, so the stride is the width times the bytes a pixel |

## Text

| App feature | PyMuPDF calls | Engine | Gap |
| --- | --- | --- | --- |
| Text panel | `get_text("text")` | Done | 91% of the specification's lines and 81% of the supplement's are the same as PyMuPDF's; the rest differ mostly in where a line is cut at a gap |
| Selection and copy | `get_text("words")`: a box, text, and block, line, and word numbers for each word | Done | `page_words` gives the same text in the same place as PyMuPDF for 98.7% of the specification's words; box heights are estimated from the font size, about half a point off. PyMuPDF reads code 0xA0 of some fonts as a figure space where the engine reads WinAnsiEncoding's space |
| Search | `search_for(query)` | Done | 98% to 100% of PyMuPDF's hits on the two sample files; text is matched in content order |
| Rotated pages | Words, hits, and spans are used without a rotation transform | Partial | PyMuPDF reports text in the unrotated page's space, from its top left; the engine reports it from the bottom left, and `Page::to_view` gives where it is displayed. The module does not offer PyMuPDF's form directly |

## Links

| App feature | PyMuPDF calls | Engine | Gap |
| --- | --- | --- | --- |
| Link rectangles and targets | `get_links`: `from`, `uri`, `file`, `page`, `to`, `zoom`, `nameddest` | Done | A link's `View` gives the point and zoom; on the specification all 3,915 links agree with PyMuPDF's page and point. Links do not report the name they were resolved from |
| Named destinations | `resolve_link("#nameddest=...")` | Done | `named_destination`; all 1,689 of the specification's names agree with PyMuPDF |
| Rotation of link rectangles | `rotation_matrix`, `transformation_matrix` | Done | `to_view` on a rectangle's corners, or on a view's point, gives where it is displayed |

## Editing and saving

| App feature | PyMuPDF calls | Engine | Gap |
| --- | --- | --- | --- |
| Rotate a page | `set_rotation` | Done | |
| Insert a blank page | `new_page(pno, width, height)` | Done | |
| Delete a page | `delete_page` | Done | |
| Move a page | `move_page` | Done | Through `select_pages` |
| Remove content | `add_redact_annot(rect, fill=(1, 1, 1))`, `apply_redactions()` | Done | `redact`, with PyMuPDF's defaults: text whose middle is inside goes, line art wholly inside goes, and image pixels inside are blanked. On 25 pages of the specification both engines removed the same words. Inline images and images in undecodable filters partly inside are kept, and counted |
| Find the text to replace | `get_text("dict")`: each span's text, box, origin, font, size, and colour | Done | `page_spans` gives these, with as many runs as PyMuPDF on the page compared |
| Replace text | `get_text_length`, `insert_text` with a Base-14 font | Done | `text_width` and `insert_text` accept PyMuPDF's short names; for ASCII text the widths match PyMuPDF's to a millionth of a point. PyMuPDF miscounts other characters (two accented letters as one), and the engine uses Adobe's widths for them |
| Save in place | `can_save_incrementally`, `saveIncr` | Missing | The app writes a full save to a temporary file and replaces the original |
| Save and Save As | `save(path, encryption=PDF_ENCRYPT_KEEP)` | Done | `save` keeps the encryption; PyMuPDF opens files kept as RC4-40, RC4-128, AES-128, and AES-256 with either password and sees the changes |

## How the app replaces text today

1. `get_text("dict")` gives the spans of the page; the span that overlaps the
   selection most is chosen.
1. The span's box is redacted, which erases everything inside it.
1. The font is mapped to a Base-14 name (`helv`, `tiro`, `cour`, `times`,
   `symb`, `zadb`); most real fonts fall back to `helv`.
1. The size starts at the span's and shrinks by half a point, down to 4, until
   `get_text_length` fits the span's width. The text is never wrapped.
1. `insert_text` draws it at the span's original baseline origin in the span's
   colour.

So the engine needs spans with font, size, and colour; redaction of a
rectangle; widths for the standard 14 fonts; and adding text to a page's
content with one of those fonts.

## Tests

The app's tests build their fixtures with the engine through
`tests/engine_pdfs.py`: `Document.new`, `insert_blank_page`, `insert_text`,
`add_link`, `set_outline`, `add_attachment`, and `save_encrypted`. A small
hand-written PDF covers named destinations, which the engine does not write.
A test checks that nothing in `plain_open_pdf/` imports `pymupdf` or `fitz`.

## Other things the audit found

- Several threads each open their own handle on one file. The engine allows
  that: documents are independent.
- OCR runs Tesseract on a page image at twice the page size. It needs only
  pixels from the engine and writes nothing into the PDF.
- Printing draws page images at the printer's resolution. There is no vector
  path.
- PyMuPDF reports the words of a rotated page where they are on the
  unrotated page, and the app uses them without a rotation transform. Whether
  the app's selection lines up on a rotated page was not checked.
- PyMuPDF gave link destination points from the top left of the page, and
  the app converted them as if they were from the bottom left, so a link
  scrolled to the wrong height. The engine's points are all from the bottom
  left, and the switch fixed the scrolling; the tests now expect a
  destination 250 points below the page's top to be shown there.
- Not verified: the background rendering, search, and OCR handles open the
  file without a password, so they may fail on a protected PDF.
- Not verified: the OCR setup message names an `ocr` extra that
  `pyproject.toml` does not declare.

## Order of work

1. Done on Windows: the adapter layer in
   `plain_open_pdf/adapters/document/engine/` and the switch.
1. Next: the Linux and macOS implementations, which still use PyMuPDF.
