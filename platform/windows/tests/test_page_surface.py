import os
import subprocess
import sys
from pathlib import Path

import pytest

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

from PySide6.QtCore import QEvent, QPoint, QPointF, QRect, QRectF, Qt
from PySide6.QtGui import QColor, QImage, QMouseEvent, QPixmap
from PySide6.QtWidgets import QApplication, QWidget

from plain_open_pdf.domains.document.models import PdfPageLink
from plain_open_pdf.gui.views.rendering.page_geometry import (
    _CURRENT_PAGE_BORDER_STYLE,
    _DEFAULT_PAGE_BORDER_STYLE,
)
from plain_open_pdf.gui.views.rendering.page_metrics import PAGE_CONTENT_CHROME
from plain_open_pdf.gui.views.rendering.page_surface import PageSurfaceLabel

_INSET = PAGE_CONTENT_CHROME // 2
_RED = QColor("red")
_WHITE = QColor("white")


def _app() -> QApplication:
    app = QApplication.instance()
    if app is None:
        app = QApplication([])
    return app


def _render_surface(surface: PageSurfaceLabel) -> QImage:
    image = QImage(surface.size(), QImage.Format.Format_ARGB32)
    image.fill(QColor("white"))
    surface.render(image)
    return image


def _send_mouse_event(
    widget: QWidget,
    event_type: QEvent.Type,
    position: QPoint,
    button: Qt.MouseButton = Qt.MouseButton.NoButton,
) -> None:
    """Deliver a mouse event straight to `widget`.

    `QTest.mouseMove` and `QTest.mouseClick` inject events through the window
    system using *global* coordinates. Under the offscreen platform every
    top-level widget is placed at the same geometry, so the window shown
    first swallows the event and the widget under test never receives it.
    That made this test pass alone and fail after any earlier test had shown
    a window. Sending directly to the widget keeps delivery deterministic
    while still exercising the real event handlers.
    """
    buttons = (
        Qt.MouseButton.NoButton
        if event_type == QEvent.Type.MouseMove
        else button
    )
    event = QMouseEvent(
        event_type,
        QPointF(position),
        widget.mapToGlobal(QPointF(position)),
        button,
        buttons,
        Qt.KeyboardModifier.NoModifier,
    )
    QApplication.sendEvent(widget, event)


def test_text_overlay_hover_shows_link_feedback_and_keeps_clickable() -> None:
    app = _app()
    link = PdfPageLink(
        bounds=(20.0, 30.0, 140.0, 55.0),
        target_page_index=1,
    )
    activated_links: list[PdfPageLink] = []
    surface = PageSurfaceLabel()
    pixmap = QPixmap(200, 120)
    pixmap.fill(QColor("white"))
    surface.setPixmap(pixmap)
    surface.resize(204, 124)
    surface.set_page_links([link])
    surface.set_text_selection_enabled(enabled=True)
    surface.link_activated.connect(activated_links.append)
    surface.show()
    app.processEvents()

    before_hover = _render_surface(surface)
    _send_mouse_event(
        surface.text_layer(),
        QEvent.Type.MouseMove,
        QPoint(80, 42),
    )
    app.processEvents()
    after_hover = _render_surface(surface)

    assert (
        surface.text_layer().cursor().shape()
        == Qt.CursorShape.PointingHandCursor
    )
    assert before_hover != after_hover

    _send_mouse_event(
        surface.text_layer(),
        QEvent.Type.MouseButtonPress,
        QPoint(80, 42),
        Qt.MouseButton.LeftButton,
    )
    _send_mouse_event(
        surface.text_layer(),
        QEvent.Type.MouseButtonRelease,
        QPoint(80, 42),
        Qt.MouseButton.LeftButton,
    )
    app.processEvents()

    assert activated_links == [link]


def _surface_with_sharp_region() -> PageSurfaceLabel:
    """Return a white 200x120 page with a red region at (20, 30, 50, 40)."""
    surface = PageSurfaceLabel()
    surface.setAlignment(Qt.AlignmentFlag.AlignCenter)
    base = QPixmap(200, 120)
    base.fill(_WHITE)
    surface.setPixmap(base)
    surface.setFixedSize(
        200 + PAGE_CONTENT_CHROME,
        120 + PAGE_CONTENT_CHROME,
    )
    region = QPixmap(50, 40)
    region.fill(_RED)
    surface.set_sharp_region(region, rect=QRectF(20, 30, 50, 40), zoom=1.0)
    return surface


def _content_color(image: QImage, x: int, y: int) -> QColor:
    return image.pixelColor(_INSET + x, _INSET + y)


def test_sharp_region_paints_inside_its_rectangle_only() -> None:
    _app()
    surface = _surface_with_sharp_region()
    size_before = surface.size()

    image = _render_surface(surface)

    assert surface.sharp_region() == QRectF(20, 30, 50, 40)
    assert surface.size() == size_before
    assert _content_color(image, 20, 30) == _RED
    assert _content_color(image, 69, 69) == _RED
    assert _content_color(image, 19, 30) == _WHITE
    assert _content_color(image, 20, 29) == _WHITE
    assert _content_color(image, 70, 50) == _WHITE
    assert _content_color(image, 40, 70) == _WHITE


def test_sharp_region_uses_the_pixel_ratio_of_its_pixmap() -> None:
    _app()
    surface = _surface_with_sharp_region()
    region = QPixmap(100, 80)
    region.fill(_RED)
    region.setDevicePixelRatio(2.0)
    surface.set_sharp_region(region, rect=QRectF(20, 30, 50, 40), zoom=1.0)

    image = _render_surface(surface)

    assert _content_color(image, 21, 31) == _RED
    assert _content_color(image, 68, 68) == _RED
    assert _content_color(image, 72, 50) == _WHITE
    assert _content_color(image, 40, 72) == _WHITE


