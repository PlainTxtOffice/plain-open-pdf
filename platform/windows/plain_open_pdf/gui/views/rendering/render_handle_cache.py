"""Per-thread document handle cache for background renders."""

from __future__ import annotations

import threading
from contextlib import suppress
from dataclasses import dataclass
from typing import TYPE_CHECKING

from plain_open_pdf.adapters.document.engine.lifecycle import (
    close_document,
    open_document,
)

if TYPE_CHECKING:
    from pathlib import Path

    from plain_open_pdf.adapters.document.engine.document import EngineDocument


@dataclass(frozen=True)
class _CachedHandle:
    """One thread's open document and what it was opened for."""

    handle: EngineDocument
    source_path: Path
    render_revision: int


class _RenderHandleCache:
    """Reuse one open document per worker thread, keyed by source path.

    - Threads are told apart by their identity, not by `threading.local`:
      Qt's pool threads are not Python threads, so every task they run gets
      a new Python thread state, and thread-local values are gone by the
      next task. Keyed by identity, a pool thread keeps its handle.
    """

    def __init__(self) -> None:
        """Initialize the shared handle registry."""
        self._lock = threading.Lock()
        self._handles: dict[int, _CachedHandle] = {}

    def acquire(
        self,
        source_path: Path,
        render_revision: int,
    ) -> EngineDocument:
        """Return an open document for `source_path` on the current thread.

        - Reuses the thread's handle when its path and revision match.
        - Closes the thread's previous handle and opens a fresh one when the
          path or render revision differs, or after `close_all`.
        """
        thread = threading.get_ident()
        with self._lock:
            cached = self._handles.get(thread)
            if (
                cached is not None
                and cached.source_path == source_path
                and cached.render_revision == render_revision
            ):
                return cached.handle
            self._handles.pop(thread, None)

        if cached is not None:
            close_document(cached.handle)

        handle = open_document(source_path)
        with self._lock:
            # Handles other threads hold for older revisions of this file
            # are forgotten, not closed: a thread still rendering with one
            # keeps it until it finishes, and then it is freed.
            for other, held in list(self._handles.items()):
                if (
                    held.source_path == source_path
                    and held.render_revision < render_revision
                ):
                    del self._handles[other]
            self._handles[thread] = _CachedHandle(
                handle,
                source_path,
                render_revision,
            )
        return handle

    def invalidate(self) -> None:
        """Close and forget the current thread's cached handle."""
        with self._lock:
            cached = self._handles.pop(threading.get_ident(), None)
        if cached is not None:
            close_document(cached.handle)

    def close_all(self) -> None:
        """Close every cached handle across all threads.

        Callers must ensure no render task is mid-render, e.g. by waiting
        for the render thread pool to drain first.
        """
        with self._lock:
            handles = [cached.handle for cached in self._handles.values()]
            self._handles.clear()

        for handle in handles:
            with suppress(ValueError, RuntimeError):
                close_document(handle)


RENDER_HANDLE_CACHE = _RenderHandleCache()
