"""Adapters that turn rendered pages into Qt objects."""

from __future__ import annotations

from typing import TYPE_CHECKING

from PySide6.QtGui import QImage, QPixmap

if TYPE_CHECKING:
    from plain_open_pdf.adapters.document.engine.document import RenderedPage


def qimage_from_render(rendered_page: RenderedPage) -> QImage:
    """Convert rendered page pixels into a detached Qt image."""
    image_format = QImage.Format.Format_RGB888
    if rendered_page.channels == 1:
        image_format = QImage.Format.Format_Grayscale8

    image = QImage(
        rendered_page.samples,
        rendered_page.width,
        rendered_page.height,
        rendered_page.stride,
        image_format,
    )
    return image.copy()


def pixmap_from_render(rendered_page: RenderedPage) -> QPixmap:
    """Convert rendered page pixels into a detached Qt pixmap."""
    return QPixmap.fromImage(qimage_from_render(rendered_page))
