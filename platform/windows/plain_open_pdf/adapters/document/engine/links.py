"""Page links and where they lead, with the PDF engine."""

from __future__ import annotations

from typing import TYPE_CHECKING

from plain_open_pdf.domains.document.models import PdfPageLink

if TYPE_CHECKING:
    from plain_open_pdf.adapters.document.engine.document import EngineDocument


#: Named viewer actions that lead to a page, and how to find it from the
#: page the link is on and the page count.
_NAMED_PAGE_ACTIONS = {
    "NextPage": lambda page, _count: page + 1,
    "PrevPage": lambda page, _count: page - 1,
    "FirstPage": lambda _page, _count: 0,
    "LastPage": lambda _page, count: count - 1,
}

#: Views that place a point of the page at the window's top left corner.
_POSITIONED_FITS = frozenset({"XYZ", "FitH", "FitV", "FitBH", "FitBV", "FitR"})


def _target_point(
    document: EngineDocument,
    page_index: int,
    view: object,
) -> tuple[tuple[float, float] | None, float | None]:
    """Return a view's point in PDF space and its zoom.

    - A coordinate the view leaves to the viewer is taken from the page's
      left or top edge, so that the page opens at that side.
    """
    if view is None:
        return None, None
    fit = str(getattr(view, "fit", ""))
    zoom = getattr(view, "zoom", None)
    zoom = float(zoom) if zoom is not None and zoom > 0 else None
    left = getattr(view, "left", None)
    top = getattr(view, "top", None)
    if fit not in _POSITIONED_FITS or (left is None and top is None):
        return None, zoom
    crop_left, _bottom, _right, crop_top = document.crop_box(page_index)
    return (
        float(left) if left is not None else crop_left,
        float(top) if top is not None else crop_top,
    ), zoom


def _display_bounds(
    document: EngineDocument,
    page_index: int,
    rectangle: tuple[float, float, float, float],
    zoom: float,
) -> tuple[float, float, float, float]:
    """Return a PDF rectangle where it is displayed, scaled by `zoom`."""
    left, bottom, right, top = rectangle
    corners = [
        corner
        for x, y in ((left, bottom), (right, bottom), (left, top), (right, top))
        if (corner := document.native.to_view(page_index, x, y)) is not None
    ]
    if not corners:
        return (0.0, 0.0, 0.0, 0.0)
    xs = [corner[0] for corner in corners]
    ys = [corner[1] for corner in corners]
    return (min(xs) * zoom, min(ys) * zoom, max(xs) * zoom, max(ys) * zoom)


def page_links(
    document: EngineDocument,
    page_index: int,
    *,
    zoom: float,
) -> list[PdfPageLink]:
    """Return clickable link hotspots scaled into the current render space."""
    links: list[PdfPageLink] = []
    page_count = document.page_count
    for link in document.native.links(page_index):
        left, bottom, right, top = link.rectangle
        if right <= left or top <= bottom:
            continue

        target_page_index: int | None = None
        target_point: tuple[float, float] | None = None
        target_zoom: float | None = None
        uri: str | None = None
        file_path: str | None = None
        if link.kind == "page" and link.page is not None:
            target_page_index = int(link.page)
            target_point, target_zoom = _target_point(
                document,
                target_page_index,
                link.view,
            )
        elif link.kind == "uri":
            uri = (link.uri or "").strip() or None
        elif link.kind in {"remote", "launch"}:
            file_path = (link.file or "").strip() or None
        elif link.kind == "named" and link.name in _NAMED_PAGE_ACTIONS:
            candidate = _NAMED_PAGE_ACTIONS[link.name](page_index, page_count)
            if 0 <= candidate < page_count:
                target_page_index = candidate

        if target_page_index is None and uri is None and file_path is None:
            continue

        target_fit = None
        if target_page_index is not None:
            target_fit = (
                "XYZ"
                if target_point is not None or target_zoom is not None
                else "Fit"
            )
        links.append(
            PdfPageLink(
                bounds=_display_bounds(
                    document,
                    page_index,
                    (left, bottom, right, top),
                    zoom,
                ),
                target_page_index=target_page_index,
                target_point=target_point,
                target_zoom=target_zoom,
                target_fit=target_fit,
                named_destination=None,
                uri=uri,
                file_path=file_path,
            ),
        )
    return links


def page_link_target_position(
    document: EngineDocument,
    *,
    target_page_index: int | None,
    target_point: tuple[float, float] | None,
    zoom: float,
) -> tuple[float, float] | None:
    """Resolve a PDF-space destination point into display coordinates."""
    if target_page_index is None or target_point is None:
        return None

    position = document.native.to_view(target_page_index, *target_point)
    if position is None:
        return None
    return (position[0] * zoom, position[1] * zoom)


def pdf_point_for_display_position(
    document: EngineDocument,
    *,
    page_index: int,
    position: tuple[float, float],
    zoom: float,
) -> tuple[float, float] | None:
    """Resolve a display-space position back into its PDF-space point.

    - Exact inverse of `page_link_target_position`, so a round trip at any
      zoom or rotation returns the original point.
    """
    if zoom <= 0:
        return None

    point = document.native.from_view(
        page_index,
        position[0] / zoom,
        position[1] / zoom,
    )
    if point is None:
        return None
    return (float(point[0]), float(point[1]))


def resolve_page_link_target(
    document: EngineDocument,
    *,
    target_page_index: int | None,
    target_point: tuple[float, float] | None,
    named_destination: str | None,
    zoom: float,
) -> tuple[int, tuple[float, float] | None] | None:
    """Resolve direct or named PDF link targets into page and position."""
    if target_page_index is None:
        if named_destination is None:
            return None
        resolved = document.native.named_destination(named_destination)
        if resolved is None:
            return None
        target_page_index, view = resolved
        target_point, _zoom = _target_point(document, target_page_index, view)

    return (
        target_page_index,
        page_link_target_position(
            document,
            target_page_index=target_page_index,
            target_point=target_point,
            zoom=zoom,
        ),
    )
