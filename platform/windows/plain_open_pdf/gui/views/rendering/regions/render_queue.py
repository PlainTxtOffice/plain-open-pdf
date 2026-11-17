"""Render queue for the visible part of large pages.

- A page too large to render whole gets two images: a sharp one for the
  part in view, painted over a blurry one of the whole page.
- The page label keeps the size of its placeholder throughout, so text
  selection, links and scroll positions do not move.
"""

from __future__ import annotations

import math
from typing import TYPE_CHECKING, Protocol

from PySide6.QtCore import QPointF, QRect, QRectF, QSize
from PySide6.QtGui import QPixmap

from plain_open_pdf.gui.views.rendering.page_metrics import PAGE_CONTENT_CHROME
from plain_open_pdf.gui.views.rendering.page_rendering import (
    PageRenderRequest,
    PageRenderTask,
    RenderedRegion,
)
from plain_open_pdf.gui.views.rendering.page_surface import PageSurfaceLabel
from plain_open_pdf.gui.views.rendering.regions.geometry import (
    RegionPlan,
    calc_base_pixels,
    calc_base_zoom,
    calc_region_plan,
    is_large_page,
)

if TYPE_CHECKING:
    from pathlib import Path
    from threading import Lock

    from PySide6.QtCore import QThreadPool
    from PySide6.QtGui import QImage
    from PySide6.QtWidgets import QLabel

    from plain_open_pdf.application.document.session import PdfDocument
    from plain_open_pdf.domains.document.models import Rectangle
    from plain_open_pdf.gui.models.window_state import RenderGenerations
    from plain_open_pdf.gui.views.rendering.zoom.preview import (
        ZoomPreviewGesture,
    )

#: An image already on a page label serves as the blurry whole page when
#: it has at least this share of the pixels a new one would have.
_BASE_KEEP_SHARE = 0.9


class _PageRegionQueueHost(Protocol):
    """Describe the main-window API required by PageRegionQueueMixin."""

    _document: PdfDocument
    _page_labels: list[QLabel]
    _desired_page_indexes: set[int]
    _pending_page_indexes: dict[int, int]
    _generations: RenderGenerations
    _render_request_counter: int
    _render_request_lock: Lock
    _render_scope_generation: int
    _render_scope_page_indexes: frozenset[int]
    _thread_pool: QThreadPool
    _zoom_preview_gesture: ZoomPreviewGesture | None
    _sharp_regions: dict[int, QRect]
    _pending_page_regions: dict[int, tuple[int, QRect, bool]]
    _pending_base_pages: dict[int, int]
    _region_page_indexes: set[int]
    _failed_page_indexes: set[int]

    def _label_index_for_page(
        self,
        page_index: int | None,
    ) -> int | None: ...
    def _viewport_in_page(self, page_index: int) -> QRect | None: ...
    def _render_device_pixel_ratio(self) -> float: ...
    def _update_current_page_highlight(self) -> None: ...
    def _highlight_search_matches(
        self,
        *,
        pixmap: QPixmap,
        rectangles: tuple[Rectangle, ...],
        origin: QPointF | None = None,
        zoom: float | None = None,
    ) -> QPixmap: ...
    def _search_rectangles_for_page(
        self,
        page_index: int,
    ) -> tuple[Rectangle, ...]: ...
    def _ensure_page_links_loaded(self, page_index: int) -> None: ...
    def _ensure_page_words_loaded(self, page_index: int) -> None: ...
    def _queue_page_render(self, page_index: int) -> None: ...
    def _is_page_render_pending(self, page_index: int) -> bool: ...
    def _register_pending_page_render(self, page_index: int) -> int: ...
    def _retire_pending_page_render(
        self,
        page_index: int,
        request_token: int | None,
    ) -> None: ...
    def _is_render_request_current(
        self,
        generation: int,
        page_index: int,
        request_token: int,
    ) -> bool: ...
    def _handle_page_render_cancelled(
        self,
        _generation: int,
        page_index: int,
        request_token: int,
    ) -> None: ...
    def _handle_page_render_failed(
        self,
        generation: int,
        page_index: int,
        request_token: int | None,
        error_message: str,
    ) -> None: ...

    def _page_surface(self, page_index: int) -> PageSurfaceLabel | None: ...
    def _look_region_to_render(
        self,
        page_index: int,
        plan: RegionPlan,
    ) -> QRect | None: ...
    def _look_uncovered_region(
        self,
        page_index: int,
        plan: RegionPlan,
    ) -> QRect: ...
    def _look_pending_region(self, page_index: int) -> QRect | None: ...
    def _start_region_render(
        self,
        page_index: int,
        region: QRect,
        source_path: Path,
        *,
        has_margins: bool,
    ) -> None: ...
    def _is_base_render_needed(self, page_index: int) -> bool: ...
    def _start_base_render(
        self,
        page_index: int,
        source_path: Path,
    ) -> None: ...
    def _is_base_request_current(
        self,
        generation: int,
        page_index: int,
        request_token: int,
    ) -> bool: ...
    def _retire_pending_base_render(
        self,
        _generation: int,
        page_index: int,
        request_token: int,
    ) -> None: ...
    def _apply_region_render(
        self,
        generation: int,
        page_index: int,
        request_token: int,
        rendered: RenderedRegion,
    ) -> None: ...
    def _apply_base_render(
        self,
        generation: int,
        page_index: int,
        request_token: int,
        image: QImage,
    ) -> None: ...


