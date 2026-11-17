import threading
import time

from pathlib import Path

from PySide6.QtCore import QCoreApplication, QRunnable, QThreadPool

from engine_pdfs import create_pdf
from plain_open_pdf.application.document.session import PdfDocument
from plain_open_pdf.adapters.document.engine.document import EngineDocument
from plain_open_pdf.gui.views.rendering.render_handle_cache import _RenderHandleCache


def _create_sample_pdf(path: Path) -> None:
    create_pdf(path)


def test_acquire_reopens_same_path_when_render_revision_changes(
    tmp_path: Path,
) -> None:
    pdf_path = tmp_path / "cache.pdf"
    _create_sample_pdf(pdf_path)
    cache = _RenderHandleCache()

    first_handle = cache.acquire(pdf_path, 1)
    second_handle = cache.acquire(pdf_path, 1)
    third_handle = cache.acquire(pdf_path, 2)

    assert second_handle is first_handle
    assert third_handle is not first_handle

    cache.invalidate()


def test_close_all_closes_handle_and_forces_reopen_on_same_thread(
    tmp_path: Path,
) -> None:
    pdf_path = tmp_path / "close-all.pdf"
    _create_sample_pdf(pdf_path)
    cache = _RenderHandleCache()

    first_handle = cache.acquire(pdf_path, 1)

    cache.close_all()

    assert first_handle.is_closed

    second_handle = cache.acquire(pdf_path, 1)

    assert second_handle is not first_handle
    assert second_handle.is_closed is False
    assert second_handle.page_count == 1

    cache.invalidate()


def test_close_all_closes_handles_held_by_other_threads(
    tmp_path: Path,
) -> None:
    pdf_path = tmp_path / "close-all-threads.pdf"
    _create_sample_pdf(pdf_path)
    cache = _RenderHandleCache()
    worker_handles: dict[str, EngineDocument] = {}

    def _acquire_in_worker() -> None:
        worker_handles["handle"] = cache.acquire(pdf_path, 1)

    worker = threading.Thread(target=_acquire_in_worker)
    worker.start()
    worker.join()

    cache.close_all()

    assert worker_handles["handle"].is_closed


def test_pool_threads_keep_their_handle_between_tasks(tmp_path: Path) -> None:
    """A Qt pool thread reuses its handle; `threading.local` would not."""
    pdf_path = tmp_path / "pool.pdf"
    _create_sample_pdf(pdf_path)
    cache = _RenderHandleCache()
    _app = QCoreApplication.instance() or QCoreApplication([])
    handles: list[tuple[int, EngineDocument]] = []

    class _Acquire(QRunnable):
        def run(self) -> None:
            handles.append((threading.get_ident(), cache.acquire(pdf_path, 1)))

    pool = QThreadPool()
    pool.setMaxThreadCount(1)
    for _ in range(3):
        pool.start(_Acquire())
        # Wait for the task without draining the pool, which would end
        # its thread.
        deadline = time.monotonic() + 5
        while pool.activeThreadCount() and time.monotonic() < deadline:
            time.sleep(0.01)
    pool.waitForDone()

    assert len({thread for thread, _ in handles}) == 1
    assert len({id(handle) for _, handle in handles}) == 1

    cache.close_all()


def test_reopened_file_gets_a_fresh_handle_after_it_changes(
    tmp_path: Path,
) -> None:
    """A tab reopening a changed file never gets the old file's handle."""
    pdf_path = tmp_path / "changing.pdf"
    create_pdf(pdf_path, ["one", "two"])
    cache = _RenderHandleCache()
    _app = QCoreApplication.instance() or QCoreApplication([])
    pool = QThreadPool()
    pool.setMaxThreadCount(1)

    def acquire_on_pool(revision: int) -> EngineDocument:
        handles: list[EngineDocument] = []

        class _Acquire(QRunnable):
            def run(self) -> None:
                handles.append(cache.acquire(pdf_path, revision))

        pool.start(_Acquire())
        deadline = time.monotonic() + 5
        while not handles and time.monotonic() < deadline:
            time.sleep(0.01)
        return handles[0]

    first = PdfDocument()
    first.open_document(pdf_path)
    assert acquire_on_pool(first.render_revision).page_count == 2
    first.close_document()

    create_pdf(pdf_path, ["one", "two", "three"])
    second = PdfDocument()
    second.open_document(pdf_path)

    assert acquire_on_pool(second.render_revision).page_count == 3
    pool.waitForDone()
    second.close_document()
    cache.close_all()
