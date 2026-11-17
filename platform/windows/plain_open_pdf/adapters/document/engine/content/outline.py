"""Document outline (bookmarks), with the PDF engine."""

from __future__ import annotations

from typing import TYPE_CHECKING

from plain_open_pdf.domains.document.models import PdfOutlineItem

if TYPE_CHECKING:
    from plain_open_pdf.adapters.document.engine.document import EngineDocument


def outline_items(document: EngineDocument) -> list[PdfOutlineItem]:
    """Return the outline entries that lead to a page of the document.

    `toc_index` counts every entry, including those that lead nowhere, so
    that it can address the entry for renaming and deleting.
    """
    items: list[PdfOutlineItem] = []
    for toc_index, item in enumerate(document.native.outline()):
        if item.page is None:
            continue
        items.append(
            PdfOutlineItem(
                toc_index=toc_index,
                level=int(item.level),
                title=str(item.title),
                page_index=int(item.page),
            ),
        )
    return items


def append_outline_item(
    document: EngineDocument,
    *,
    title: str,
    page_index: int,
    level: int = 1,
) -> None:
    """Append one outline entry to the document's outline."""
    items = list(document.native.outline())
    document.native.set_outline([*items, (level, title, page_index)])


def rename_outline_item(
    document: EngineDocument,
    *,
    toc_index: int,
    title: str,
) -> None:
    """Rename one outline entry, keeping where it leads."""
    items = document.native.outline()
    item = items[toc_index]
    document.native.set_outline(
        [
            *items[:toc_index],
            (item.level, title, item.page, item.view),
            *items[toc_index + 1 :],
        ],
    )


def delete_outline_item(document: EngineDocument, *, toc_index: int) -> None:
    """Delete one outline entry and its nested descendants."""
    items = list(document.native.outline())
    selected_level = items[toc_index].level
    delete_end = toc_index + 1
    while delete_end < len(items) and items[delete_end].level > selected_level:
        delete_end += 1

    del items[toc_index:delete_end]
    document.native.set_outline(items)