class PageRegionQueueMixin:
    """Queue and show renders of the visible part of large pages."""

    def _calc_page_plan(
        self: _PageRegionQueueHost,
        page_index: int,
    ) -> RegionPlan | None:
        """Return what to render of one page, or None to render it whole.

        - None also covers a page without a label and a view that is not
          laid out yet.
        """
        view = self._viewport_in_page(page_index)
        page_surface = self._page_surface(page_index)
        if view is None or page_surface is None:
            return None

        ratio = self._render_device_pixel_ratio()
        left = math.floor(view.x() * ratio)
        top = math.floor(view.y() * ratio)
        right = math.ceil((view.x() + view.width()) * ratio)
        bottom = math.ceil((view.y() + view.height()) * ratio)
        return calc_region_plan(
            page=_calc_page_pixel_size(page_surface, ratio),
            view=QRect(left, top, right - left, bottom - top),
        )

    def _is_large_page(self: _PageRegionQueueHost, page_index: int) -> bool:
        """Return whether a page is large enough to be rendered in part.

        - Depends on the page's size on screen only, so it has an answer
          even while the view's place on the page is unknown.
        """
        page_surface = self._page_surface(page_index)
        if page_surface is None:
            return False

        ratio = self._render_device_pixel_ratio()
        return is_large_page(_calc_page_pixel_size(page_surface, ratio))

    def _queue_region_render(
        self: _PageRegionQueueHost,
        page_index: int,
        plan: RegionPlan,
    ) -> None:
        """Start the renders a large page still needs for the current view.

        - Starts a region render unless the region on screen covers the
          view or a render of the page is on its way.
        - A render on its way is never replaced. It cannot be stopped
          once it draws, so its image is shown when it arrives and the
          next request is then made for the view at that moment. While
          the user keeps scrolling, every finished render reaches the
          screen instead of being thrown away.
        - Starts the blurry whole-page render when the label has no
          image good enough. It goes second so the sharp image of the
          view arrives first.
        - A page whose render failed is left alone: asking again would
          fail again and swap the message on the page for an image and
          back.
        """
        source_path = self._document.source_path
        if source_path is None or page_index in self._failed_page_indexes:
            return

        region = (
            None
            if self._is_page_render_pending(page_index)
            else self._look_region_to_render(page_index, plan)
        )
        if region is not None:
            self._start_region_render(
                page_index,
                region,
                source_path,
                has_margins=region.contains(plan.wide),
            )

        # A thread pool may run the region task, and everything that
        # follows from its result, before `start` returns. The label is
        # therefore checked only now, as is a failure of that task.
        if (
            page_index not in self._failed_page_indexes
            and self._is_base_render_needed(page_index)
        ):
            self._start_base_render(page_index, source_path)

    def _look_region_to_render(
        self: _PageRegionQueueHost,
        page_index: int,
        plan: RegionPlan,
    ) -> QRect | None:
        """Return the rectangle to render next, or None when none is needed.

        - A page seen for the first time gets the tight rectangle, which
          renders fastest; the wide one follows once that is on screen.
        - A page outside the view gets one wide render and is left alone
          until it scrolls into view.
        - A rectangle that may lie wholly past the real page edge is not
          asked for; see `_is_region_on_page`.
        """
        page_surface = self._page_surface(page_index)
        if page_surface is None:
            return None

        region = self._look_uncovered_region(page_index, plan)
        ratio = self._render_device_pixel_ratio()
        if _is_region_on_page(region, page_surface, ratio):
            return region
        return None

    def _look_uncovered_region(
        self: _PageRegionQueueHost,
        page_index: int,
        plan: RegionPlan,
    ) -> QRect:
        """Return the planned rectangle the page does not show yet.

        - The result is empty when the region on screen covers the view.
        - A tight region on screen is recorded as an empty rectangle and
          covers nothing, so the wide one is asked for after it.
        """
        shown = self._sharp_regions.get(page_index)
        if plan.needed.isEmpty():
            return plan.wide if shown is None else QRect()

        if shown is not None and shown.contains(plan.needed):
            return QRect()

        if shown is None and not plan.tight.isEmpty():
            return plan.tight
        return plan.wide

    def _look_pending_region(
        self: _PageRegionQueueHost,
        page_index: int,
    ) -> QRect | None:
        """Return the rectangle of the region render on its way, if any.

        - An entry counts only while its token is the page's pending one,
          so a cancelled, failed or replaced request leaves nothing behind.
        """
        entry = self._pending_page_regions.get(page_index)
        if entry is None:
            return None

        request_token, region, _has_margins = entry
        with self._render_request_lock:
            if self._pending_page_indexes.get(page_index) != request_token:
                return None
        return region

    def _start_region_render(
        self: _PageRegionQueueHost,
        page_index: int,
        region: QRect,
        source_path: Path,
        *,
        has_margins: bool,
    ) -> None:
        """Queue a render of one rectangle of a page.

        - `has_margins` says whether the rectangle includes the scroll
          margins. A tight rectangle without them is shown but does not
          count as covering the view, so the wide one is asked for next.
        """
        request_token = self._register_pending_page_render(page_index)
        self._pending_page_regions[page_index] = (
            request_token,
            QRect(region),
            has_margins,
        )
        task = PageRenderTask(
            request=PageRenderRequest(
                source_path=source_path,
                render_revision=self._document.render_revision,
                page_index=page_index,
                zoom=self._document.state.zoom,
                device_pixel_ratio=self._render_device_pixel_ratio(),
                generation=self._generations.render,
                request_token=request_token,
                is_still_current=self._is_render_request_current,
                region=(
                    region.x(),
                    region.y(),
                    region.width(),
                    region.height(),
                ),
            ),
        )
        task.signals.region_rendered.connect(self._apply_region_render)
        task.signals.failed.connect(self._handle_page_render_failed)
        task.signals.cancelled.connect(self._handle_page_render_cancelled)
        self._thread_pool.start(task)

    def _is_base_render_needed(
        self: _PageRegionQueueHost,
        page_index: int,
    ) -> bool:
        """Return whether the page lacks a whole-page image to show.

        - True when the label has no image, or one with clearly fewer
          pixels than the blurry whole page would have: a page zoomed in
          from a small size keeps its old image, which is far too coarse
          to show beside a sharp region.
        - False while such a render is on its way.
        """
        page_surface = self._page_surface(page_index)
        if page_surface is None:
            return False

        ratio = self._render_device_pixel_ratio()
        wanted_pixels = calc_base_pixels(
            _calc_page_pixel_size(page_surface, ratio),
        )
        shown_pixels = _count_pixels(page_surface.pixmap())
        if shown_pixels >= _BASE_KEEP_SHARE * wanted_pixels:
            return False

        with self._render_request_lock:
            return page_index not in self._pending_base_pages

    def _start_base_render(
        self: _PageRegionQueueHost,
        page_index: int,
        source_path: Path,
    ) -> None:
        """Queue a blurry render of the whole page to show under a region.

        - It has its own token and pending entry, so it and a region
          render of the same page never cancel each other.
        """
        page_surface = self._page_surface(page_index)
        if page_surface is None:
            return

        ratio = self._render_device_pixel_ratio()
        with self._render_request_lock:
            self._render_request_counter += 1
            request_token = self._render_request_counter
            self._pending_base_pages[page_index] = request_token

        task = PageRenderTask(
            request=PageRenderRequest(
                source_path=source_path,
                render_revision=self._document.render_revision,
                page_index=page_index,
                zoom=calc_base_zoom(
                    zoom=self._document.state.zoom,
                    device_pixel_ratio=ratio,
                    page=_calc_page_pixel_size(page_surface, ratio),
                ),
                device_pixel_ratio=1.0,
                generation=self._generations.render,
                request_token=request_token,
                is_still_current=self._is_base_request_current,
                is_base=True,
            ),
        )
        task.signals.base_rendered.connect(self._apply_base_render)
        task.signals.failed.connect(self._retire_pending_base_render)
        task.signals.cancelled.connect(self._retire_pending_base_render)
        self._thread_pool.start(task)

    def _is_base_request_current(
        self: _PageRegionQueueHost,
        generation: int,
        page_index: int,
        request_token: int,
    ) -> bool:
        """Return whether a blurry whole-page render is still wanted.

        - Called from render threads, so it reads under the request lock.
        """
        with self._render_request_lock:
            return (
                generation == self._render_scope_generation
                and page_index in self._render_scope_page_indexes
                and self._pending_base_pages.get(page_index) == request_token
            )

    def _retire_pending_base_render(
        self: _PageRegionQueueHost,
        _generation: int,
        page_index: int,
        request_token: int,
    ) -> None:
        """Forget a blurry whole-page render that ended.

        - A failed or cancelled render needs nothing else: the region
          render of the same page reports a page that cannot be drawn.
        """
        with self._render_request_lock:
            if self._pending_base_pages.get(page_index) == request_token:
                del self._pending_base_pages[page_index]

    def _apply_region_render(
        self: _PageRegionQueueHost,
        generation: int,
        page_index: int,
        request_token: int,
        rendered: RenderedRegion,
    ) -> None:
        """Show a rendered region on its page label.

        - A result whose request is no longer the page's pending one is
          dropped: the pages were rebuilt or re-rendered since.
        - The label keeps its size and the page does not count as fully
          rendered, so later scrolling can ask for another region.
        - Ends by asking for what the page still needs: the wide region
          after a tight one, or the whole page when the view grew.
        """
        entry = self._pending_page_regions.get(page_index)
        requested = (
            self._look_pending_region(page_index)
            if entry is not None and entry[0] == request_token
            else None
        )
        self._retire_pending_page_render(page_index, request_token)
        if (
            entry is None
            or requested is None
            or generation != self._generations.render
        ):
            return

        if page_index not in self._desired_page_indexes:
            return

        page_surface = self._page_surface(page_index)
        if page_surface is None:
            return

        # The image is placed and marked at the zoom it was rendered
        # for. During a zoom gesture that differs from the current zoom;
        # the label stretches the finished image to the page.
        image = rendered.image
        ratio = image.devicePixelRatio()
        origin = QPointF(rendered.left / ratio, rendered.top / ratio)
        region_pixmap = self._highlight_search_matches(
            pixmap=QPixmap.fromImage(image),
            rectangles=self._search_rectangles_for_page(page_index),
            origin=origin,
            zoom=rendered.zoom,
        )
        page_surface.set_sharp_region(
            region_pixmap,
            rect=QRectF(
                origin.x(),
                origin.y(),
                image.width() / ratio,
                image.height() / ratio,
            ),
            zoom=rendered.zoom,
        )
        if page_surface.pixmap().isNull():
            page_surface.setText("")

        # A region rendered for an earlier zoom is still painted,
        # stretched, but it does not count as covering the view. Neither
        # does a tight region, so that the wide one is asked for below.
        if rendered.zoom == self._document.state.zoom:
            has_margins = entry[2]
            self._sharp_regions[page_index] = (
                requested if has_margins else QRect()
            )
        self._region_page_indexes.add(page_index)
        self._ensure_page_links_loaded(page_index)
        self._ensure_page_words_loaded(page_index)
        self._update_current_page_highlight()
        if self._zoom_preview_gesture is None:
            self._queue_page_render(page_index)

    def _apply_base_render(
        self: _PageRegionQueueHost,
        generation: int,
        page_index: int,
        request_token: int,
        image: QImage,
    ) -> None:
        """Show a blurry whole-page render under the page's region.

        - The image is stretched to the label's page area by giving it a
          matching pixel ratio; the label keeps its size.
        - Dropped when the label already holds an image with at least
          as many pixels, and when the page's render failed meanwhile.
        - During a zoom gesture the image joins the ones the gesture
          resizes on every step.
        """
        self._retire_pending_base_render(generation, page_index, request_token)
        if generation != self._generations.render:
            return

        if page_index not in self._desired_page_indexes:
            return

        page_surface = self._page_surface(page_index)
        if page_surface is None or page_index in self._failed_page_indexes:
            return

        # During a zoom gesture the label may show a shrunken copy; the
        # image the gesture resizes from is the one to compare with.
        gesture = self._zoom_preview_gesture
        shown_pixmap = (
            gesture.page_pixmaps.get(page_index) if gesture else None
        ) or page_surface.pixmap()
        if _count_pixels(shown_pixmap) >= image.width() * image.height():
            return

        base_pixmap = QPixmap.fromImage(image)
        base_pixmap.setDevicePixelRatio(
            base_pixmap.width() / _content_length(page_surface.width()),
        )
        base_pixmap = self._highlight_search_matches(
            pixmap=base_pixmap,
            rectangles=self._search_rectangles_for_page(page_index),
        )
        page_surface.setPixmap(base_pixmap)
        self._region_page_indexes.add(page_index)
        # A zoom gesture resizes the images it noted when it began. One
        # that arrives during the gesture is added, or it would keep its
        # size while the page around it grows or shrinks.
        if gesture is not None:
            gesture.page_pixmaps[page_index] = base_pixmap

    def _page_surface(
        self: _PageRegionQueueHost,
        page_index: int,
    ) -> PageSurfaceLabel | None:
        """Return the label showing a page, or None when it has none."""
        label_index = self._label_index_for_page(page_index)
        if label_index is None or label_index >= len(self._page_labels):
            return None

        page_label = self._page_labels[label_index]
        return page_label if isinstance(page_label, PageSurfaceLabel) else None


