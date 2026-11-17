"""Page and text editing with the PDF engine."""

from __future__ import annotations

from typing import TYPE_CHECKING

import plain_open_pdf_engine

if TYPE_CHECKING:
    from collections.abc import Sequence

    from plain_open_pdf.adapters.document.engine.document import EngineDocument
    from plain_open_pdf.domains.document.models import PdfTextSpan


_FALLBACK_FONT = "helv"
_MIN_REPLACEMENT_FONT_SIZE = 4.0
_FONT_SIZE_STEP = 0.5
_QUARTER_TURN = 90
_FULL_TURN = 360


def _replacement_font_name(font_name: str) -> str:
    """Return a standard font usable for arbitrary replacement characters."""
    base_14 = ("helv", "tiro", "cour", "times", "symb", "zadb")
    normalized = font_name.rsplit("+", maxsplit=1)[-1].casefold()
    for candidate in base_14:
        if normalized.startswith(candidate):
            return candidate
    return _FALLBACK_FONT


def insert_blank_document_page(
    document: EngineDocument,
    *,
    page_index: int,
    size: tuple[float, float],
) -> None:
    """Insert an empty page of `size` at `page_index`."""
    width, height = size
    document.native.insert_blank_page(page_index, width, height)


def delete_document_page(document: EngineDocument, page_index: int) -> None:
    """Delete one page from the active document."""
    kept = [
        index for index in range(document.page_count) if index != page_index
    ]
    document.native.select_pages(kept)


def move_document_page(
    document: EngineDocument,
    page_index: int,
    destination_index: int,
) -> None:
    """Move one page to another zero-based page position."""
    order = list(range(document.page_count))
    moved = order.pop(page_index)
    order.insert(destination_index, moved)
    document.native.select_pages(order)


def redact_page_rectangles(
    document: EngineDocument,
    page_index: int,
    rectangles: Sequence[tuple[float, float, float, float]],
) -> bool:
    """Redact page content inside the supplied page-space rectangles."""
    areas = []
    for x0, y0, x1, y1 in rectangles:
        if min(x0, x1) == max(x0, x1) or min(y0, y1) == max(y0, y1):
            continue
        areas.append(document.box_to_pdf_space(page_index, (x0, y0, x1, y1)))
    if not areas:
        return False

    document.native.redact(page_index, areas, fill=(1.0, 1.0, 1.0))
    return True


def replace_text_span(
    document: EngineDocument,
    *,
    span: PdfTextSpan,
    text: str,
) -> bool:
    """Erase the text inside a span's bounds and draw `text` on its baseline.

    - Stays on one line: PDF has no reflow, so wrapping would collide with
      the line beneath. Longer text shrinks to fit the original width,
      down to a floor, and is allowed to run on if it still will not fit.

    Returns:
        True when the page was modified.

    """
    x0, y0, x1, y1 = span.bounds
    if x1 <= x0 or y1 <= y0:
        return False

    if not redact_page_rectangles(document, span.page_index, [span.bounds]):
        return False
    if not text:
        return True

    replacement_font = _replacement_font_name(span.font_name)
    rgb = (
        ((span.color >> 16) & 0xFF) / 255.0,
        ((span.color >> 8) & 0xFF) / 255.0,
        (span.color & 0xFF) / 255.0,
    )
    available_width = x1 - x0
    size = span.font_size
    while size > _MIN_REPLACEMENT_FONT_SIZE:
        width = plain_open_pdf_engine.text_width(text, replacement_font, size)
        if width <= available_width:
            break
        size -= _FONT_SIZE_STEP

    x, y = document.to_pdf_space(span.page_index, *span.origin)
    document.native.insert_text(
        span.page_index,
        x,
        y,
        text,
        font=replacement_font,
        size=size,
        color=rgb,
    )
    return True


def rotate_page_clockwise(document: EngineDocument, page_index: int) -> None:
    """Rotate one page by ninety degrees."""
    rotation = document.load_page(page_index).rotation
    document.native.set_page_rotation(
        page_index,
        (rotation + _QUARTER_TURN) % _FULL_TURN,
    )
