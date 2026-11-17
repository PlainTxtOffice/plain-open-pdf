"""Opening, closing, and saving documents with the PDF engine."""

from __future__ import annotations

from contextlib import suppress
from typing import TYPE_CHECKING
from uuid import uuid4

import plain_open_pdf_engine
from plain_open_pdf.adapters.document.engine.document import EngineDocument
from plain_open_pdf.domains.document.errors import (
    IncorrectPasswordError,
    PasswordRequiredError,
)

if TYPE_CHECKING:
    from pathlib import Path

    from plain_open_pdf.adapters.document.engine.document import EnginePage


def _temporary_pdf_path(path: Path) -> Path:
    while True:
        temporary_path = path.with_name(
            f".{path.stem}.{uuid4().hex}{path.suffix}.tmp",
        )
        if not temporary_path.exists():
            return temporary_path


def _backup_pdf_path(path: Path) -> Path:
    while True:
        backup_path = path.with_name(
            f".{path.stem}.{uuid4().hex}{path.suffix}.bak",
        )
        if not backup_path.exists():
            return backup_path


def open_document(
    path: Path,
    *,
    password: str | None = None,
) -> EngineDocument:
    """Open a PDF document from disk.

    Raises:
        PasswordRequiredError: The PDF needs a password and none was given.
        IncorrectPasswordError: The password given is wrong.

    """
    try:
        return EngineDocument.open(path, password=password)
    except plain_open_pdf_engine.PasswordRequired as error:
        if password is not None:
            msg = "The provided PDF password is incorrect."
            raise IncorrectPasswordError(msg) from error
        msg = "This PDF is password-protected and requires a password to open."
        raise PasswordRequiredError(msg) from error


def close_document(document: EngineDocument | None) -> None:
    """Close a document when one is open."""
    if document is not None:
        document.close()


def _write_document(document: EngineDocument, path: Path) -> None:
    """Write the document to `path`, keeping its encryption."""
    path.write_bytes(document.native.save())


def save_document_copy(document: EngineDocument, path: Path) -> None:
    """Write document changes to a target path via an atomic replace."""
    temporary_path = _temporary_pdf_path(path)
    try:
        _write_document(document, temporary_path)
        temporary_path.replace(path)
    except Exception:
        with suppress(OSError):
            temporary_path.unlink(missing_ok=True)
        raise


def save_document_in_place(document: EngineDocument, path: Path) -> None:
    """Write document changes back to the opened file via a safe replace."""
    temporary_path = _temporary_pdf_path(path)
    backup_path = _backup_pdf_path(path)
    source_backed_up = False
    try:
        _write_document(document, temporary_path)
        document.close()
        path.replace(backup_path)
        source_backed_up = True
        temporary_path.replace(path)
    except Exception:
        with suppress(OSError):
            temporary_path.unlink(missing_ok=True)
        if source_backed_up and backup_path.exists() and not path.exists():
            backup_path.replace(path)
        raise
    else:
        backup_path.unlink(missing_ok=True)


def load_document_page(document: EngineDocument, page_index: int) -> EnginePage:
    """Load an engine page for an application workflow."""
    return document.load_page(page_index)
