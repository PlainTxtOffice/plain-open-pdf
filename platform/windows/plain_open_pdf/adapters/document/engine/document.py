"""The open document and page objects the app holds, over the PDF engine."""

from __future__ import annotations

from dataclasses import dataclass
from functools import cache
from typing import TYPE_CHECKING

import plain_open_pdf_engine
from plain_open_pdf.config.path_handler import ASSETS_DIR

if TYPE_CHECKING:
    from pathlib import Path

#: Draws text whose own font is not embedded in the PDF.
_FALLBACK_FONT_PATH = ASSETS_DIR / "fonts" / "DejaVuSans.ttf"


@cache
def _fallback_font() -> bytes:
    """Return the fallback font's data, or nothing when it is missing."""
    try:
        return _FALLBACK_FONT_PATH.read_bytes()
    except OSError:
        return b""


@dataclass(frozen=True)
class PageRect:
    """A page's displayed size in points, after its rotation."""

    width: float
    height: float


@dataclass(frozen=True)
class RenderedPage:
    """Rendered page pixels: rows from the top without padding."""

    width: int
    height: int
    #: Bytes for each pixel: 3 for RGB.
    channels: int
    samples: bytes

    @property
    def stride(self) -> int:
        """Return the bytes in one row."""
        return self.width * self.channels


class EngineDocument:
    """An open PDF, wrapping the engine's document.

    - Coordinates given to and taken from the app follow the app's page
      space: points on the unrotated page, from the crop box's top left,
      with y growing downward.
    """

    def __init__(self, native: plain_open_pdf_engine.Document) -> None:
        """Wrap an opened engine document."""
        self._native = native
        self._closed = False
        font = _fallback_font()
        if font:
            native.set_fallback_font(font)

    @classmethod
    def open(cls, path: Path, *, password: str | None = None) -> EngineDocument:
        """Open the PDF stored at `path`."""
        return cls(plain_open_pdf_engine.Document.open(path, password=password))

    @classmethod
    def from_bytes(
        cls,
        data: bytes,
        *,
        password: str | None = None,
    ) -> EngineDocument:
        """Open a PDF held in memory."""
        return cls(plain_open_pdf_engine.Document(data, password=password))

    @property
    def native(self) -> plain_open_pdf_engine.Document:
        """Return the engine document, which must still be open."""
        if self._closed:
            msg = "The document is closed."
            raise ValueError(msg)
        return self._native

    @property
    def page_count(self) -> int:
        """Return the number of pages."""
        return int(self.native.page_count)

    @property
    def is_closed(self) -> bool:
        """Return whether the document was closed."""
        return self._closed

    def close(self) -> None:
        """Close the document; later use raises ValueError."""
        self._closed = True

    def load_page(self, page_index: int) -> EnginePage:
        """Return one page by its zero-based index."""
        if not 0 <= page_index < self.page_count:
            msg = f"Page index {page_index} is out of range."
            raise ValueError(msg)
        return EnginePage(self, page_index)

    def crop_box(self, page_index: int) -> tuple[float, float, float, float]:
        """Return a page's crop box as left, bottom, right, and top."""
        left, bottom, right, top = self.native.page(page_index).crop_box
        return (float(left), float(bottom), float(right), float(top))

    def to_page_space(
        self,
        page_index: int,
        x: float,
        y: float,
    ) -> tuple[float, float]:
        """Convert a PDF point into the app's page space."""
        left, _bottom, _right, top = self.crop_box(page_index)
        return (x - left, top - y)

    def to_pdf_space(
        self,
        page_index: int,
        x: float,
        y: float,
    ) -> tuple[float, float]:
        """Convert a point of the app's page space into a PDF point."""
        left, _bottom, _right, top = self.crop_box(page_index)
        return (x + left, top - y)

    def box_to_page_space(
        self,
        page_index: int,
        box: tuple[float, float, float, float],
    ) -> tuple[float, float, float, float]:
        """Convert a PDF rectangle into the app's page space."""
        left, bottom, right, top = box
        x0, y0 = self.to_page_space(page_index, left, top)
        x1, y1 = self.to_page_space(page_index, right, bottom)
        return (min(x0, x1), min(y0, y1), max(x0, x1), max(y0, y1))

    def box_to_pdf_space(
        self,
        page_index: int,
        box: tuple[float, float, float, float],
    ) -> tuple[float, float, float, float]:
        """Convert a rectangle of the app's page space into PDF space."""
        x0, y0, x1, y1 = box
        left, top = self.to_pdf_space(page_index, x0, y0)
        right, bottom = self.to_pdf_space(page_index, x1, y1)
        return (
            min(left, right),
            min(bottom, top),
            max(left, right),
            max(bottom, top),
        )


@dataclass(frozen=True)
class EnginePage:
    """One page of an open document."""

    document: EngineDocument
    index: int

    @property
    def rect(self) -> PageRect:
        """Return the page's displayed size, after its rotation."""
        size = self.document.native.view_size(self.index)
        if size is None:
            return PageRect(0.0, 0.0)
        return PageRect(size[0], size[1])

    @property
    def rotation(self) -> int:
        """Return the page's clockwise rotation in degrees."""
        return int(self.document.native.page(self.index).rotation)
