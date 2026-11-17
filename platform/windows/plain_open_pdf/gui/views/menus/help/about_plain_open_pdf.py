"""About dialog helper for Plain Open PDF."""

from __future__ import annotations

from PySide6.QtWidgets import QMessageBox, QWidget


def show_about_plain_open_pdf_dialog(parent: QWidget) -> None:
    """Show the About Plain Open PDF dialog."""
    QMessageBox.about(
        parent,
        "About Plain Open PDF",
        "Plain Open PDF\n\nA lightweight desktop PDF viewer and editor.",
    )
