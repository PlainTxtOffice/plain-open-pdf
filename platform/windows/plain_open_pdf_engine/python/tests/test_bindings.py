"""Tests of the plain_open_pdf_engine Python module against a hand-built PDF."""

import os
import threading

import pytest

import plain_open_pdf_engine as engine


def build_pdf(objects: list[bytes], trailer: bytes = b"") -> bytes:
    """Assemble numbered objects, a cross-reference table, and a trailer."""
    data = bytearray(b"%PDF-1.7\n")
    offsets = []
    for number, body in enumerate(objects, 1):
        offsets.append(len(data))
        data += b"%d 0 obj\n%s\nendobj\n" % (number, body)
    start = len(data)
    data += b"xref\n0 %d\n0000000000 65535 f \n" % (len(objects) + 1)
    for offset in offsets:
        data += b"%010d 00000 n \n" % offset
    data += b"trailer\n<< /Size %d /Root 1 0 R %s >>\n" % (len(objects) + 1, trailer)
    data += b"startxref\n%d\n%%%%EOF\n" % start
    return bytes(data)


@pytest.fixture
def sample() -> bytes:
    """Two pages with information, an outline, links, and an embedded file."""
    return build_pdf(
        [
            b"<< /Type /Catalog /Pages 2 0 R /Outlines 6 0 R"
            b" /Names << /EmbeddedFiles 12 0 R >> >>",
            b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 612 792] >>",
            b"<< /Type /Page /Parent 2 0 R /Annots [10 0 R 11 0 R] /Contents 9 0 R >>",
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100]"
            b" /CropBox [10 10 190 90] /Rotate 90 >>",
            b"<< /Title (Sample) /Author <FEFF004A00F800720067> >>",
            b"<< /Type /Outlines /First 7 0 R /Last 8 0 R >>",
            b"<< /Title (Second page) /Parent 6 0 R /Next 8 0 R /Dest [4 0 R /Fit] >>",
            b"<< /Title (Web) /Parent 6 0 R /A << /S /URI /URI (https://example.org) >> >>",
            b"<< /Length 12 /Filter /ASCIIHexDecode >>\nstream\n48656C6C6F21>\nendstream",
            b"<< /Subtype /Link /Rect [10 20 110 40] /Dest [4 0 R /Fit] >>",
            b"<< /Subtype /Link /Rect [0 0 5 5]"
            b" /A << /S /URI /URI (https://example.org) >> >>",
            b"<< /Names [(hello.txt)"
            b" << /F (hello.txt) /Desc (Greeting) /EF << /F 9 0 R >> >>] >>",
        ],
        b"/Info 5 0 R",
    )


def test_reads_pages(sample: bytes) -> None:
    document = engine.Document(sample)
    assert document.version == "1.7"
    assert document.page_count == len(document) == 2
    first, second = document.pages
    assert first.media_box == first.crop_box == (0.0, 0.0, 612.0, 792.0)
    assert first.rotation == 0
    assert second.media_box == (0.0, 0.0, 200.0, 100.0)
    assert second.crop_box == (10.0, 10.0, 190.0, 90.0)
    assert second.rotation == 90
    assert "rotation=90" in repr(second)
    assert repr(document) == "Document(version=1.7, page_count=2)"


def test_reads_information_and_outline(sample: bytes) -> None:
    document = engine.Document(sample)
    assert document.information() == {"title": "Sample", "author": "Jørg"}
    outline = [(item.level, item.title, item.page) for item in document.outline()]
    assert outline == [(1, "Second page", 1), (1, "Web", None)]


def test_reads_links_and_attachments(sample: bytes) -> None:
    document = engine.Document(sample)
    to_page, to_web = document.links(0)
    assert (to_page.kind, to_page.page, to_page.uri) == ("page", 1, None)
    assert to_page.rectangle == (10.0, 20.0, 110.0, 40.0)
    assert (to_web.kind, to_web.uri, to_web.page) == ("uri", "https://example.org", None)
    assert to_web.file is None and to_web.name is None
    assert document.links(1) == [] == document.links(50)
    assert "example.org" in repr(to_web)

    (attachment,) = document.attachments()
    assert attachment.name == attachment.file_name == "hello.txt"
    assert attachment.description == "Greeting"
    assert document.stream(attachment.stream) == b"Hello!"