def _content_length(label_length: int) -> int:
    """Return the page image's share of a label's width or height."""
    return max(label_length - PAGE_CONTENT_CHROME, 1)


def _count_pixels(pixmap: QPixmap) -> int:
    """Return how many pixels an image holds; none for an empty one."""
    return pixmap.width() * pixmap.height()


def _is_region_on_page(
    region: QRect,
    page_surface: PageSurfaceLabel,
    ratio: float,
) -> bool:
    """Return whether a planned rectangle is sure to touch the page image.

    - The page size behind a plan is an upper bound. A rectangle lying
      wholly in the few pixels past the real page edge would be refused
      by the engine, so it must not be asked for.
    """
    if region.isEmpty():
        return False

    sure_width = _content_length(page_surface.width()) - 1
    sure_height = _content_length(page_surface.height()) - 1
    return region.x() < math.floor(
        sure_width * ratio,
    ) and region.y() < math.floor(sure_height * ratio)


def _calc_page_pixel_size(
    page_surface: PageSurfaceLabel,
    ratio: float,
) -> QSize:
    """Return an upper bound on the size of the whole page image.

    - The label is sized from the page size rounded to whole logical
      pixels, so the exact image size is not known here. One extra logical
      pixel makes the result at least as large as the image; the engine
      cuts a region reaching past the real edge.
    """
    return QSize(
        math.ceil((_content_length(page_surface.width()) + 1) * ratio),
        math.ceil((_content_length(page_surface.height()) + 1) * ratio),
    )
