"""Background page rendering tasks for the PySide6 PDF view."""

from __future__ import annotations

from dataclasses import dataclass
from typing import TYPE_CHECKING

from PySide6.QtCore import QObject, QRunnable, Signal

from plain_open_pdf.adapters.document.engine.qt import qimage_from_render
from plain_open_pdf.adapters.document.engine.rendering import (
    render_page,
    render_page_region,
)
from plain_open_pdf.adapters.document.errors import PDF_IO_ERRORS
from plain_open_pdf.gui.views.rendering.render_handle_cache import (
    RENDER_HANDLE_CACHE,
)

if TYPE_CHECKING:
    from collections.abc import Callable
    from pathlib import Path

    from PySide6.QtGui import QImage


class _CancelledPageRenderError(RuntimeError):
    """Raised when a queued page render is no longer current."""


class PageRenderSignals(QObject):
    """Signals emitted by one background page render task.

    - A task emits exactly one of them, each with the generation, page
      index and request token of its request.
    - `rendered` carries a whole page as a `QImage`, `region_rendered` a
      `RenderedRegion`, and `base_rendered` the blurry whole page shown
      under a region, as a `QImage`.
    """

    rendered = Signal(int, int, int, object)
    region_rendered = Signal(int, int, int, object)
    base_rendered = Signal(int, int, int, object)
    failed = Signal(int, int, int, str)
    cancelled = Signal(int, int, int)


@dataclass(frozen=True, slots=True)
class RenderedRegion:
    """Hold one rendered rectangle of a page.

    - `left` and `top` place the image inside the whole page image, in
      device pixels at `zoom`.
    - The image can be smaller than the rectangle asked for: a region
      reaching past the page is cut at the page edge.
    """

    image: QImage
    left: int
    top: int
    zoom: float


@dataclass(slots=True)
class PageRenderRequest:
    """Store the inputs required for one background page render.

    - `region` asks for one rectangle `(left, top, width, height)` of the
      page image instead of the whole page.
    - `is_base` marks a whole-page render at reduced sharpness that is
      shown under a region.
    """

    source_path: Path
    render_revision: int
    page_index: int
    zoom: float
    device_pixel_ratio: float
    generation: int
    request_token: int
    is_still_current: Callable[[int, int, int], bool]
    region: tuple[int, int, int, int] | None = None
    is_base: bool = False


class PageRenderTask(QRunnable):
    """Render one PDF page into a detached Qt image."""

    def __init__(
        self,
        *,
        request: PageRenderRequest,
    ) -> None:
        """Store the inputs for one background render job."""
        super().__init__()
        self._request = request
        self.signals = PageRenderSignals()

    def run(self) -> None:
        """Render the configured page and emit the result."""
        try:
            image = self._render_image()
        except _CancelledPageRenderError:
            self.signals.cancelled.emit(
                self._request.generation,
                self._request.page_index,
                self._request.request_token,
            )
            return
        except PDF_IO_ERRORS as error:
            RENDER_HANDLE_CACHE.invalidate()
            self.signals.failed.emit(
                self._request.generation,
                self._request.page_index,
                self._request.request_token,
                str(error),
            )
            return

        if self._is_stale():
            self.signals.cancelled.emit(
                self._request.generation,
                self._request.page_index,
                self._request.request_token,
            )
            return

        region = self._request.region
        if region is not None:
            signal = self.signals.region_rendered
            result: object = RenderedRegion(
                image=image,
                left=region[0],
                top=region[1],
                zoom=self._request.zoom,
            )
        elif self._request.is_base:
            signal = self.signals.base_rendered
            result = image
        else:
            signal = self.signals.rendered
            result = image

        signal.emit(
            self._request.generation,
            self._request.page_index,
            self._request.request_token,
            result,
        )

    def _is_stale(self) -> bool:
        return not self._request.is_still_current(
            self._request.generation,
            self._request.page_index,
            self._request.request_token,
        )

    def _render_image(self) -> QImage:
        """Return the configured page, or its region, as a copied Qt image."""
        if self._is_stale():
            raise _CancelledPageRenderError

        document = RENDER_HANDLE_CACHE.acquire(
            self._request.source_path,
            self._request.render_revision,
        )
        if self._is_stale():
            raise _CancelledPageRenderError

        if self._request.region is None:
            pixmap = render_page(
                document,
                self._request.page_index,
                zoom=self._request.zoom,
                device_pixel_ratio=self._request.device_pixel_ratio,
            )
        else:
            pixmap = render_page_region(
                document,
                self._request.page_index,
                zoom=self._request.zoom,
                device_pixel_ratio=self._request.device_pixel_ratio,
                region=self._request.region,
            )
        if self._is_stale():
            raise _CancelledPageRenderError

        image = qimage_from_render(pixmap)
        image.setDevicePixelRatio(self._request.device_pixel_ratio)
        return image