def test_reads_and_writes_destination_views(sample: bytes) -> None:
    document = engine.Document(sample)
    (second, web) = document.outline()
    assert second.view == engine.View("Fit")
    assert second.view.fit == "Fit" and second.view.left is None
    assert web.view is None
    to_page, to_web = document.links(0)
    assert to_page.view == engine.View("Fit") and to_web.view is None
    assert document.named_destination("nothing") is None

    corner = engine.View("XYZ", left=10, top=80, zoom=2)
    assert (corner.left, corner.top, corner.zoom, corner.area) == (10.0, 80.0, 2.0, None)
    area = engine.View("FitR", area=(50, 60, 20, 30))
    assert area.area == (20.0, 30.0, 50.0, 60.0)
    assert (area.left, area.top) == (20.0, 60.0)
    assert engine.View("XYZ", zoom=0).zoom is None
    assert "FitR" in repr(area)
    with pytest.raises(ValueError, match="unknown view"):
        engine.View("Zoom")
    with pytest.raises(ValueError, match="area"):
        engine.View("FitR")

    # Items from outline() are written back as they are; tuples may add a
    # view, and an item without one leads to the top of its page.
    document.set_outline(
        [*document.outline(), (2, "Corner", 0, corner), (1, "Area", 1, area), (1, "Top", 0)]
    )
    reopened = engine.Document(document.save())
    views = [(item.title, item.page, item.view) for item in reopened.outline()]
    assert views == [
        ("Second page", 1, engine.View("Fit")),
        ("Web", None, None),
        ("Corner", 0, corner),
        ("Area", 1, area),
        ("Top", 0, engine.View("XYZ", top=792)),
    ]


def test_redacts_an_area() -> None:
    content = b"BT /F1 10 Tf 100 700 Td (Hello World) Tj ET"
    data = build_pdf(
        [
            b"<< /Type /Catalog /Pages 2 0 R >>",
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>",
            b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R"
            b" /Resources << /Font << /F1 5 0 R >> >> >>",
            b"<< /Length %d >>\nstream\n%s\nendstream" % (len(content), content),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Plain >>",
        ]
    )
    document = engine.Document(data)
    # A font without widths that is not a standard one gets half its size
    # for each glyph: 5 points, so "World" starts at 130.
    result = document.redact(0, [(128, 695, 160, 710)])
    assert result == {"glyphs": 5, "images": 0, "graphics": 0, "kept_images": 0}
    assert document.page_text(0).strip() == "Hello"
    assert b"1 1 1 rg" in document.page_content(0)
    assert document.redact(0, [(0, 0, 10, 10)], fill=None)["glyphs"] == 0
    with pytest.raises(engine.PdfError, match="out of range"):
        document.redact(3, [])
    assert engine.Document(document.save()).page_text(0).strip() == "Hello"


def test_measures_and_inserts_standard_font_text(sample: bytes) -> None:
    # Helvetica's H 722, e 556, l 222, l 222, o 556 thousandths.
    assert engine.text_width("Hello", "helv", 10) == pytest.approx(22.78)
    assert engine.text_width("Hello", "Arial", 10) == engine.text_width("Hello", size=10)
    assert engine.text_width("abc", "cour", 10) == pytest.approx(18.0)
    with pytest.raises(ValueError, match="not a standard font"):
        engine.text_width("x", "Verdana")

    document = engine.Document(sample)
    width = document.insert_text(1, 20, 30, "Caf\u00e9", font="tiro", size=12, color=(1, 0, 0))
    assert width == pytest.approx(engine.text_width("Caf\u00e9", "Times-Roman", 12))
    assert document.page_text(1).strip() == "Caf\u00e9"
    (glyph, *_) = document.page_glyphs(1)
    assert glyph[:3] == ("C", 20.0, 30.0)
    (span,) = document.page_spans(1)
    assert (span[3], span[4], span[5]) == ("Times-Roman", 12.0, (255, 0, 0))
    with pytest.raises(engine.PdfError, match="out of range"):
        document.insert_text(5, 0, 0, "x")
    reopened = engine.Document(document.save())
    assert reopened.page_text(1).strip() == "Caf\u00e9"


def test_saves_with_or_without_encryption(sample: bytes) -> None:
    document = engine.Document(sample)
    assert document.encryption is None and document.permissions is None
    protected = document.save_encrypted("user-pass", "owner-pass", permissions=["print"])
    with pytest.raises(engine.PasswordRequired):
        engine.Document(protected)
    reopened = engine.Document(protected, password="user-pass")
    assert reopened.encryption == "Standard V5 R6 256-bit AES"
    assert reopened.permissions["print"] and not reopened.permissions["copy"]
    assert reopened.information()["title"] == "Sample"
    # Saving keeps the protection; saving decrypted removes it.
    with pytest.raises(engine.PasswordRequired):
        engine.Document(reopened.save())
    assert engine.Document(reopened.save(), password="owner-pass").page_count == 2
    plain = engine.Document(reopened.save_decrypted())
    assert plain.encryption is None and plain.page_count == 2
    with pytest.raises(ValueError, match="unknown permission"):
        document.save_encrypted("a", "b", permissions=["fly"])


