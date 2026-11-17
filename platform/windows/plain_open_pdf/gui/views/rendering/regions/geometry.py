"""Choosing which rectangle of a large page to render.

- At high zoom a whole page image has tens of millions of pixels, most of
  them outside the window. The functions here pick the part worth drawing.
- All rectangles are in device pixels of the whole page image, from its
  top left corner.
"""

from __future__ import annotations

import math
from dataclasses import dataclass

from PySide6.QtCore import QRect, QSize

#: Pages with fewer pixels than this are rendered whole.
REGION_MIN_PAGE_PIXELS = 4_000_000
#: A region covering at least this share of the page is not worth a
#: separate code path; the page is rendered whole.
REGION_MAX_SHARE = 0.7
#: Extra height rendered above and below the view, as a share of the view
#: height, so short scrolls stay inside the region.
REGION_MARGIN_SHARE = 0.25
#: Smallest extra height rendered above and below the view, in pixels.
REGION_MIN_MARGIN = 256
#: Extra width rendered left and right of the view, in pixels.
REGION_SIDE_MARGIN = 128
#: How far past the view a region must reach before a new one is needed,
#: as a share of the view's width and height.
REGION_TRIGGER_SHARE = 0.08
#: Region edges are moved outward to a multiple of this many pixels, so
#: small scroll steps ask for the same rectangle.
REGION_GRID = 64
#: Pixel count of the blurry whole-page image shown under a region.
BASE_PIXELS = 1_500_000


@dataclass(frozen=True, slots=True)
class RegionPlan:
    """Rectangles of one page that the view asks for.

    - `needed` must be inside the region on screen; when it is not, a new
      region has to be rendered. It is the view plus a small margin.
    - `tight` is the visible part of the page alone: the smallest render
      that makes the view sharp.
    - `wide` is the view plus scroll margins: the region to keep on screen.
    - Any of them is empty when the view does not reach the page.
    """

    needed: QRect
    tight: QRect
    wide: QRect


def calc_region_plan(*, page: QSize, view: QRect) -> RegionPlan | None:
    """Return the rectangles to render for a view, or None for a whole page.

    - `page` is the size of the whole page image and `view` is the window's
      visible area in the same pixels; `view` may reach outside the page.
    - None means the page should be rendered whole: it is small, or a
      view of this size with its margins covers most of it.
    - The choice depends on the sizes of the page and the view only, not
      on where the view is. A page therefore keeps one answer while it
      scrolls past, and a page still outside the view gets the same
      answer it will have once it is in view.
    """
    if not is_large_page(page):
        return None

    page_pixels = page.width() * page.height()
    page_rect = QRect(0, 0, page.width(), page.height())
    vertical_margin = max(
        math.ceil(REGION_MARGIN_SHARE * view.height()),
        REGION_MIN_MARGIN,
    )
    reach = _grow(view, REGION_SIDE_MARGIN, vertical_margin)
    largest_region_pixels = min(reach.width(), page.width()) * min(
        reach.height(),
        page.height(),
    )
    if largest_region_pixels >= REGION_MAX_SHARE * page_pixels:
        return None

    wide = _snap_outward(reach, page_rect)

    # The trigger margins stay well inside the wide margins, so a wide
    # region always covers what is needed for the view it was made for.
    needed = _grow(
        view,
        min(
            math.ceil(REGION_TRIGGER_SHARE * view.width()),
            REGION_SIDE_MARGIN // 2,
        ),
        min(
            math.ceil(REGION_TRIGGER_SHARE * view.height()),
            vertical_margin // 2,
        ),
    )
    return RegionPlan(
        needed=_intersect(needed, wide),
        tight=_snap_outward(_intersect(view, page_rect), page_rect),
        wide=wide,
    )


def is_large_page(page: QSize) -> bool:
    """Return whether a page image is large enough to render in part.

    - `page` is the size of the whole page image in device pixels.
    """
    return page.width() * page.height() >= REGION_MIN_PAGE_PIXELS


def calc_base_pixels(page: QSize) -> int:
    """Return the pixel count the blurry whole-page image aims for.

    - `page` is the size of the whole page image in device pixels.
    """
    return min(page.width() * page.height(), BASE_PIXELS)


def calc_base_zoom(
    *,
    zoom: float,
    device_pixel_ratio: float,
    page: QSize,
) -> float:
    """Return the render scale for the blurry whole-page image.

    - The scale gives an image of about `BASE_PIXELS` pixels, and never
      more than the page at full sharpness.
    - `page` is the size of the whole page image at `zoom` on a screen
      with `device_pixel_ratio`.
    """
    scale = zoom * device_pixel_ratio
    page_pixels = max(page.width() * page.height(), 1)
    return min(scale, scale * math.sqrt(BASE_PIXELS / page_pixels))


def _grow(rect: QRect, horizontal: int, vertical: int) -> QRect:
    """Return `rect` with margins added on all four sides."""
    if rect.isEmpty():
        return QRect()

    return rect.adjusted(-horizontal, -vertical, horizontal, vertical)


def _intersect(first: QRect, second: QRect) -> QRect:
    """Return the shared part of two rectangles, or an empty rectangle."""
    if first.isEmpty() or second.isEmpty():
        return QRect()

    shared = first.intersected(second)
    return QRect() if shared.isEmpty() else shared


def _snap_outward(rect: QRect, page_rect: QRect) -> QRect:
    """Move the edges of `rect` out to the grid, then cut it to the page."""
    if rect.isEmpty():
        return QRect()

    grid = max(REGION_GRID, 1)
    left = (rect.x() // grid) * grid
    top = (rect.y() // grid) * grid
    right = -((-(rect.x() + rect.width())) // grid) * grid
    bottom = -((-(rect.y() + rect.height())) // grid) * grid
    return _intersect(
        QRect(left, top, right - left, bottom - top),
        page_rect,
    )
