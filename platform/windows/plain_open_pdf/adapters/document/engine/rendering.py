"""Rendering pages with the PDF engine."""

from __future__ import annotations

from typing import TYPE_CHECKING

from plain_open_pdf.adapters.document.engine.document import RenderedPage

if TYPE_CHECKING:
    from plain_open_pdf.adapters.document.engine.document import EngineDocument

_RGB_CHANNELS = 3


def _render(
    document: EngineDocument,
    page_index: int,
    scale: float,
) -> RenderedPage:
    width, height, samples = document.native.render_page(
        page_index,
        scale,
        mode="RGB",
    )
    return RenderedPage(
        width=int(width),
        height=int(height),
        channels=_RGB_CHANNELS,
        samples=bytes(samples),
    )


def _calc_render_scale(zoom: float, device_pixel_ratio: float) -> float:
    """Return the engine scale for a zoom level on one screen.

    - Whole-page and region renders both use this, so a region lines up
      pixel for pixel with the whole page drawn at the same zoom.
    """
    return zoom * max(device_pixel_ratio, 1.0)


def render_page(
    document: EngineDocument,
    page_index: int,
    *,
    zoom: float,
    device_pixel_ratio: float = 1.0,
) -> RenderedPage:
    """Render a page at a specific zoom level."""
    scale = _calc_render_scale(zoom, device_pixel_ratio)
    return _render(document, page_index, scale)


def render_page_region(
    document: EngineDocument,
    page_index: int,
    *,
    zoom: float,
    device_pixel_ratio: float = 1.0,
    region: tuple[int, int, int, int],
) -> RenderedPage:
    """Render one rectangle of a page at a specific zoom level.

    - `region` is `(left, top, width, height)` in pixels of the whole page
      image that `render_page` gives for the same zoom and pixel ratio.
    - The result holds the same pixels as that rectangle of the whole
      image. A region reaching past the page is cut at the page edge, so
      the result can be smaller than requested.
    - Raises the engine's error when no pixel of the region is on the page.
    """
    left, top, region_width, region_height = region
    width, height, samples = document.native.render_page_region(
        page_index,
        _calc_render_scale(zoom, device_pixel_ratio),
        left,
        top,
        region_width,
        region_height,
        mode="RGB",
    )
    return RenderedPage(
        width=int(width),
        height=int(height),
        channels=_RGB_CHANNELS,
        samples=bytes(samples),
    )


def render_thumbnail(
    document: EngineDocument,
    page_index: int,
    *,
    max_dimension: int = 160,
) -> RenderedPage:
    """Render a page thumbnail constrained by the larger dimension."""
    width, height = page_size_points(document, page_index)
    scale = min(max_dimension / max(width, 1), max_dimension / max(height, 1))
    return _render(document, page_index, scale)


def page_size_points(
    document: EngineDocument,
    page_index: int,
) -> tuple[float, float]:
    """Return one page's unscaled displayed size in PDF points."""
    size = document.native.view_size(page_index)
    if size is None:
        msg = f"Page index {page_index} is out of range."
        raise ValueError(msg)
    return size


def display_size_for_page_size(
    page_size: tuple[float, float],
    *,
    zoom: float,
) -> tuple[int, int]:
    """Return the rendered pixel size for an unscaled page size."""
    page_width, page_height = page_size
    return (
        max(int(page_width * zoom), 1),
        max(int(page_height * zoom), 1),
    )


def page_display_size(
    document: EngineDocument,
    page_index: int,
    *,
    zoom: float,
) -> tuple[int, int]:
    """Return the rendered page size for a given zoom level."""
    return display_size_for_page_size(
        page_size_points(document, page_index),
        zoom=zoom,
    )