def test_changes_embedded_files_and_xmp(sample: bytes) -> None:
    document = engine.Document(sample)
    assert document.attachment_data("hello.txt") == b"Hello!"
    document.add_attachment("notes.txt", b"my notes", description="Notes")
    assert document.remove_attachment("hello.txt")
    assert not document.remove_attachment("hello.txt")
    assert document.xmp_metadata() is None
    document.set_xmp_metadata("<x:xmpmeta/>")
    reopened = engine.Document(document.save())
    assert [file.name for file in reopened.attachments()] == ["notes.txt"]
    assert reopened.attachment_data("notes.txt") == b"my notes"
    assert reopened.attachment_data("hello.txt") is None
    assert reopened.xmp_metadata() == "<x:xmpmeta/>"
    reopened.set_xmp_metadata(None)
    assert reopened.xmp_metadata() is None


def test_builds_a_document_and_tells_passwords_apart() -> None:
    document = engine.Document.new()
    assert document.page_count == 0
    document.insert_blank_page(0, 200, 100)
    document.insert_blank_page(1, 200, 100)
    document.insert_text(0, 20, 60, "target page")
    document.add_link(0, (10, 10, 90, 30), "page", page=1, view=engine.View("XYZ", left=20, top=50))
    document.add_link(0, (10, 40, 90, 60), "uri", uri="https://example.com/spec")
    with pytest.raises(ValueError, match="needs a uri"):
        document.add_link(0, (0, 0, 1, 1), "uri")
    with pytest.raises(ValueError, match="unknown link kind"):
        document.add_link(0, (0, 0, 1, 1), "jump")
    document.set_information("Department", "Sales")
    protected = document.save_encrypted("user-pass", "owner-pass")
    with pytest.raises(engine.PasswordRequired) as missing:
        engine.Document(protected)
    assert not isinstance(missing.value, engine.WrongPassword)
    with pytest.raises(engine.WrongPassword):
        engine.Document(protected, password="nope")
    reopened = engine.Document(protected, password="user-pass")
    to_page, to_web = reopened.links(0)
    assert (to_page.kind, to_page.page, to_page.view.left, to_page.view.top) == ("page", 1, 20.0, 50.0)
    assert (to_web.kind, to_web.uri) == ("uri", "https://example.com/spec")
    assert reopened.information_entries() == {"Department": "Sales"}
    width, height, rgb = reopened.render_page(0, mode="RGB")
    assert (width, height, len(rgb)) == (200, 100, 200 * 100 * 3)
    assert len(reopened.render_page(0, mode="L")[2]) == 200 * 100
    with pytest.raises(ValueError, match="mode"):
        reopened.render_page(0, mode="CMYK")


