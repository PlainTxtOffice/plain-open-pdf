"""Describe the engine-facing OCR integration contract."""

from __future__ import annotations

from typing import TYPE_CHECKING, Protocol

if TYPE_CHECKING:
    from plain_open_pdf.adapters.document.engine.document import (
        EngineDocument,
    )
    from plain_open_pdf.domains.document.models import PdfOcrPageResult


class PdfOcrBackend(Protocol):
    """Recognize text for one page of the active PDF document."""

    def recognize_page(
        self,
        document: EngineDocument,
        page_index: int,
        *,
        language_codes: tuple[str, ...],
    ) -> PdfOcrPageResult:
        """Return OCR output for one page."""
        ...
