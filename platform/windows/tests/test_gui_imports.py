"""Structural guards over the GUI layer's outward dependencies."""

from __future__ import annotations

import ast
from pathlib import Path

GUI_ROOT = Path("plain_open_pdf/gui")

# Modules that are allowed to reach past the GUI layer into an adapter or
# straight into the PDF engine, as (module path, imported module) pairs.
#
# Keep this list exact. Adding a pair is a deliberate decision to widen the
# GUI layer's contact surface with the rest of the app, and
# `test_no_stale_integration_allowances` fails when a pair stops matching so
# that moving a module cannot silently retire its guard.
ALLOWED_INTEGRATION_IMPORTS = {
    ('plain_open_pdf/gui/ocr/task.py', 'plain_open_pdf.adapters.document.engine.processing'),
    ('plain_open_pdf/gui/ocr/task.py', 'plain_open_pdf.adapters.ocr.backend'),
    ('plain_open_pdf/gui/ocr/workflow.py', 'plain_open_pdf.adapters.ocr.backend'),
    ('plain_open_pdf/gui/ocr/workflow.py', 'plain_open_pdf.adapters.ocr.results'),
    ('plain_open_pdf/gui/views/rendering/page_render_cache.py', 'plain_open_pdf.adapters.document.errors'),
    ('plain_open_pdf/gui/views/rendering/page_render_cache.py', 'plain_open_pdf.adapters.document.engine.qt'),
    ('plain_open_pdf/gui/views/rendering/page_rendering.py', 'plain_open_pdf.adapters.document.errors'),
    ('plain_open_pdf/gui/views/rendering/page_rendering.py', 'plain_open_pdf.adapters.document.engine.qt'),
    ('plain_open_pdf/gui/views/rendering/page_rendering.py', 'plain_open_pdf.adapters.document.engine.rendering'),
    ('plain_open_pdf/gui/views/rendering/render_handle_cache.py', 'plain_open_pdf.adapters.document.engine.document'),
    ('plain_open_pdf/gui/views/rendering/render_handle_cache.py', 'plain_open_pdf.adapters.document.engine.lifecycle'),
    ('plain_open_pdf/gui/views/rendering/selection.py', 'plain_open_pdf.adapters.document.errors'),
    ('plain_open_pdf/gui/views/rendering/thumbnail_rendering.py', 'plain_open_pdf.adapters.document.errors'),
    ('plain_open_pdf/gui/views/rendering/thumbnail_rendering.py', 'plain_open_pdf.adapters.document.engine.qt'),
    ('plain_open_pdf/gui/views/rendering/thumbnail_rendering.py', 'plain_open_pdf.adapters.document.engine.rendering'),
    ('plain_open_pdf/gui/views/rendering/zoom/preview.py', 'plain_open_pdf.adapters.document.engine.rendering'),
    ('plain_open_pdf/gui/views/root_window/document_sync.py', 'plain_open_pdf.adapters.document.errors'),
    ('plain_open_pdf/gui/views/root_window/navigation/bookmarks.py', 'plain_open_pdf.adapters.document.errors'),
    ('plain_open_pdf/gui/views/root_window/navigation/page_links.py', 'plain_open_pdf.adapters.document.errors'),
    ('plain_open_pdf/gui/views/root_window/panes/thumbnails.py', 'plain_open_pdf.adapters.document.errors'),
    ('plain_open_pdf/gui/views/root_window/panes/thumbnails.py', 'plain_open_pdf.adapters.document.engine.qt'),
    ('plain_open_pdf/gui/views/root_window/persistence.py', 'plain_open_pdf.adapters.persistence.desktop'),
    ('plain_open_pdf/gui/views/root_window/window.py', 'plain_open_pdf.adapters.document.errors'),
    ('plain_open_pdf/gui/views/root_window/window.py', 'plain_open_pdf.adapters.ocr.backend'),
    ('plain_open_pdf/gui/views/root_window/window.py', 'plain_open_pdf.adapters.ocr.results'),
    ('plain_open_pdf/gui/views/root_window/window.py', 'plain_open_pdf.adapters.persistence.desktop'),
    ('plain_open_pdf/gui/views/search/search_navigation.py', 'plain_open_pdf.adapters.document.errors'),
    ('plain_open_pdf/gui/views/search/search_task.py', 'plain_open_pdf.adapters.document.errors'),
    ('plain_open_pdf/gui/views/search/search_task.py', 'plain_open_pdf.adapters.document.engine.processing'),
    ('plain_open_pdf/gui/views/workflows/printing.py', 'plain_open_pdf.adapters.document.errors'),
    ('plain_open_pdf/gui/views/workflows/printing.py', 'plain_open_pdf.adapters.document.engine.qt'),
    ('plain_open_pdf/gui/views/workflows/text_editing.py', 'plain_open_pdf.adapters.document.errors'),
}


