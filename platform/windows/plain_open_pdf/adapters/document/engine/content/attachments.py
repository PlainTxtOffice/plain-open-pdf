"""Document-level embedded text attachments, with the PDF engine."""

from __future__ import annotations

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from plain_open_pdf.adapters.document.engine.document import EngineDocument


def text_attachment_exists(document: EngineDocument, *, name: str) -> bool:
    """Return whether the document contains the named attachment."""
    return any(
        attachment.name == name for attachment in document.native.attachments()
    )


def read_text_attachment(document: EngineDocument, *, name: str) -> str:
    """Return one UTF-8 text attachment, or an empty string if absent."""
    data = document.native.attachment_data(name)
    if data is None:
        return ""
    return bytes(data).decode("utf-8", errors="replace")


def write_text_attachment(
    document: EngineDocument,
    *,
    name: str,
    text: str,
    description: str,
) -> None:
    """Create, update, or remove one UTF-8 text attachment."""
    if not text:
        document.native.remove_attachment(name)
        return

    document.native.add_attachment(
        name,
        text.encode("utf-8"),
        file_name=name,
        description=description,
    )