# Paints a patterned region on a page label inside a parent widget and
# checks that the parent shows every pixel of the pattern exactly once,
# for several label positions. It runs in its own process because the
# screen's pixel ratio is fixed when the application starts.
_PIXEL_GRID_SCRIPT = """
import sys

from PySide6.QtCore import QRect, QRectF
from PySide6.QtGui import QImage, QPixmap
from PySide6.QtWidgets import QApplication, QWidget

from plain_open_pdf.gui.views.rendering.page_surface import PageSurfaceLabel

ratio = float(sys.argv[1])
app = QApplication([])
assert abs(app.devicePixelRatio() - ratio) < 1e-6, app.devicePixelRatio()
width, height = 1280, 384
samples = bytearray(width * height * 4)
for y in range(height):
    for x in range(width):
        at = (y * width + x) * 4
        samples[at] = (x * 37) & 255
        samples[at + 1] = (y * 91) & 255
        samples[at + 2] = (x + y) & 255
        samples[at + 3] = 255
pattern = QImage(
    bytes(samples), width, height, width * 4, QImage.Format.Format_RGB32
).copy()
pattern.setDevicePixelRatio(ratio)
parent = QWidget()
parent.resize(900, 400)
surface = PageSurfaceLabel()
surface.setParent(parent)
surface.setFixedSize(820, 320)
left, top = 64, 128
surface.set_sharp_region(
    QPixmap.fromImage(pattern.copy(left, top, width - left, height - top)),
    rect=QRectF(
        left / ratio,
        top / ratio,
        (width - left) / ratio,
        (height - top) / ratio,
    ),
    zoom=1.0,
)
inside_x = int(left / ratio) + 12
inside_y = int(top / ratio) + 12
for position in ((24, 12), (25, 13), (26, 14), (27, 15)):
    surface.move(*position)
    shown = parent.grab(
        QRect(position[0] + inside_x, position[1] + inside_y, 600, 100)
    ).toImage().convertToFormat(QImage.Format.Format_RGB32)
    matches = [
        (x, y)
        for y in range(64)
        for x in range(64)
        if shown == pattern.copy(
            left + x, top + y, shown.width(), shown.height()
        ).convertToFormat(QImage.Format.Format_RGB32)
    ]
    if len(matches) != 1:
        print("region pixels were resampled at label position", position)
        sys.exit(1)
"""


@pytest.mark.parametrize("pixel_ratio", ["1.25", "1.5", "1.75"])
def test_sharp_region_keeps_its_pixels_on_a_fractional_screen(
    pixel_ratio: str,
) -> None:
    result = subprocess.run(
        [sys.executable, "-c", _PIXEL_GRID_SCRIPT, pixel_ratio],
        capture_output=True,
        check=False,
        cwd=Path(__file__).resolve().parents[1],
        env={
            **os.environ,
            "QT_QPA_PLATFORM": "offscreen",
            "QT_SCALE_FACTOR": pixel_ratio,
            "QT_SCALE_FACTOR_ROUNDING_POLICY": "PassThrough",
        },
        text=True,
        timeout=180,
    )

    assert result.returncode == 0, result.stdout + result.stderr


def test_sharp_region_stretches_with_the_page_zoom() -> None:
    _app()
    surface = _surface_with_sharp_region()

    surface.set_page_zoom(zoom=2.0)
    image = _render_surface(surface)

    assert _content_color(image, 30, 40) == _WHITE
    assert _content_color(image, 42, 62) == _RED
    assert _content_color(image, 137, 110) == _RED
    assert _content_color(image, 143, 100) == _WHITE


def test_clear_sharp_region_removes_it() -> None:
    _app()
    surface = _surface_with_sharp_region()

    surface.clear_sharp_region()
    image = _render_surface(surface)

    assert surface.sharp_region() is None
    assert _content_color(image, 40, 50) == _WHITE


def test_links_still_work_with_a_sharp_region() -> None:
    app = _app()
    link = PdfPageLink(
        bounds=(20.0, 30.0, 140.0, 55.0),
        target_page_index=1,
    )
    activated_links: list[PdfPageLink] = []
    surface = _surface_with_sharp_region()
    surface.set_page_links([link])
    surface.link_activated.connect(activated_links.append)
    surface.show()
    app.processEvents()
    position = QPoint(_INSET + 80, _INSET + 42)

    before_hover = _render_surface(surface)
    _send_mouse_event(surface, QEvent.Type.MouseMove, position)
    app.processEvents()
    after_hover = _render_surface(surface)
    _send_mouse_event(
        surface,
        QEvent.Type.MouseButtonPress,
        position,
        Qt.MouseButton.LeftButton,
    )
    app.processEvents()

    assert surface.cursor().shape() == Qt.CursorShape.PointingHandCursor
    assert before_hover != after_hover
    assert _content_color(after_hover, 30, 40) == _RED
    assert activated_links == [link]
    surface.close()


@pytest.mark.parametrize(
    "style",
    [_DEFAULT_PAGE_BORDER_STYLE, _CURRENT_PAGE_BORDER_STYLE],
)
def test_page_border_styles_leave_the_same_area_for_the_page(
    style: str,
) -> None:
    _app()
    surface = PageSurfaceLabel()
    surface.setStyleSheet(style)
    surface.setFixedSize(100 + PAGE_CONTENT_CHROME, 80 + PAGE_CONTENT_CHROME)
    surface.ensurePolished()

    # A smaller area would crop the page image at its edges.
    assert surface.contentsRect() == QRect(_INSET, _INSET, 100, 80)
