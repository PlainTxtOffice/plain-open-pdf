"""Build and read PDF test fixtures with the app's PDF engine.

Points and boxes are in the app's page space: from the top left of the
unrotated page, with y growing downward.
"""

from __future__ import annotations

from contextlib import contextmanager
from typing import TYPE_CHECKING

import plain_open_pdf_engine

from plain_open_pdf.adapters.document.engine.document import EngineDocument

if TYPE_CHECKING:
    from collections.abc import Iterator, Sequence
    from pathlib import Path

#: The page size used when a fixture does not give one: A4, in points.
A4 = (595.0, 842.0)


class PdfBuilder:
    """A new PDF, built page by page."""

    def __init__(self) -> None:
        self.document = EngineDocument(plain_open_pdf_engine.Document.new())

    @property
    def native(self) -> plain_open_pdf_engine.Document:
        """Return the engine document being built."""
        return self.document.native

    def new_page(self, *, width: float = A4[0], height: float = A4[1]) -> int:
        """Append a blank page and return its index."""
        page_index = self.document.page_count
        self.native.insert_blank_page(page_index, width, height)
        return page_index

    def insert_text(
        self,
        page_index: int,
        point: tuple[float, float],
        text: str,
        *,
        size: float = 11.0,
    ) -> None:
        """Draw Helvetica text whose baseline starts at `point`."""
        x, y = self.document.to_pdf_space(page_index, *point)
        self.native.insert_text(page_index, x, y, text, size=size)

    def insert_uri_link(
        self,
        page_index: int,
        box: tuple[float, float, float, float],
        uri: str,
    ) -> None:
        """Add a link that opens `uri`."""
        rectangle = self.document.box_to_pdf_space(page_index, box)
        self.native.add_link(page_index, rectangle, "uri", uri=uri)

    def insert_page_link(
        self,
        page_index: int,
        box: tuple[float, float, float, float],
        *,
        target_page: int,
        point: tuple[float, float],
        zoom: float | None = None,
    ) -> None:
        """Add a link that shows `point` of another page at the top left."""
        left, top = self.document.to_pdf_space(target_page, *point)
        view = plain_open_pdf_engine.View("XYZ", left=left, top=top, zoom=zoom)
        rectangle = self.document.box_to_pdf_space(page_index, box)
        self.native.add_link(
            page_index,
            rectangle,
            "page",
            page=target_page,
            view=view,
        )

    def save(
        self,
        path: Path,
        *,
        user_password: str | None = None,
        owner_password: str | None = None,
    ) -> None:
        """Write the PDF, encrypted with AES-256 when given a password."""
        if user_password is None and owner_password is None:
            data = self.native.save()
        else:
            data = self.native.save_encrypted(
                user_password or "",
                owner_password or "",
            )
        path.write_bytes(data)


def create_pdf(
    path: Path,
    texts: Sequence[str] = ("",),
    *,
    width: float = A4[0],
    height: float = A4[1],
    point: tuple[float, float] = (72.0, 72.0),
) -> None:
    """Write a PDF with one page for each text, drawn at `point`."""
    builder = PdfBuilder()
    for text in texts:
        page_index = builder.new_page(width=width, height=height)
        if text:
            builder.insert_text(page_index, point, text)
    builder.save(path)


def open_pdf(
    path: Path,
    *,
    password: str | None = None,
) -> plain_open_pdf_engine.Document:
    """Open a saved PDF with the engine to check what it holds."""
    return plain_open_pdf_engine.Document.open(path, password=password)


@contextmanager
def edited_pdf(path: Path) -> Iterator[plain_open_pdf_engine.Document]:
    """Open a saved PDF and write it back after the block changes it."""
    document = open_pdf(path)
    yield document
    path.write_bytes(document.save())


def raw_pdf(objects: Sequence[str]) -> bytes:
    """Return a PDF holding `objects`, numbered from 1, with 1 the catalog."""
    output = bytearray(b"%PDF-1.7\n")
    offsets: list[int] = []
    for number, body in enumerate(objects, start=1):
        offsets.append(len(output))
        output += f"{number} 0 obj\n{body}\nendobj\n".encode("latin-1")
    xref_offset = len(output)
    output += f"xref\n0 {len(objects) + 1}\n0000000000 65535 f \n".encode()
    for offset in offsets:
        output += f"{offset:010d} 00000 n \n".encode()
    output += (
        f"trailer\n<< /Size {len(objects) + 1} /Root 1 0 R >>\n"
        f"startxref\n{xref_offset}\n%%EOF\n"
    ).encode()
    return bytes(output)


def create_named_destination_pdf(path: Path) -> None:
    """Write two A4 pages; a link on the first leads to `chapter-2`.

    - The named destination shows the second page with the point 20 points
      from its left and 250 points from its top at the window's top left.
    - The link covers the page-space box (72, 140, 152, 164).
    """
    path.write_bytes(
        raw_pdf(
            [
                "<< /Type /Catalog /Pages 2 0 R /Names << /Dests 3 0 R >> >>",
                "<< /Type /Pages /Kids [4 0 R 5 0 R] /Count 2 >>",
                "<< /Names [(chapter-2) [5 0 R /XYZ 20 592 0]] >>",
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842]"
                " /Annots [6 0 R] >>",
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] >>",
                "<< /Type /Annot /Subtype /Link /Rect [72 678 152 702]"
                " /Border [0 0 0] /Dest (chapter-2) >>",
            ],
        ),
    )
