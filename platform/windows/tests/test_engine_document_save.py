"""File-safety tests for the engine save adapters."""

from pathlib import Path

import pytest

from engine_pdfs import create_pdf
from plain_open_pdf.adapters.document.engine import lifecycle
from plain_open_pdf.adapters.document.engine.document import EngineDocument
from plain_open_pdf.adapters.document.engine.lifecycle import (
    open_document,
    save_document_copy,
    save_document_in_place,
)


def _install_failing_write(monkeypatch: pytest.MonkeyPatch) -> None:
    """Make document writes simulate a mid-write failure at their target."""

    def _failing_write(document: EngineDocument, path: Path) -> None:
        del document
        path.write_bytes(b"%PDF-1.7 partial garbage")
        msg = "simulated mid-write failure"
        raise RuntimeError(msg)

    monkeypatch.setattr(lifecycle, "_write_document", _failing_write)


def test_save_document_copy_keeps_existing_target_when_write_fails(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    source_path = tmp_path / "source.pdf"
    target_path = tmp_path / "target.pdf"
    create_pdf(source_path, ["source content"])
    create_pdf(target_path, ["precious existing target"])
    original_target_bytes = target_path.read_bytes()

    document = open_document(source_path)
    _install_failing_write(monkeypatch)
    with pytest.raises(RuntimeError, match="simulated mid-write failure"):
        save_document_copy(document, target_path)

    assert target_path.read_bytes() == original_target_bytes
    assert list(tmp_path.glob("*.tmp")) == []


def test_save_document_in_place_recovers_from_non_oserror_write_failure(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    pdf_path = tmp_path / "document.pdf"
    create_pdf(pdf_path, ["original content"])
    original_bytes = pdf_path.read_bytes()

    document = open_document(pdf_path)
    _install_failing_write(monkeypatch)
    with pytest.raises(RuntimeError, match="simulated mid-write failure"):
        save_document_in_place(document, pdf_path)

    assert document.is_closed is False
    assert pdf_path.read_bytes() == original_bytes
    assert list(tmp_path.glob("*.tmp")) == []
    assert list(tmp_path.glob("*.bak")) == []


def test_save_document_in_place_replaces_the_file(tmp_path: Path) -> None:
    pdf_path = tmp_path / "rotated.pdf"
    create_pdf(pdf_path, ["rotate me"])

    document = open_document(pdf_path)
    document.native.set_page_rotation(0, 90)
    save_document_in_place(document, pdf_path)

    assert document.is_closed is True
    reopened = open_document(pdf_path)
    assert reopened.load_page(0).rotation == 90
    assert list(tmp_path.glob("*.bak")) == []