def _imported_modules(module_path: Path) -> list[str]:
    """Return every module name imported by one source file."""
    tree = ast.parse(module_path.read_text(encoding="utf-8"))
    imported: list[str] = []
    for node in ast.walk(tree):
        if isinstance(node, ast.ImportFrom) and node.module is not None:
            imported.append(node.module)
        elif isinstance(node, ast.Import):
            imported.extend(alias.name for alias in node.names)
    return imported


def _is_integration_import(module: str) -> bool:
    """Report whether an import crosses out of the GUI layer."""
    return module == "plain_open_pdf_engine" or module.startswith(
        "plain_open_pdf.adapters.",
    )


def _actual_integration_imports() -> set[tuple[str, str]]:
    """Collect every outward import the GUI layer makes today."""
    return {
        (module_path.as_posix(), imported)
        for module_path in GUI_ROOT.rglob("*.py")
        for imported in _imported_modules(module_path)
        if _is_integration_import(imported)
    }


def test_app_is_the_only_direct_gui_module() -> None:
    direct_modules = sorted(
        path.name for path in GUI_ROOT.glob("*.py") if path.name != "__init__.py"
    )

    assert direct_modules == ["app.py"]


def test_gui_components_do_not_import_composition_root() -> None:
    importing_modules = sorted(
        module_path.as_posix()
        for module_path in GUI_ROOT.rglob("*.py")
        if module_path.name != "app.py"
        and "plain_open_pdf.gui.app" in _imported_modules(module_path)
    )

    assert importing_modules == []


def test_gui_adapter_imports_are_explicit() -> None:
    unexpected = sorted(
        _actual_integration_imports() - ALLOWED_INTEGRATION_IMPORTS
    )

    assert unexpected == [], (
        "GUI modules reached outside the GUI layer without an allowance. "
        "Route the call through an existing seam, or add the pair to "
        "ALLOWED_INTEGRATION_IMPORTS deliberately: "
        f"{unexpected}"
    )


def test_app_does_not_import_pymupdf() -> None:
    """The app reads PDFs with its own engine, never PyMuPDF or MuPDF."""
    importing_modules = sorted(
        module_path.as_posix()
        for module_path in Path("plain_open_pdf").rglob("*.py")
        for imported in _imported_modules(module_path)
        if imported.split(".")[0] in {"pymupdf", "fitz"}
    )

    assert importing_modules == []


def test_no_stale_integration_allowances() -> None:
    """Fail when an allowance no longer matches a real import.

    A stale entry means a module moved or dropped the import, and the guard
    silently stopped protecting anything. Without this check the allowlist
    rots into a list of paths that no longer exist.
    """
    stale = sorted(ALLOWED_INTEGRATION_IMPORTS - _actual_integration_imports())

    assert stale == [], (
        "ALLOWED_INTEGRATION_IMPORTS lists imports that no longer exist. "
        "Remove or repoint these entries: "
        f"{stale}"
    )
