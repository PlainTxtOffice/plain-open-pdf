"""Text extraction and search with the PDF engine."""

from __future__ import annotations

from typing import TYPE_CHECKING

from plain_open_pdf.domains.document.models import (
    PdfSearchMatch,
    PdfTextSpan,
)

if TYPE_CHECKING:
    from plain_open_pdf.adapters.document.engine.document import (
        EngineDocument,
        EnginePage,
    )
    from plain_open_pdf.domains.document.models import Rectangle


def extract_page_text(page: EnginePage) -> str:
    """Return plain text extracted from a page."""
    return str(page.document.native.page_text(page.index))


def extract_page_words(
    page: EnginePage,
) -> list[tuple[float, float, float, float, str, int, int, int]]:
    """Return word boxes in page space with their block, line, and word."""
    document = page.document
    words: list[tuple[float, float, float, float, str, int, int, int]] = []
    for word in document.native.page_words(page.index):
        left, bottom, right, top, text, block, line, number = word
        if not text:
            continue
        x0, y0, x1, y1 = document.box_to_page_space(
            page.index,
            (left, bottom, right, top),
        )
        words.append((x0, y0, x1, y1, str(text), block, line, number))
    return words


def search_page(page: EnginePage, query: str) -> tuple[Rectangle, ...]:
    """Return the rectangles of every match on a page, in page space."""
    document = page.document
    return tuple(
        document.box_to_page_space(page.index, rectangle)
        for hit in document.native.search_page(page.index, query)
        for rectangle in hit
    )


def search_document(
    document: EngineDocument,
    query: str,
) -> list[PdfSearchMatch]:
    """Return text matches across a document."""
    normalized_query = query.strip()
    if not normalized_query:
        return []

    matches: list[PdfSearchMatch] = []
    for page_index in range(document.page_count):
        rectangles = search_page(
            document.load_page(page_index), normalized_query
        )
        if rectangles:
            matches.append(
                PdfSearchMatch(page_index=page_index, rectangles=rectangles),
            )
    return matches


def page_text_spans(page: EnginePage, page_index: int) -> list[PdfTextSpan]:
    """Return every styled text span on a page, in reading order."""
    document = page.document
    spans: list[PdfTextSpan] = []
    for text, box, origin, font, size, color in document.native.page_spans(
        page_index,
    ):
        if not str(text).strip():
            continue
        red, green, blue = color
        spans.append(
            PdfTextSpan(
                page_index=page_index,
                span_index=len(spans),
                text=str(text),
                bounds=document.box_to_page_space(page_index, box),
                origin=document.to_page_space(page_index, *origin),
                font_name=str(font),
                font_size=float(size),
                color=(int(red) << 16) | (int(green) << 8) | int(blue),
            ),
        )
    return spans