def test_renders_a_region_of_a_page() -> None:
    content = (
        b"1 0 0 rg 12.5 7.5 60 50 re f 0 0 1 RG 3 w 5 90 m 190 10 l S"
        b" 0 0.6 0 rg 150 20 m 195 50 l 160 95 l h f"
    )
    data = build_pdf(
        [
            b"<< /Type /Catalog /Pages 2 0 R >>",
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 100] >>",
            b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>",
            b"<< /Length %d >>\nstream\n%s\nendstream" % (len(content), content),
        ]
    )
    document = engine.Document(data)
    width, height, rgb = document.render_page(0, mode="RGB")
    assert (width, height, len(rgb)) == (200, 100, 200 * 100 * 3)

    # A region holds the rows and columns of the whole image inside it.
    part = document.render_page_region(0, 1.0, 10, 20, 50, 30, mode="RGB")
    rows = [rgb[(y * 200 + 10) * 3 : (y * 200 + 60) * 3] for y in range(20, 50)]
    assert part == (50, 30, b"".join(rows))
    assert len(part[2]) == 4500
    # The region is not blank: it holds white, red, and the blue line.
    assert len(set(part[2][index : index + 3] for index in range(0, 4500, 3))) > 2

    # The same at another scale and in the other modes.
    for mode, size in (("RGBA", 4), ("RGB", 3), ("L", 1)):
        width, height, whole = document.render_page(0, 2.5, mode=mode)
        assert (width, height) == (500, 250)
        part = document.render_page_region(0, 2.5, 301, 99, 120, 77, mode)
        rows = [whole[(y * 500 + 301) * size : (y * 500 + 421) * size] for y in range(99, 176)]
        assert part == (120, 77, b"".join(rows))

    # One that reaches past the image is cut to it.
    width, height, gray = document.render_page_region(0, 1.0, 190, 95, 50, 30, mode="L")
    assert (width, height, len(gray)) == (10, 5, 50)
    assert len(document.render_page_region(0, 1.0, 190, 95, 50, 30)[2]) == 200
    # One with no pixel of the image is refused, as is an unknown mode.
    with pytest.raises(engine.PdfError, match="region"):
        document.render_page_region(0, 1.0, 200, 0, 5, 5)
    with pytest.raises(engine.PdfError, match="region"):
        document.render_page_region(0, 1.0, 0, 0, 0, 5)
    with pytest.raises(engine.PdfError, match="size"):
        document.render_page_region(0, 0.0, 0, 0, 5, 5)
    with pytest.raises(ValueError, match="mode"):
        document.render_page_region(0, 1.0, 0, 0, 5, 5, mode="CMYK")


def test_decodes_streams(sample: bytes) -> None:
    document = engine.Document(sample)
    assert document.stream(9) == b"Hello!"
    assert document.stream(9, generation=0) == b"Hello!"
    assert document.stream(1) is None
    assert document.stream(999) is None


def test_reads_page_content(sample: bytes) -> None:
    document = engine.Document(sample)
    assert document.page_content(0) == b"Hello!"
    assert document.page_content(1) == b"" == document.page_content(50)
    # The sample page's content has no text-showing operators.
    assert document.page_text(0) == "" == document.page_text(50)


def test_opens_from_a_path(sample: bytes, tmp_path) -> None:
    path = tmp_path / "sample.pdf"
    path.write_bytes(sample)
    assert engine.Document.open(path).page_count == 2
    assert engine.Document.open(str(path), password="unused").page_count == 2
    with pytest.raises(OSError, match="missing.pdf"):
        engine.Document.open(tmp_path / "missing.pdf")


def test_documents_from_one_file_follow_its_changes(sample: bytes, tmp_path) -> None:
    # Documents open together share the file's bytes; one opened after the
    # file changes reads the new bytes.
    path = tmp_path / "changing.pdf"
    path.write_bytes(sample)
    first = engine.Document.open(path)
    second = engine.Document.open(path)
    assert first.page_count == second.page_count == 2
    one_page = engine.Document(sample)
    one_page.select_pages([0])
    path.write_bytes(one_page.save())
    assert engine.Document.open(path).page_count == 1
    assert first.page_count == 2
    assert first.page(1).crop_box == first.pages[1].crop_box
    with pytest.raises(IndexError):
        first.page(2)


def test_reports_errors_as_exceptions(sample: bytes) -> None:
    assert issubclass(engine.PasswordRequired, engine.PdfError)
    with pytest.raises(engine.PdfError):
        engine.Document(b"not a pdf")
    with pytest.raises(TypeError):
        engine.Document("text is not bytes")
    damaged = sample.replace(b"/Filter /ASCIIHexDecode", b"/Filter /JPXDecode     ")
    with pytest.raises(engine.PdfError, match="unsupported"):
        engine.Document(damaged).stream(9)


def test_is_usable_from_several_threads(sample: bytes) -> None:
    document = engine.Document(sample)
    results = []

    def read() -> None:
        for _ in range(200):
            results.append((document.page_count, document.stream(9), len(document.outline())))

    threads = [threading.Thread(target=read) for _ in range(8)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()
    assert results == [(2, b"Hello!", 2)] * 1600


def test_a_same_size_change_with_the_same_time_is_read(sample: bytes, tmp_path) -> None:
    # Some file systems keep times to two seconds, so a file can change
    # without its size or time changing; its bytes must still be read.
    path = tmp_path / "same-size.pdf"
    path.write_bytes(sample)
    held = engine.Document.open(path)
    before = os.stat(path)
    changed = sample.replace(b"(Sample)", b"(Sampla)")
    assert len(changed) == len(sample)
    path.write_bytes(changed)
    os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))
    assert engine.Document.open(path).information()["title"] == "Sampla"
    assert held.information()["title"] == "Sample"
