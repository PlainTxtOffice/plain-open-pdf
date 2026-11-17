# ruff: noqa: E402

import json
import logging
import math
import os
import sys

from pathlib import Path

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")
_FONT_DIRECTORY = Path(__file__).resolve().parents[1] / "assets" / "fonts"
if _FONT_DIRECTORY.exists():
    os.environ.setdefault("QT_QPA_FONTDIR", str(_FONT_DIRECTORY))
import pytest

from engine_pdfs import (
    PdfBuilder,
    create_named_destination_pdf,
    create_pdf,
    edited_pdf,
    open_pdf,
)

from PySide6.QtGui import QAction
from PySide6.QtGui import QCloseEvent
from PySide6.QtGui import QKeyEvent
from PySide6.QtGui import QMouseEvent
from PySide6.QtGui import QWheelEvent
from PySide6.QtCore import QEvent
from PySide6.QtCore import QPoint
from PySide6.QtCore import QPointF
from PySide6.QtCore import QRect
from PySide6.QtCore import QRectF
from PySide6.QtCore import QSize
from PySide6.QtCore import Qt
from PySide6.QtPrintSupport import QAbstractPrintDialog
from PySide6.QtTest import QTest
from PySide6.QtWidgets import QMessageBox
from PySide6.QtWidgets import QFileDialog
from PySide6.QtWidgets import QApplication
from PySide6.QtWidgets import QDialog
from PySide6.QtWidgets import QListView
from PySide6.QtWidgets import QMenu
from PySide6.QtWidgets import QSplitter
from PySide6.QtWidgets import QStyle
from PySide6.QtWidgets import QToolBar
from PySide6.QtWidgets import QToolButton

import plain_open_pdf.gui.views.root_window.window as app_module
import plain_open_pdf.gui.views.root_window.navigation.bookmarks as bookmarks_module
import plain_open_pdf.gui.views.rendering.selection as selection_module
import plain_open_pdf.gui.views.root_window.navigation.page_links as page_links_module
import plain_open_pdf.gui.views.root_window.panes.thumbnails as thumbnails_module
import plain_open_pdf.gui.views.workflows.printing as printing_module
from plain_open_pdf.adapters.document.engine.qt import qimage_from_render
from plain_open_pdf.adapters.document.engine.rendering import (
    display_size_for_page_size,
)
from plain_open_pdf.adapters.ocr.cache import OcrCache
from plain_open_pdf.domains.document.models import PdfOcrPageResult, PdfPageLink
from plain_open_pdf.gui.app import MainWindow
from plain_open_pdf.gui.views.rendering.page_metrics import PAGE_CONTENT_CHROME
from plain_open_pdf.gui.views.rendering.page_surface import PageSurfaceLabel
from plain_open_pdf.gui.views.rendering.regions import (
    geometry as region_geometry,
)
from plain_open_pdf.gui.views.rendering.regions import (
    render_queue as region_queue,
)
from plain_open_pdf.gui.views.dialogs.properties_dialog import DocumentPropertiesDialog


class _NoOpThreadPool:
    def start(self, _task: object) -> None:
        return None


class _ImmediateThreadPool:
    def start(self, task: object) -> None:
        task.run()


class _FailingThreadPool:
    def start(self, _task: object) -> None:
        msg = "Dirty documents should not queue source-file render tasks."
        raise AssertionError(msg)


class _CollectingThreadPool:
    def __init__(self) -> None:
        self.tasks: list[object] = []

    def start(self, task: object) -> None:
        self.tasks.append(task)


class _FakeContextMenu:
    def __init__(self, *_args, **_kwargs) -> None:
        self._actions: list[QAction] = []

    def addAction(self, text: str) -> QAction:
        action = QAction(text)
        self._actions.append(action)
        return action

    def actions(self) -> list[QAction]:
        return self._actions

    def exec(self, *_args, **_kwargs) -> QAction | None:
        return self._actions[0] if self._actions else None

    def setPalette(self, _palette: object) -> None:
        return None

    def setStyleSheet(self, _style_sheet: str) -> None:
        return None


@pytest.fixture(autouse=True)
def _isolate_ui_settings(monkeypatch, tmp_path: Path) -> None:
    monkeypatch.setattr(
        app_module,
        "_PANE_SETTINGS_PATH",
        tmp_path / "ui-state.json",
    )
    monkeypatch.setattr(
        app_module,
        "_DOCUMENT_PASSWORDS_PATH",
        tmp_path / "document-passwords.json",
    )


def _app() -> QApplication:
    return QApplication.instance() or QApplication(sys.argv)


def _create_sample_pdf(path: Path) -> None:
    create_pdf(path, ["first page", "second page"])


def _create_three_page_pdf(path: Path) -> None:
    create_pdf(path, [f"page {page_number}" for page_number in range(1, 4)])


def _create_single_page_pdf(
    path: Path,
    *,
    width: float,
    height: float,
) -> None:
    create_pdf(
        path,
        ["single page"],
        width=width,
        height=height,
        point=(20.0, 40.0),
    )


def _create_password_protected_pdf(path: Path) -> None:
    builder = PdfBuilder()
    builder.new_page()
    builder.insert_text(0, (72, 72), "locked")
    builder.save(path, user_password="user-pass", owner_password="owner-pass")


def _create_image_only_pdf(path: Path, *, page_count: int = 1) -> None:
    create_pdf(path, [""] * page_count)


def _create_linked_pdf(path: Path) -> None:
    builder = PdfBuilder()
    for _ in range(4):
        builder.new_page()
    builder.insert_text(1, (72, 72), "target page")
    builder.insert_text(0, (72, 72), "external link")
    builder.insert_uri_link(0, (72, 72, 152, 96), "https://example.com/spec")
    builder.insert_text(0, (72, 140), "internal link")
    builder.insert_page_link(
        0,
        (72, 140, 152, 164),
        target_page=1,
        point=(20, 250),
    )
    builder.save(path)


def _build_ocr_page_result(page_index: int, text: str) -> PdfOcrPageResult:
    words = []
    cursor_x = 72.0
    for word_index, word in enumerate(text.split()):
        word_width = max(len(word) * 8.0, 12.0)
        words.append(
            (
                cursor_x,
                72.0,
                cursor_x + word_width,
                92.0,
                word,
                0,
                0,
                word_index,
            ),
        )
        cursor_x += word_width + 8.0

    return PdfOcrPageResult(
        page_index=page_index,
        text=text,
        words=tuple(words),
        engine_name="fake-ocr",
    )


class _FakeOcrBackend:
    def __init__(self, results_by_page: dict[int, PdfOcrPageResult]) -> None:
        self._results_by_page = results_by_page
        self.calls: list[tuple[int, tuple[str, ...]]] = []

    def recognize_page(
        self,
        document: object,
        page_index: int,
        *,
        language_codes: tuple[str, ...],
    ) -> PdfOcrPageResult:
        del document
        self.calls.append((page_index, language_codes))
        return self._results_by_page[page_index]


def _mouse_press_event(*, x: float, y: float) -> QMouseEvent:
    position = QPointF(x, y)
    return QMouseEvent(
        QEvent.Type.MouseButtonPress,
        position,
        position,
        position,
        Qt.MouseButton.LeftButton,
        Qt.MouseButton.LeftButton,
        Qt.KeyboardModifier.NoModifier,
    )


def _ctrl_wheel_event(
    *,
    x: float,
    y: float,
    angle_delta_y: int,
    pixel_delta_y: int = 0,
) -> QWheelEvent:
    position = QPointF(x, y)
    return QWheelEvent(
        position,
        position,
        QPoint(0, pixel_delta_y),
        QPoint(0, angle_delta_y),
        Qt.MouseButton.NoButton,
        Qt.KeyboardModifier.ControlModifier,
        Qt.ScrollPhase.ScrollUpdate,
        False,
    )


def test_thumbnail_items_do_not_show_page_captions() -> None:
    assert MainWindow._thumbnail_item_text(0) == ""


def test_open_document_logs_page_count(tmp_path: Path, caplog) -> None:
    _app()
    source_path = tmp_path / "logging.pdf"
    _create_sample_pdf(source_path)
    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()

    with caplog.at_level(logging.INFO, logger=app_module.__name__):
        assert window._open_document(source_path) is True

    assert "Opened PDF with 2 pages." in caplog.messages


def test_thumbnail_list_centers_icons_in_single_column() -> None:
    _app()
    window = MainWindow()
    thumbnail_list = window._thumbnail_list

    assert thumbnail_list.viewMode() == QListView.ViewMode.IconMode
    assert thumbnail_list.flow() == QListView.Flow.TopToBottom
    assert thumbnail_list.isWrapping() is False
    assert thumbnail_list.movement() == QListView.Movement.Static
    assert thumbnail_list.spacing() == 8
    assert (
        thumbnail_list.gridSize().width()
        >= window._thumbnail_item_size().width()
    )
    assert (
        thumbnail_list.gridSize().height()
        == window._thumbnail_item_size().height()
    )


def test_thumbnail_grid_uses_the_full_viewport_width() -> None:
    app = _app()
    window = MainWindow()
    window.resize(1000, 700)
    window.show()
    app.processEvents()

    window._update_thumbnail_grid_size()

    assert window._thumbnail_list.gridSize().width() == (
        window._thumbnail_list.viewport().width()
    )


def test_thumbnail_items_fill_the_viewport_width(tmp_path: Path) -> None:
    app = _app()
    source_path = tmp_path / "thumbnail-item-width.pdf"
    _create_sample_pdf(source_path)
    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    window.resize(1000, 700)
    window.show()
    window._open_document(source_path)
    app.processEvents()

    item = window._thumbnail_list.item(0)

    assert item is not None
    assert window._thumbnail_list.visualItemRect(item).width() == (
        window._thumbnail_list.viewport().width()
    )
    assert window._thumbnail_pane.width() == window._thumbnail_list.width()


def test_thumbnail_items_reserve_space_before_icons_render(
    tmp_path: Path,
) -> None:
    _app()
    source_path = tmp_path / "thumbnail-spacing.pdf"
    _create_three_page_pdf(source_path)
    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()

    window.open_document_in_tab(source_path)

    expected_height = window._thumbnail_item_size().height()
    assert expected_height > window._thumbnail_icon_size().height()
    assert all(
        window._thumbnail_list.item(row).sizeHint().height() == expected_height
        for row in range(window._thumbnail_list.count())
    )


def test_thumbnail_pane_width_is_resizable_via_the_splitter() -> None:
    app = _app()
    window = MainWindow()
    window.resize(1100, 700)
    window.show()
    app.processEvents()

    splitter = window._main_splitter
    sizes = splitter.sizes()
    target = sizes[2] + 80
    splitter.setSizes([sizes[0], sizes[1] - 80, target])
    app.processEvents()

    assert abs(window._thumbnail_pane.width() - target) <= 1


def _widen_thumbnail_pane(window: MainWindow, extra_width: int) -> None:
    splitter = window._main_splitter
    sizes = splitter.sizes()
    splitter.setSizes(
        [sizes[0], sizes[1] - extra_width, sizes[2] + extra_width],
    )


def test_thumbnail_icons_scale_to_fill_a_wider_pane() -> None:
    app = _app()
    window = MainWindow()
    window.resize(1100, 700)
    window.show()
    app.processEvents()

    _widen_thumbnail_pane(window, 100)
    app.processEvents()

    viewport_width = window._thumbnail_list.viewport().width()
    icon_size = window._thumbnail_list.iconSize()
    assert icon_size.width() >= viewport_width - 16
    assert window._thumbnail_list.gridSize().width() == viewport_width
    assert window._thumbnail_list.gridSize().height() == (
        icon_size.height() + window._thumbnail_cell_padding().height()
    )


def test_growing_thumbnail_icons_queues_a_sharper_rerender(
    tmp_path: Path,
) -> None:
    app = _app()
    source_path = tmp_path / "thumbnail-rerender.pdf"
    _create_sample_pdf(source_path)
    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    window.open_document_in_tab(source_path)
    window.resize(1100, 700)
    window.show()
    app.processEvents()
    window._thumbnail_rendered_pages.add(0)

    _widen_thumbnail_pane(window, 100)
    app.processEvents()

    assert window._thumbnail_rerender_timer.isActive()

    generation_before = window._generations.thumbnail
    window._rerender_thumbnails()

    assert window._generations.thumbnail == generation_before + 1
    assert window._thumbnail_rendered_pages == set()
    assert window._thumbnail_render_max_dimension() == 2 * max(
        window._thumbnail_list.iconSize().width(),
        window._thumbnail_list.iconSize().height(),
    )


def test_thumbnail_icon_box_matches_the_document_page_aspect(
    tmp_path: Path,
) -> None:
    app = _app()
    source_path = tmp_path / "tall-pages.pdf"
    create_pdf(source_path, ["", ""], width=400, height=800)

    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    window.open_document_in_tab(source_path)
    window.resize(1100, 700)
    window.show()
    app.processEvents()

    icon_size = window._thumbnail_list.iconSize()
    viewport_width = window._thumbnail_list.viewport().width()
    assert icon_size.width() >= viewport_width - 16
    assert abs(icon_size.height() - icon_size.width() * 2) <= 2


def test_thumbnail_cell_padding_derives_from_style_metrics() -> None:
    _app()
    window = MainWindow()
    thumbnail_list = window._thumbnail_list
    style = thumbnail_list.style()

    focus_margin = style.pixelMetric(
        QStyle.PixelMetric.PM_FocusFrameHMargin,
        None,
        thumbnail_list,
    )
    spacing = max(
        style.pixelMetric(
            QStyle.PixelMetric.PM_LayoutVerticalSpacing,
            None,
            thumbnail_list,
        ),
        0,
    )
    padding = window._thumbnail_cell_padding()

    assert padding.width() == 2 * (focus_margin + 1)
    assert padding.height() == 2 * (focus_margin + 1) + spacing


def test_thumbnail_list_edge_chrome_is_minimal() -> None:
    _app()
    window = MainWindow()

    assert window._thumbnail_list.frameWidth() == 0
    scrollbar = window._thumbnail_list.verticalScrollBar()
    assert scrollbar.sizeHint().width() <= 10


def test_thumbnail_cells_reserve_only_a_small_margin_around_icons() -> None:
    _app()
    window = MainWindow()

    icon_size = window._thumbnail_icon_size()
    item_size = window._thumbnail_item_size()

    assert item_size.width() - icon_size.width() <= 16
    assert item_size.height() - icon_size.height() <= 16


def test_thumbnails_render_above_display_resolution_for_hidpi(
    tmp_path: Path,
) -> None:
    _app()
    source_path = tmp_path / "thumbnail-resolution.pdf"
    _create_sample_pdf(source_path)
    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    window.open_document_in_tab(source_path)

    window._render_dirty_thumbnail(0)

    icon_sizes = window._thumbnail_list.item(0).icon().availableSizes()
    assert icon_sizes
    rendered = icon_sizes[0]
    icon_size = window._thumbnail_icon_size()
    assert max(rendered.width(), rendered.height()) >= 2 * max(
        icon_size.width(),
        icon_size.height(),
    )


def test_flat_bookmarks_do_not_reserve_an_expander_gutter(
    tmp_path: Path,
) -> None:
    _app()
    source_path = tmp_path / "flat-bookmarks.pdf"
    _create_sample_pdf(source_path)

    with edited_pdf(source_path) as document:
        document.set_outline([(1, "Cover", 0), (1, "Copyright", 1)])

    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    window._open_document(source_path)

    assert window._bookmark_tree.rootIsDecorated() is False


def test_nested_bookmarks_retain_an_expander_gutter(tmp_path: Path) -> None:
    _app()
    source_path = tmp_path / "nested-bookmarks.pdf"
    _create_sample_pdf(source_path)

    with edited_pdf(source_path) as document:
        document.set_outline([(1, "Chapter", 0), (2, "Topic", 1)])

    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    window._open_document(source_path)

    assert window._bookmark_tree.rootIsDecorated() is True
    assert window._bookmark_tree.topLevelItem(0).childCount() == 1


def test_page_text_overlay_uses_word_boxes_for_selection() -> None:
    _app()
    page_label = PageSurfaceLabel()
    page_label.setFixedSize(210, 110)
    page_label.set_page_text_words(
        [
            (10.0, 10.0, 30.0, 20.0, "Hello", 0, 0, 0),
            (35.0, 10.0, 55.0, 20.0, "world", 0, 0, 1),
        ],
        zoom=2.0,
    )
    page_label.set_text_selection_enabled(enabled=True)

    page_label.text_layer().select_word_range(0, 1)
    menu = page_label.text_layer().createStandardContextMenu()
    action_texts = [action.text() for action in menu.actions()]

    assert page_label.selected_text() == "Hello world"
    assert page_label.text_layer().contextMenuPolicy() == (
        Qt.ContextMenuPolicy.CustomContextMenu
    )
    assert action_texts == ["Copy", "Select All"]
    assert menu.actions()[0].isEnabled() is True


def test_window_has_file_and_view_menus() -> None:
    _app()
    window = MainWindow()

    menu_titles = [action.text() for action in window.menuBar().actions()]

    assert menu_titles == ["File", "Edit", "View", "Help"]


def test_main_window_places_bookmarks_left_and_thumbnails_right() -> None:
    _app()
    window = MainWindow()

    main_splitter = window.centralWidget()

    assert isinstance(main_splitter, QSplitter)
    assert main_splitter.count() == 3
    assert main_splitter.widget(0) is window._bookmark_tree
    assert main_splitter.widget(2) is window._thumbnail_pane


def test_view_menu_can_swap_side_panes() -> None:
    _app()
    window = MainWindow()

    main_splitter = window.centralWidget()

    assert isinstance(main_splitter, QSplitter)

    window._thumbnails_left_side_action.trigger()

    assert main_splitter.widget(0) is window._thumbnail_pane
    assert main_splitter.widget(2) is window._bookmark_tree
    assert window._thumbnails_on_left is True
    assert window._thumbnails_left_side_action.isChecked() is True
    assert window._bookmarks_left_side_action.isChecked() is False

    window._bookmarks_left_side_action.trigger()

    assert main_splitter.widget(0) is window._bookmark_tree
    assert main_splitter.widget(2) is window._thumbnail_pane
    assert window._thumbnails_on_left is False


def test_file_menu_has_exit_and_help_menu_has_about(monkeypatch) -> None:
    _app()
    window = MainWindow()

    about_calls: list[tuple[str, str]] = []

    monkeypatch.setattr(
        QMessageBox,
        "about",
        staticmethod(
            lambda _parent, title, text: about_calls.append((title, text)),
        ),
    )

    assert window._properties_action.text() == "Properties"
    assert window._rename_file_action.text() == "Rename File"
    assert window._run_ocr_page_action.text() == "Run OCR on Current Page"
    assert window._run_ocr_document_action.text() == "Run OCR on Document"
    assert window._print_action.text() == "Print"
    assert window._edit_pdf_action.text() == "Edit PDF"
    assert window._remove_selected_text_action.text() == "Remove Content"
    assert window._close_tab_action.text() == "Close Tab"
    assert window._close_tab_action.shortcut().toString() == "Ctrl+W"
    assert window._exit_action.text() == "Exit"

    menus = {
        menu.title(): menu for menu in window.menuBar().findChildren(QMenu)
    }
    file_menu = menus["File"]
    edit_menu = menus["Edit"]

    assert file_menu is not None
    assert edit_menu is not None
    assert [
        action.text()
        for action in file_menu.actions()
        if not action.isSeparator()
    ] == [
        "Open",
        "Save",
        "Save As",
        "Rename File",
        "Print",
        "Properties",
        "Close Tab",
        "Exit",
    ]
    assert [
        action.text()
        for action in edit_menu.actions()
        if not action.isSeparator()
    ] == [
        "Run OCR on Current Page",
        "Run OCR on Document",
        "Insert Blank Page Before",
        "Insert Blank Page After",
        "Remove Content",
    ]


def test_toolbar_shows_edit_pdf_text_button(monkeypatch) -> None:
    _app()
    about_calls: list[tuple[str, str]] = []
    monkeypatch.setattr(
        MainWindow,
        "_show_about_dialog",
        lambda _window: about_calls.append(
            (
                "About Plain Open PDF",
                "Plain Open PDF\n\nA lightweight desktop PDF viewer and editor.",
            )
        ),
    )
    window = MainWindow()

    edit_pdf_button = window.findChild(QToolButton, "edit-pdf-button")

    assert edit_pdf_button is not None
    assert edit_pdf_button.defaultAction() is window._edit_pdf_action
    assert edit_pdf_button.toolButtonStyle() == (
        Qt.ToolButtonStyle.ToolButtonTextOnly
    )
    assert edit_pdf_button.text() == "Edit PDF"
    assert window._about_action.text() == "About Plain Open PDF"

    window._about_action.trigger()

    assert about_calls == [
        (
            "About Plain Open PDF",
            "Plain Open PDF\n\nA lightweight desktop PDF viewer and editor.",
        ),
    ]


def test_file_menu_includes_print_action_and_triggers_handler(
    monkeypatch,
    tmp_path: Path,
) -> None:
    _app()
    source_path = tmp_path / "print.pdf"
    _create_sample_pdf(source_path)
    print_calls: list[bool] = []

    monkeypatch.setattr(
        MainWindow,
        "_print_document",
        lambda self: print_calls.append(self._document.has_document()),
    )

    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()

    assert window._print_action.text() == "Print"
    assert window._print_action.isEnabled() is False

    window._open_document(source_path)

    assert window._print_action.isEnabled() is True

    window._print_action.trigger()

    assert print_calls == [True]


def test_ocr_page_action_updates_extracted_text_and_enables_actions(
    tmp_path: Path,
) -> None:
    _app()
    source_path = tmp_path / "ocr-page.pdf"
    _create_image_only_pdf(source_path)
    backend = _FakeOcrBackend({0: _build_ocr_page_result(0, "scanned alpha")})
    cache = OcrCache(cache_dir=tmp_path / "ocr-cache")

    window = MainWindow(ocr_backend=backend, ocr_cache=cache)
    window._thread_pool = _ImmediateThreadPool()

    assert window._run_ocr_page_action.isEnabled() is False
    assert window._run_ocr_document_action.isEnabled() is False

    window._open_document(source_path)

    assert window._run_ocr_page_action.isEnabled() is True
    assert window._run_ocr_document_action.isEnabled() is True

    window._run_ocr_page_action.trigger()

    assert backend.calls == [(0, ())]
    assert window._page_text.toPlainText() == "scanned alpha"
    assert window._document.has_ocr_result(0) is True
    assert window._document.is_dirty() is False
    assert "OCR completed for 1 page." in window.statusBar().currentMessage()


def test_open_document_loads_cached_ocr_results_without_backend(
    tmp_path: Path,
) -> None:
    _app()
    source_path = tmp_path / "ocr-cache.pdf"
    _create_image_only_pdf(source_path)
    cache = OcrCache(cache_dir=tmp_path / "ocr-cache")
    backend = _FakeOcrBackend({0: _build_ocr_page_result(0, "cached text")})

    first_window = MainWindow(ocr_backend=backend, ocr_cache=cache)
    first_window._thread_pool = _ImmediateThreadPool()
    first_window._open_document(source_path)
    first_window._run_ocr_page_action.trigger()

    assert backend.calls == [(0, ())]

    second_window = MainWindow(ocr_cache=cache)
    second_window._thread_pool = _NoOpThreadPool()
    second_window._open_document(source_path)

    assert second_window._document.has_ocr_result(0) is True
    assert second_window._document.page_text(0) == "cached text"
    assert second_window._page_text.toPlainText() == "cached text"


def test_ocr_action_shows_tesseract_setup_guidance_when_backend_missing(
    monkeypatch,
    tmp_path: Path,
) -> None:
    _app()
    source_path = tmp_path / "ocr-guidance.pdf"
    _create_image_only_pdf(source_path)
    shown_errors: list[str] = []

    monkeypatch.setattr(
        MainWindow,
        "_show_error",
        lambda self, message: shown_errors.append(message),
    )

    window = MainWindow(
        ocr_unavailable_message=(
            "OCR is unavailable because Tesseract is not fully configured.\n\n"
            "- Install the optional Python OCR dependencies with "
            '"pip install -e .[ocr]".\n'
            "- Install Tesseract OCR and add tesseract.exe to PATH, or set "
            "TESSERACT_CMD to its full path."
        ),
    )
    window._thread_pool = _NoOpThreadPool()
    window._open_document(source_path)

    assert window._run_ocr_page_action.isEnabled() is True
    assert window._run_ocr_document_action.isEnabled() is True

    window._run_ocr_page_action.trigger()

    assert shown_errors == [
        "OCR is unavailable because Tesseract is not fully configured.\n\n"
        "- Install the optional Python OCR dependencies with "
        '"pip install -e .[ocr]".\n'
        "- Install Tesseract OCR and add tesseract.exe to PATH, or set "
        "TESSERACT_CMD to its full path.",
    ]


def test_print_document_honors_selected_page_range(
    monkeypatch, tmp_path: Path
) -> None:
    _app()
    source_path = tmp_path / "print-range.pdf"
    _create_three_page_pdf(source_path)

    dialog_state: dict[str, object] = {}
    printer_instances: list[object] = []
    painter_instances: list[object] = []
    rendered_pages: list[int] = []

    class _FakePrinter:
        class PrinterMode:
            HighResolution = object()

        class Unit:
            DevicePixel = object()

        def __init__(self, _mode: object) -> None:
            self.new_page_calls = 0
            printer_instances.append(self)

        def resolution(self) -> int:
            return 72

        def pageRect(self, _unit: object) -> QRectF:
            return QRectF(0, 0, 600, 800)

        def printRange(self) -> QAbstractPrintDialog.PrintRange:
            return QAbstractPrintDialog.PrintRange.PageRange

        def fromPage(self) -> int:
            return 2

        def toPage(self) -> int:
            return 2

        def newPage(self) -> bool:
            self.new_page_calls += 1
            return True

    class _FakePrintDialog:
        def __init__(self, printer: object, _parent: object) -> None:
            dialog_state["printer"] = printer
            dialog_state["options"] = []

        def setMinMax(self, min_page: int, max_page: int) -> None:
            dialog_state["min_max"] = (min_page, max_page)

        def setOption(
            self,
            option: QAbstractPrintDialog.PrintDialogOption,
            on: bool = True,
        ) -> None:
            options = dialog_state["options"]
            assert isinstance(options, list)
            options.append((option, on))

        def exec(self) -> int:
            return QDialog.DialogCode.Accepted

    class _FakeImage:
        def width(self) -> int:
            return 300

        def height(self) -> int:
            return 400

    class _FakePainter:
        def __init__(self) -> None:
            self.draw_calls: list[tuple[QRectF, object]] = []
            painter_instances.append(self)

        def begin(self, _printer: object) -> bool:
            return True

        def drawImage(self, target_rect: QRectF, image: object) -> None:
            self.draw_calls.append((target_rect, image))

        def end(self) -> None:
            return None

    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    window._open_document(source_path)

    def _fake_render_page_at_zoom(page_index: int, *, zoom: float) -> object:
        assert zoom == 1.0
        rendered_pages.append(page_index)
        return object()

    monkeypatch.setattr(printing_module, "QPrinter", _FakePrinter)
    monkeypatch.setattr(printing_module, "QPrintDialog", _FakePrintDialog)
    monkeypatch.setattr(printing_module, "QPainter", _FakePainter)
    monkeypatch.setattr(
        printing_module,
        "qimage_from_render",
        lambda _pixmap: _FakeImage(),
    )
    monkeypatch.setattr(
        window._document,
        "render_page_at_zoom",
        _fake_render_page_at_zoom,
    )

    window._print_document()

    assert dialog_state["min_max"] == (1, 3)
    assert dialog_state["options"] == [
        (QAbstractPrintDialog.PrintDialogOption.PrintPageRange, True),
    ]
    assert rendered_pages == [1]
    assert len(painter_instances) == 1
    painter = painter_instances[0]
    assert isinstance(painter, _FakePainter)
    assert len(painter.draw_calls) == 1
    printer = printer_instances[0]
    assert isinstance(printer, _FakePrinter)
    assert printer.new_page_calls == 0


def test_open_document_shows_specific_message_for_password_protected_pdf(
    monkeypatch,
    tmp_path: Path,
) -> None:
    _app()
    source_path = tmp_path / "protected.pdf"
    _create_password_protected_pdf(source_path)

    shown_errors: list[str] = []
    prompt_calls: list[bool] = []
    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    monkeypatch.setattr(window, "_show_error", shown_errors.append)
    monkeypatch.setattr(
        window,
        "_prompt_for_document_password",
        lambda **kwargs: (
            prompt_calls.append(kwargs["invalid_password"]) or None
        ),
    )

    window._open_document(source_path)

    assert prompt_calls == [False]
    assert shown_errors == []
    assert window._document.has_document() is False


def test_open_document_prompts_for_password_and_retries_until_success(
    monkeypatch,
    tmp_path: Path,
) -> None:
    _app()
    source_path = tmp_path / "protected-open.pdf"
    _create_password_protected_pdf(source_path)

    prompt_calls: list[bool] = []
    shown_errors: list[str] = []
    entered_passwords = iter(["wrong-pass", "user-pass"])
    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    monkeypatch.setattr(window, "_show_error", shown_errors.append)
    monkeypatch.setattr(
        window,
        "_prompt_for_document_password",
        lambda **kwargs: (
            prompt_calls.append(kwargs["invalid_password"])
            or next(entered_passwords)
        ),
    )

    window._open_document(source_path)

    assert prompt_calls == [False, True]
    assert shown_errors == []
    assert window._document.has_document() is True
    assert window._document.source_path == source_path


def test_reopening_protected_pdf_reuses_remembered_password(
    monkeypatch,
    tmp_path: Path,
) -> None:
    _app()
    source_path = tmp_path / "protected-remember.pdf"
    _create_password_protected_pdf(source_path)

    first_window = MainWindow()
    first_window._thread_pool = _NoOpThreadPool()
    prompt_calls: list[bool] = []
    monkeypatch.setattr(
        first_window,
        "_prompt_for_document_password",
        lambda **kwargs: (
            prompt_calls.append(kwargs["invalid_password"]) or "user-pass"
        ),
    )

    first_window._open_document(source_path)

    assert prompt_calls == [False]

    second_window = MainWindow()
    second_window._thread_pool = _NoOpThreadPool()
    monkeypatch.setattr(
        second_window,
        "_prompt_for_document_password",
        lambda **_kwargs: (_ for _ in ()).throw(
            AssertionError("Remembered password should avoid a second prompt."),
        ),
    )

    second_window._open_document(source_path)

    assert second_window._document.has_document() is True
    assert second_window._document.source_path == source_path


def test_invalid_remembered_password_is_forgotten_and_replaced(
    monkeypatch,
    tmp_path: Path,
) -> None:
    _app()
    source_path = tmp_path / "protected-replace.pdf"
    _create_password_protected_pdf(source_path)

    seed_window = MainWindow()
    seed_window._thread_pool = _NoOpThreadPool()
    seed_window._remember_document_password(
        path=source_path,
        password="wrong-pass",
    )

    prompt_calls: list[bool] = []
    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    monkeypatch.setattr(
        window,
        "_prompt_for_document_password",
        lambda **kwargs: (
            prompt_calls.append(kwargs["invalid_password"]) or "user-pass"
        ),
    )

    window._open_document(source_path)

    assert prompt_calls == [True]

    reopened_window = MainWindow()
    reopened_window._thread_pool = _NoOpThreadPool()
    monkeypatch.setattr(
        reopened_window,
        "_prompt_for_document_password",
        lambda **_kwargs: (_ for _ in ()).throw(
            AssertionError(
                "Updated remembered password should avoid prompting."
            ),
        ),
    )

    reopened_window._open_document(source_path)

    assert reopened_window._document.has_document() is True


def test_open_document_returns_when_password_prompt_is_cancelled(
    monkeypatch,
    tmp_path: Path,
) -> None:
    _app()
    source_path = tmp_path / "protected-cancel.pdf"
    _create_password_protected_pdf(source_path)

    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    prompt_calls: list[bool] = []
    monkeypatch.setattr(
        window,
        "_prompt_for_document_password",
        lambda **kwargs: (
            prompt_calls.append(kwargs["invalid_password"]) or None
        ),
    )

    window._open_document(source_path)

    assert prompt_calls == [False]
    assert window._document.has_document() is False


def test_cancelling_password_prompt_closes_new_tab_and_restores_previous_tab(
    monkeypatch,
    tmp_path: Path,
) -> None:
    _app()
    first_path = tmp_path / "first.pdf"
    protected_path = tmp_path / "protected.pdf"
    _create_sample_pdf(first_path)
    _create_password_protected_pdf(protected_path)

    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    window.open_document_in_tab(first_path)
    window._document.go_to_page(1)
    window._document.state.set_zoom(1.5)

    prompt_calls: list[bool] = []
    monkeypatch.setattr(
        window,
        "_prompt_for_document_password",
        lambda **kwargs: (
            prompt_calls.append(kwargs["invalid_password"]) or None
        ),
    )

    window.open_document_in_tab(protected_path)

    assert prompt_calls == [False]
    assert window._document_tabs.count() == 1
    assert window._document_tabs.currentIndex() == 0
    assert window._document.source_path == first_path
    assert window._document.state.current_page_index == 1
    assert window._document.state.zoom == 1.5


def test_clicking_pdf_uri_link_opens_external_browser(
    monkeypatch,
    tmp_path: Path,
) -> None:
    _app()
    source_path = tmp_path / "uri-link.pdf"
    _create_linked_pdf(source_path)

    opened_urls: list[str] = []
    window = MainWindow()
    window._thread_pool = _ImmediateThreadPool()
    monkeypatch.setattr(
        page_links_module.QDesktopServices,
        "openUrl",
        lambda url: opened_urls.append(url.toString()) or True,
    )

    window._open_document(source_path)
    page_label = window._page_labels[0]

    assert isinstance(page_label, PageSurfaceLabel)

    inset = PAGE_CONTENT_CHROME / 2
    page_label.mousePressEvent(
        _mouse_press_event(x=90 + inset, y=84 + inset),
    )

    assert opened_urls == ["https://example.com/spec"]
    assert window._document.state.current_page_index == 0


def test_clicking_internal_pdf_link_navigates_to_target_page(
    tmp_path: Path,
) -> None:
    _app()
    source_path = tmp_path / "internal-link.pdf"
    _create_linked_pdf(source_path)

    window = MainWindow()
    window._thread_pool = _ImmediateThreadPool()
    window.resize(900, 500)
    window.show()
    _app().processEvents()

    window._open_document(source_path)
    _app().processEvents()
    page_label = window._page_labels[0]

    assert isinstance(page_label, PageSurfaceLabel)
    assert window._document.state.current_page_index == 0

    QTest.mouseClick(
        page_label.text_layer(),
        Qt.MouseButton.LeftButton,
        pos=QPoint(90, 150),
    )
    _app().processEvents()

    inset = PAGE_CONTENT_CHROME / 2
    target_page_label = window._page_labels[1]
    target_content_y = target_page_label.y() + round(inset + 250)
    scroll_value = window._document_scroll_area.verticalScrollBar().value()

    assert window._document.state.current_page_index == 1
    assert target_content_y - scroll_value == 40


def test_stale_page_render_skips_rasterization_and_keeps_new_pending(
    monkeypatch,
    tmp_path: Path,
) -> None:
    _app()
    source_path = tmp_path / "stale-render.pdf"
    _create_sample_pdf(source_path)

    window = MainWindow()
    collecting_pool = _CollectingThreadPool()
    window._thread_pool = collecting_pool
    window._open_document(source_path)

    page_render_tasks = [
        task for task in collecting_pool.tasks if hasattr(task, "_request")
    ]
    stale_task = next(
        task for task in page_render_tasks if task._request.page_index == 0
    )
    stale_generation = stale_task._request.generation

    window._build_page_placeholders()
    window._refresh_visible_pages()

    page_zero_task_count = sum(
        task._request.page_index == 0
        for task in collecting_pool.tasks
        if hasattr(task, "_request")
    )
    assert page_zero_task_count == 2
    assert any(
        task._request.page_index == 0
        and task._request.generation != stale_generation
        for task in collecting_pool.tasks
        if hasattr(task, "_request")
    )

    def _unexpected_render(*_args, **_kwargs):
        msg = "Stale page renders should be discarded before rasterization."
        raise AssertionError(msg)

    monkeypatch.setattr(
        "plain_open_pdf.gui.views.rendering.page_rendering.render_page",
        _unexpected_render,
    )

    stale_task.run()
    _app().processEvents()

    window._queue_page_render(0)

    assert (
        sum(
            task._request.page_index == 0
            for task in collecting_pool.tasks
            if hasattr(task, "_request")
        )
        == page_zero_task_count
    )


def test_clicking_named_destination_pdf_link_navigates_to_target_page(
    tmp_path: Path,
) -> None:
    _app()
    source_path = tmp_path / "named-internal-link.pdf"
    create_named_destination_pdf(source_path)

    window = MainWindow()
    window._thread_pool = _ImmediateThreadPool()
    window.resize(900, 500)
    window.show()
    _app().processEvents()

    window._open_document(source_path)
    _app().processEvents()
    page_label = window._page_labels[0]

    assert isinstance(page_label, PageSurfaceLabel)
    assert window._document.state.current_page_index == 0

    inset = PAGE_CONTENT_CHROME / 2
    page_label.mousePressEvent(
        _mouse_press_event(x=90 + inset, y=150 + inset),
    )
    _app().processEvents()

    target_page_label = window._page_labels[1]
    target_content_y = target_page_label.y() + round(inset + 250)
    scroll_value = window._document_scroll_area.verticalScrollBar().value()

    assert window._document.state.current_page_index == 1
    assert target_content_y - scroll_value == 40


def test_pdf_link_destination_zoom_is_applied(
    monkeypatch,
    tmp_path: Path,
) -> None:
    _app()
    source_path = tmp_path / "zoom-link.pdf"
    _create_sample_pdf(source_path)

    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    window._open_document(source_path)

    applied_zooms: list[float] = []

    def _fake_apply_zoom(target_zoom: float) -> None:
        applied_zooms.append(target_zoom)
        window._document.state.set_zoom(target_zoom)

    monkeypatch.setattr(window, "_apply_zoom", _fake_apply_zoom)

    window._handle_page_link_activated(
        PdfPageLink(
            bounds=(72.0, 140.0, 152.0, 164.0),
            target_page_index=1,
            target_point=(20.0, 592.0),
            target_zoom=2.0,
        ),
    )

    assert applied_zooms == [2.0]
    assert window._document.state.zoom == 2.0
    assert window._document.state.current_page_index == 1


def test_clicking_pdf_file_link_opens_target_pdf_in_app(
    monkeypatch,
    tmp_path: Path,
) -> None:
    _app()
    source_path = tmp_path / "linked-source.pdf"
    target_path = tmp_path / "linked-target.pdf"
    _create_sample_pdf(source_path)
    create_named_destination_pdf(target_path)

    opened_urls: list[str] = []
    monkeypatch.setattr(
        page_links_module.QDesktopServices,
        "openUrl",
        lambda url: opened_urls.append(url.toString()) or True,
    )

    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    window._open_document(source_path)

    window._handle_page_link_activated(
        PdfPageLink(
            bounds=(0.0, 0.0, 1.0, 1.0),
            named_destination="chapter-2",
            file_path=target_path.name,
        ),
    )

    assert opened_urls == []
    assert window._document.source_path == target_path.resolve(strict=False)
    assert window._document.state.current_page_index == 1
    assert window._document_tabs.count() == 2


def test_clicking_local_non_pdf_file_link_opens_externally(
    monkeypatch,
    tmp_path: Path,
) -> None:
    _app()
    source_path = tmp_path / "source.pdf"
    notes_path = tmp_path / "notes.txt"
    _create_sample_pdf(source_path)
    notes_path.write_text("hello", encoding="utf-8")

    opened_urls: list[str] = []
    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    monkeypatch.setattr(
        page_links_module.QDesktopServices,
        "openUrl",
        lambda url: opened_urls.append(url.toLocalFile()) or True,
    )

    window._open_document(source_path)
    window._handle_page_link_activated(
        PdfPageLink(
            bounds=(0.0, 0.0, 1.0, 1.0),
            file_path="notes.txt",
        ),
    )

    assert [Path(path) for path in opened_urls] == [
        notes_path.resolve(strict=False),
    ]
    assert window._document.source_path == source_path


def test_clicking_file_link_with_non_file_scheme_is_ignored(
    monkeypatch,
    tmp_path: Path,
) -> None:
    _app()
    source_path = tmp_path / "source.pdf"
    _create_sample_pdf(source_path)

    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    window._open_document(source_path)
    monkeypatch.setattr(
        page_links_module.QDesktopServices,
        "openUrl",
        lambda _url: (_ for _ in ()).throw(
            AssertionError("Suspicious file links should not open externally."),
        ),
    )
    monkeypatch.setattr(
        window,
        "open_document_in_tab",
        lambda _path: (_ for _ in ()).throw(
            AssertionError("Suspicious file links should not open internally."),
        ),
    )

    window._handle_page_link_activated(
        PdfPageLink(
            bounds=(0.0, 0.0, 1.0, 1.0),
            file_path="https://example.com/attack.pdf",
        ),
    )

    assert window._document.source_path == source_path


def test_toolbar_keeps_edit_button_and_read_mode_indicator() -> None:
    _app()
    window = MainWindow()

    toolbar = window.findChild(QToolBar)

    assert toolbar is not None
    assert window._rotate_right_action.text() == "Rotate Right"
    assert window._find_action.text() == "Find"
    assert window._previous_match_action.text() == "Previous Match"
    assert window._next_match_action.text() == "Next Match"
    assert isinstance(window._mouse_mode_indicator, QToolButton)
    assert window._mouse_mode_indicator.text() == "Mode: Read"
    assert window._search_bar.isHidden() is True

    toolbar_action_texts = [
        action.text() for action in toolbar.actions() if action.text()
    ]

    assert toolbar_action_texts == ["Fit Page"]


def test_thumbnail_arrow_buttons_reorder_selected_page(tmp_path: Path) -> None:
    _app()
    source_path = tmp_path / "reorder.pdf"
    _create_three_page_pdf(source_path)

    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    window._open_document(source_path)

    assert window._move_page_up_button.isEnabled() is False
    assert window._move_page_down_button.isEnabled() is True

    window._thumbnail_list.setCurrentRow(1)
    assert window._move_page_up_button.isEnabled() is True
    assert window._move_page_down_button.isEnabled() is True

    window._move_page_down_button.click()

    assert window._document.state.current_page_index == 2
    assert window._thumbnail_list.currentRow() == 2
    assert window._move_page_up_button.isEnabled() is True
    assert window._move_page_down_button.isEnabled() is False
    assert window._document.is_dirty() is True
    assert "page 3" in window._document.page_text(1)
    assert "page 2" in window._document.page_text(2)


def test_find_is_the_only_remaining_keyboard_shortcut() -> None:
    _app()
    window = MainWindow()

    assert window._find_action.shortcut().toString() == "Ctrl+F"

    actions_without_shortcuts = [
        window._open_action,
        window._save_action,
        window._save_as_action,
        window._previous_page_shortcut_action,
        window._next_page_shortcut_action,
        window._zoom_out_shortcut_action,
        window._zoom_in_shortcut_action,
        window._actual_size_action,
        window._fit_width_action,
        window._fit_page_action,
        window._single_page_view_action,
        window._previous_match_action,
        window._next_match_action,
    ]

    assert all(
        action.shortcut().isEmpty() for action in actions_without_shortcuts
    )
    assert window._close_tab_action.shortcut().toString() == "Ctrl+W"
    assert window._exit_action.shortcut().isEmpty() is True


def test_find_action_reveals_search_bar(tmp_path: Path) -> None:
    _app()
    source_path = tmp_path / "search-reveal.pdf"
    _create_sample_pdf(source_path)

    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    window._open_document(source_path)

    assert window._search_bar.isHidden() is True

    window._find_action.trigger()

    assert window._search_bar.isHidden() is False


def test_hide_search_bar_clears_query_and_matches(tmp_path: Path) -> None:
    _app()
    source_path = tmp_path / "search-hide.pdf"
    _create_sample_pdf(source_path)

    window = MainWindow()
    window._thread_pool = _ImmediateThreadPool()
    window._open_document(source_path)
    window._find_action.trigger()
    window._search_input.setText("first")

    window._run_search()

    assert window._search.matches

    window._hide_search_bar()

    assert window._search_bar.isHidden() is True
    assert window._search_input.text() == ""
    assert window._search.matches == []


def test_search_updates_as_user_types(tmp_path: Path) -> None:
    _app()
    source_path = tmp_path / "search-live.pdf"
    _create_sample_pdf(source_path)

    window = MainWindow()
    window._thread_pool = _ImmediateThreadPool()
    window._open_document(source_path)
    window._find_action.trigger()

    window._search_input.setText("first")
    window._handle_search_text_changed("first")
    window._search_debounce_timer.stop()
    window._run_search()

    assert len(window._search.matches) == 1
    assert window._search.matches[0].page_index == 0
    assert window._search_status_label.text() == "1/1"


def test_escape_hides_search_bar(tmp_path: Path) -> None:
    _app()
    source_path = tmp_path / "search-escape.pdf"
    _create_sample_pdf(source_path)

    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    window._open_document(source_path)
    window._find_action.trigger()
    window._search_input.setText("first")

    escape_event = QKeyEvent(
        QEvent.Type.KeyPress,
        Qt.Key.Key_Escape,
        Qt.KeyboardModifier.NoModifier,
    )

    handled = window.eventFilter(window._search_input, escape_event)

    assert handled is True
    assert window._search_bar.isHidden() is True
    assert window._search_input.text() == ""


def test_thumbnail_viewport_show_and_resize_events_refresh_visible_range(
    tmp_path: Path,
) -> None:
    _app()
    source_path = tmp_path / "thumbnail-viewport-events.pdf"
    _create_sample_pdf(source_path)

    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    window._open_document(source_path)

    refresh_calls: list[QEvent.Type] = []

    def _record_refresh() -> None:
        refresh_calls.append(current_event_type)

    window._refresh_visible_thumbnails = _record_refresh  # type: ignore[method-assign]

    current_event_type = QEvent.Type.Show
    show_handled = window.eventFilter(
        window._thumbnail_list.viewport(),
        QEvent(QEvent.Type.Show),
    )

    current_event_type = QEvent.Type.Resize
    resize_handled = window.eventFilter(
        window._thumbnail_list.viewport(),
        QEvent(QEvent.Type.Resize),
    )

    assert show_handled is False
    assert resize_handled is False
    assert refresh_calls == [QEvent.Type.Show, QEvent.Type.Resize]


def test_scrolling_ignores_stale_page_geometry_cache(tmp_path: Path) -> None:
    _app()
    source_path = tmp_path / "geometry.pdf"
    _create_sample_pdf(source_path)

    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    window._open_document(source_path)

    window._page_top_offsets = []
    window._page_bottom_offsets = []

    window._handle_document_scrolled(0)


def test_single_page_view_shows_only_current_page(
    tmp_path: Path,
) -> None:
    _app()
    source_path = tmp_path / "single-page-view.pdf"
    _create_three_page_pdf(source_path)

    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    window._open_document(source_path)

    assert len(window._page_labels) == 3
    assert window._visible_page_indexes == [0, 1, 2]

    window._single_page_view_action.trigger()

    assert window._single_page_view_action.isChecked() is True
    assert len(window._page_labels) == 1
    assert window._visible_page_indexes == [0]

    window._page_selector.setValue(2)

    assert window._document.state.current_page_index == 1
    assert len(window._page_labels) == 1
    assert window._visible_page_indexes == [1]

    window._single_page_view_action.trigger()

    assert window._single_page_view_action.isChecked() is False
    assert window._document.state.current_page_index == 1
    assert len(window._page_labels) == 3
    assert window._visible_page_indexes == [0, 1, 2]


def test_single_page_view_keeps_thumbnail_navigation(
    tmp_path: Path,
) -> None:
    _app()
    source_path = tmp_path / "single-page-thumbnails.pdf"
    _create_three_page_pdf(source_path)

    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    window._open_document(source_path)

    window._single_page_view_action.trigger()
    window._thumbnail_list.setCurrentRow(2)

    assert window._document.state.current_page_index == 2
    assert len(window._page_labels) == 1
    assert window._visible_page_indexes == [2]
    assert window._thumbnail_list.currentRow() == 2


def test_single_page_view_uses_up_down_for_page_steps(
    tmp_path: Path,
) -> None:
    _app()
    source_path = tmp_path / "single-page-keys.pdf"
    _create_three_page_pdf(source_path)

    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    window._open_document(source_path)
    viewport = window._document_scroll_area.viewport()

    normal_down = QKeyEvent(
        QEvent.Type.KeyPress,
        Qt.Key.Key_Down,
        Qt.KeyboardModifier.NoModifier,
    )
    assert window.eventFilter(viewport, normal_down) is False

    window._single_page_view_action.trigger()

    down_event = QKeyEvent(
        QEvent.Type.KeyPress,
        Qt.Key.Key_Down,
        Qt.KeyboardModifier.NoModifier,
    )
    assert window.eventFilter(viewport, down_event) is True
    assert window._document.state.current_page_index == 1
    assert window._visible_page_indexes == [1]

    up_event = QKeyEvent(
        QEvent.Type.KeyPress,
        Qt.Key.Key_Up,
        Qt.KeyboardModifier.NoModifier,
    )
    assert window.eventFilter(viewport, up_event) is True
    assert window._document.state.current_page_index == 0
    assert window._visible_page_indexes == [0]


def test_zoom_keeps_current_page_visible_after_rerender(
    tmp_path: Path,
) -> None:
    app = _app()
    source_path = tmp_path / "zoom-rerender.pdf"
    create_pdf(source_path, [f"page {number}" for number in range(1, 5)])

    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    window.show()
    window._open_document(source_path)
    app.processEvents()

    window._document.go_to_page(2)
    window._sync_current_page_ui(scroll_to_page=True)
    app.processEvents()

    assert window._document.state.current_page_index == 2

    window._change_zoom(zoom_in=True)
    app.processEvents()

    assert window._document.state.current_page_index == 2
    assert window._document_scroll_area.verticalScrollBar().value() > 0


def test_zoom_preserves_page_point_under_mouse_anchor(
    tmp_path: Path,
) -> None:
    app = _app()
    source_path = tmp_path / "zoom-mouse-anchor.pdf"
    create_pdf(
        source_path,
        [f"page {number}" for number in range(1, 5)],
        width=320,
        height=240,
        point=(40.0, 40.0),
    )

    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    window.show()
    window._open_document(source_path)
    app.processEvents()

    vertical_bar = window._document_scroll_area.verticalScrollBar()
    target_page_label = window._page_labels[2]
    vertical_bar.setValue(max(target_page_label.y() - 40, 0))
    window._document.go_to_page(0, record_history=False)
    app.processEvents()

    viewport = window._document_scroll_area.viewport()
    anchor_position = QPointF(
        viewport.width() / 2,
        target_page_label.y() - vertical_bar.value() + 80,
    )

    def anchor_snapshot() -> tuple[int, float, float]:
        content_x = (
            window._document_scroll_area.horizontalScrollBar().value()
            + anchor_position.x()
        )
        content_y = vertical_bar.value() + anchor_position.y()
        for page_index, page_label in enumerate(window._page_labels):
            page_left = float(page_label.x())
            page_top = float(page_label.y())
            page_right = page_left + page_label.width()
            page_bottom = page_top + page_label.height()
            if not (
                page_left <= content_x <= page_right
                and page_top <= content_y <= page_bottom
            ):
                continue

            inset = PAGE_CONTENT_CHROME / 2
            content_width = max(page_label.width() - PAGE_CONTENT_CHROME, 1)
            content_height = max(page_label.height() - PAGE_CONTENT_CHROME, 1)
            relative_x = (
                min(
                    max(content_x - page_left - inset, 0.0),
                    content_width,
                )
                / content_width
            )
            relative_y = (
                min(
                    max(content_y - page_top - inset, 0.0),
                    content_height,
                )
                / content_height
            )
            return page_index, relative_x, relative_y

        msg = "Mouse anchor is not positioned over any page label."
        raise AssertionError(msg)

    before_page_index, before_x_ratio, before_y_ratio = anchor_snapshot()

    window._change_zoom(
        zoom_in=True,
        anchor_position=anchor_position,
    )
    app.processEvents()

    after_page_index, after_x_ratio, after_y_ratio = anchor_snapshot()

    assert before_page_index == 2
    assert after_page_index == before_page_index
    assert after_x_ratio == pytest.approx(before_x_ratio, abs=0.03)
    assert after_y_ratio == pytest.approx(before_y_ratio, abs=0.03)


def test_ctrl_wheel_zoom_uses_smaller_increment_than_shortcut_zoom(
    tmp_path: Path,
) -> None:
    app = _app()
    source_path = tmp_path / "ctrl-wheel-zoom.pdf"
    _create_sample_pdf(source_path)

    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    window.show()
    window._open_document(source_path)
    app.processEvents()

    viewport = window._document_scroll_area.viewport()
    anchor = QPointF(viewport.width() / 2, viewport.height() / 2)
    wheel_event = _ctrl_wheel_event(
        x=anchor.x(),
        y=anchor.y(),
        angle_delta_y=120,
    )

    handled = window.eventFilter(viewport, wheel_event)
    app.processEvents()

    assert handled is True
    assert window._document.state.zoom == pytest.approx(1.1, abs=0.001)


def test_ctrl_wheel_zoom_keeps_visible_page_when_mouse_is_in_margin(
    tmp_path: Path,
) -> None:
    app = _app()
    source_path = tmp_path / "ctrl-wheel-margin-anchor.pdf"
    create_pdf(
        source_path,
        [f"page {number}" for number in range(1, 5)],
        width=320,
        height=240,
        point=(40.0, 40.0),
    )

    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    window.resize(1200, 900)
    window.show()
    window._open_document(source_path)
    app.processEvents()

    vertical_bar = window._document_scroll_area.verticalScrollBar()
    target_page_label = window._page_labels[2]
    vertical_bar.setValue(max(target_page_label.y() - 40, 0))
    window._document.go_to_page(0, record_history=False)
    app.processEvents()

    viewport = window._document_scroll_area.viewport()
    anchor_x = min(
        float(target_page_label.x() + target_page_label.width()) + 20.0,
        float(viewport.width() - 1),
    )
    anchor_y = target_page_label.y() - vertical_bar.value() + 80
    wheel_event = _ctrl_wheel_event(
        x=anchor_x,
        y=anchor_y,
        angle_delta_y=120,
    )

    handled = window.eventFilter(viewport, wheel_event)
    app.processEvents()

    assert handled is True
    assert window._document.state.current_page_index == 2
    assert window._document_scroll_area.verticalScrollBar().value() > 0


def test_ctrl_wheel_zoom_burst_keeps_current_page(tmp_path: Path) -> None:
    app = _app()
    source_path = tmp_path / "ctrl-wheel-burst.pdf"
    create_pdf(source_path, [f"page {number}" for number in range(1, 7)])

    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    window.resize(1200, 900)
    window.show()
    window._open_document(source_path)
    app.processEvents()

    window._document.go_to_page(2)
    window._sync_current_page_ui(scroll_to_page=True)
    app.processEvents()

    viewport = window._document_scroll_area.viewport()
    anchor = QPointF(viewport.width() / 2, viewport.height() / 2)

    # Two wheel notches in the same event-loop turn: the second arrives
    # before the first zoom's deferred scroll restore has run.
    for _ in range(2):
        wheel_event = _ctrl_wheel_event(
            x=anchor.x(),
            y=anchor.y(),
            angle_delta_y=120,
        )
        window.eventFilter(viewport, wheel_event)
    app.processEvents()
    app.processEvents()

    assert window._document.state.current_page_index == 2
    assert window._document_scroll_area.verticalScrollBar().value() > 0


def test_ctrl_wheel_zoom_reuses_page_labels_until_the_gesture_settles(
    tmp_path: Path,
) -> None:
    app = _app()
    source_path = tmp_path / "ctrl-wheel-preview.pdf"
    _create_three_page_pdf(source_path)

    window = MainWindow()
    window._thread_pool = _ImmediateThreadPool()
    window.resize(1200, 900)
    window.show()
    window._open_document(source_path)
    app.processEvents()

    labels_before = list(window._page_labels)
    generation_before = window._generations.render
    zoom_before = window._document.state.zoom
    viewport = window._document_scroll_area.viewport()

    for _ in range(3):
        wheel_event = _ctrl_wheel_event(
            x=viewport.width() / 2,
            y=viewport.height() / 2,
            angle_delta_y=120,
        )
        assert window.eventFilter(viewport, wheel_event) is True
    app.processEvents()

    assert window._document.state.zoom > zoom_before
    assert window._page_labels == labels_before
    assert window._generations.render == generation_before
    assert window._zoom_settle_timer.isActive() is True

    window._settle_zoom_preview()
    app.processEvents()

    assert window._generations.render > generation_before
    assert window._page_labels == labels_before
    assert window._zoom_preview_gesture is None


def test_ctrl_wheel_zoom_preview_resizes_pages_like_a_rebuild(
    tmp_path: Path,
) -> None:
    app = _app()
    source_path = tmp_path / "ctrl-wheel-preview-size.pdf"
    _create_three_page_pdf(source_path)

    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    window.resize(1200, 900)
    window.show()
    window._open_document(source_path)
    app.processEvents()

    viewport = window._document_scroll_area.viewport()
    for _ in range(4):
        wheel_event = _ctrl_wheel_event(
            x=viewport.width() / 2,
            y=viewport.height() / 2,
            angle_delta_y=120,
        )
        window.eventFilter(viewport, wheel_event)
    app.processEvents()

    preview_sizes = [label.size() for label in window._page_labels]

    # A full rebuild at the same zoom is the reference layout; repeated
    # preview notches must not drift away from it.
    window._build_page_placeholders()
    app.processEvents()

    assert [label.size() for label in window._page_labels] == preview_sizes


def test_ctrl_wheel_zoom_uses_trackpad_pixel_delta(tmp_path: Path) -> None:
    app = _app()
    source_path = tmp_path / "ctrl-wheel-pixel-delta.pdf"
    _create_three_page_pdf(source_path)

    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    window.resize(1200, 900)
    window.show()
    window._open_document(source_path)
    app.processEvents()

    zoom_before = window._document.state.zoom
    viewport = window._document_scroll_area.viewport()
    wheel_event = _ctrl_wheel_event(
        x=viewport.width() / 2,
        y=viewport.height() / 2,
        angle_delta_y=0,
        pixel_delta_y=25,
    )

    assert window.eventFilter(viewport, wheel_event) is True
    app.processEvents()

    # Half a notch of travel zooms by half a notch, not a whole one.
    assert zoom_before < window._document.state.zoom < zoom_before * 1.1


def test_toolbar_zoom_cancels_a_pending_wheel_zoom_settle(
    tmp_path: Path,
) -> None:
    app = _app()
    source_path = tmp_path / "ctrl-wheel-settle-cancel.pdf"
    _create_three_page_pdf(source_path)

    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    window.resize(1200, 900)
    window.show()
    window._open_document(source_path)
    app.processEvents()

    viewport = window._document_scroll_area.viewport()
    wheel_event = _ctrl_wheel_event(
        x=viewport.width() / 2,
        y=viewport.height() / 2,
        angle_delta_y=120,
    )
    window.eventFilter(viewport, wheel_event)
    app.processEvents()

    assert window._zoom_settle_timer.isActive() is True

    window._zoom_in()
    app.processEvents()

    assert window._zoom_settle_timer.isActive() is False
    assert window._zoom_preview_gesture is None


def test_ctrl_wheel_zoom_anchor_uses_document_page_in_single_page_view(
    tmp_path: Path,
) -> None:
    app = _app()
    source_path = tmp_path / "single-page-ctrl-wheel-anchor.pdf"
    _create_three_page_pdf(source_path)

    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    window.show()
    window._open_document(source_path)
    app.processEvents()

    window._single_page_view_action.trigger()
    window._document.go_to_page(2)
    window._sync_current_page_ui(scroll_to_page=True)
    app.processEvents()

    page_label = window._page_labels[0]
    anchor_position = QPointF(
        page_label.x() + page_label.width() / 2,
        page_label.y() + page_label.height() / 2,
    )

    scroll_restore = window._capture_zoom_scroll_restore(
        anchor_position=anchor_position,
        previous_zoom=window._document.state.zoom,
        updated_zoom=window._document.state.zoom * 1.1,
    )

    assert window._visible_page_indexes == [2]
    assert scroll_restore.restore_page_index == 2
    assert scroll_restore.page_anchor is not None
    assert scroll_restore.page_anchor.page_index == 2


def test_thumbnail_context_menu_can_delete_a_page(
    monkeypatch,
    tmp_path: Path,
) -> None:
    _app()
    source_path = tmp_path / "delete-thumbnail.pdf"
    _create_sample_pdf(source_path)

    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    window._open_document(source_path)

    monkeypatch.setattr(
        QMessageBox,
        "question",
        staticmethod(
            lambda *_args, **_kwargs: QMessageBox.StandardButton.Yes,
        ),
    )

    assert window._thumbnail_list.contextMenuPolicy() == (
        Qt.ContextMenuPolicy.CustomContextMenu
    )
    assert window._thumbnail_list.count() == 2

    window._delete_thumbnail_page(0)

    assert window._document.state.page_count == 1
    assert window._document.state.current_page_index == 0
    assert window._thumbnail_list.count() == 1
    assert "second page" in window._document.page_text(0)
    assert window._save_action.isEnabled() is True


def test_thumbnail_context_menu_can_rotate_its_page(
    monkeypatch,
    tmp_path: Path,
) -> None:
    app = _app()
    source_path = tmp_path / "rotate-thumbnail.pdf"
    _create_sample_pdf(source_path)
    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    window.show()
    window._open_document(source_path)
    app.processEvents()

    monkeypatch.setattr(thumbnails_module, "QMenu", _FakeContextMenu)
    item = window._thumbnail_list.item(1)

    window._show_thumbnail_context_menu(
        window._thumbnail_list.visualItemRect(item).center(),
    )

    assert window._document.state.current_page_index == 1
    assert window._document.current_page().rotation == 90
    assert window._save_action.isEnabled() is True


def test_page_delete_renders_both_views_from_edited_document(
    monkeypatch,
    tmp_path: Path,
) -> None:
    _app()
    source_path = tmp_path / "delete-last-page.pdf"
    _create_sample_pdf(source_path)

    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    window._open_document(source_path)
    window._thread_pool = _FailingThreadPool()
    monkeypatch.setattr(
        QMessageBox,
        "question",
        staticmethod(
            lambda *_args, **_kwargs: QMessageBox.StandardButton.Yes,
        ),
    )

    window._delete_thumbnail_page(1)

    page_pixmap = window._page_labels[0].pixmap()
    thumbnail_item = window._thumbnail_list.item(0)
    assert page_pixmap is not None
    assert not page_pixmap.isNull()
    assert thumbnail_item is not None
    assert not thumbnail_item.icon().isNull()


def test_dirty_document_search_uses_in_memory_state(tmp_path: Path) -> None:
    _app()
    source_path = tmp_path / "dirty-search.pdf"
    _create_sample_pdf(source_path)

    window = MainWindow()
    try:
        window._thread_pool = _NoOpThreadPool()
        window._open_document(source_path)
        window._document.delete_page(0)
        window._search_input.setText("first page")
        window._thread_pool = _ImmediateThreadPool()

        window._run_search()

        assert window._search.matches == []
        assert "No matches found" in window.statusBar().currentMessage()
    finally:
        window._document.close_document()


def test_view_menu_toggles_panes() -> None:
    _app()
    window = MainWindow()

    window._bookmarks_pane_action.setChecked(False)
    window._thumbnails_pane_action.setChecked(False)
    window._text_pane_action.setChecked(False)

    assert window._bookmark_tree.isHidden()
    assert window._thumbnail_pane.isHidden()
    assert window._page_text.isHidden()

    window._bookmarks_pane_action.setChecked(True)
    window._thumbnails_pane_action.setChecked(True)
    window._text_pane_action.setChecked(True)

    assert not window._bookmark_tree.isHidden()
    assert not window._thumbnail_pane.isHidden()
    assert not window._page_text.isHidden()


def test_text_selection_mode_allows_bookmark_from_selected_text(
    tmp_path: Path,
) -> None:
    _app()
    source_path = tmp_path / "selection-bookmark.pdf"
    _create_sample_pdf(source_path)

    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    window._open_document(source_path)

    selection_flags = window._page_text.textInteractionFlags()
    assert (
        bool(
            selection_flags & Qt.TextInteractionFlag.TextSelectableByMouse,
        )
        is True
    )

    text_cursor = window._page_text.textCursor()
    text_cursor.setPosition(0)
    text_cursor.movePosition(
        text_cursor.MoveOperation.EndOfWord,
        text_cursor.MoveMode.KeepAnchor,
    )
    window._page_text.setTextCursor(text_cursor)

    window._add_bookmark_from_selected_text()

    assert window._bookmark_tree.topLevelItemCount() == 1
    assert window._bookmark_tree.topLevelItem(0).text(0) == "first"
    assert window._save_action.isEnabled() is True


def test_text_selection_mode_updates_toolbar_indicator_and_page_overlay(
    tmp_path: Path,
) -> None:
    _app()
    source_path = tmp_path / "page-overlay.pdf"
    _create_sample_pdf(source_path)

    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    window._open_document(source_path)

    page_label = window._page_labels[0]

    assert isinstance(page_label, PageSurfaceLabel)
    assert page_label.text_layer().isHidden() is False
    assert window._mouse_mode_indicator.text() == "Mode: Read"

    window._text_selection_mouse_mode_action.trigger()

    assert page_label.text_layer().isHidden() is False
    assert window._mouse_mode_indicator.text() == "Mode: Read"


def test_lazy_text_overlay_failures_are_logged_once_per_label_generation(
    monkeypatch,
    caplog: pytest.LogCaptureFixture,
    tmp_path: Path,
) -> None:
    _app()
    source_path = tmp_path / "page-words-failure.pdf"
    _create_sample_pdf(source_path)

    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    window._open_document(source_path)
    window._text_selection_mouse_mode_action.trigger()

    page_words_calls: list[int] = []

    def _raise_page_words(page_index: int | None = None):
        page_words_calls.append(-1 if page_index is None else page_index)
        msg = "synthetic page word extraction failure"
        raise RuntimeError(msg)

    monkeypatch.setattr(window._document, "page_words", _raise_page_words)

    with caplog.at_level(
        logging.ERROR,
        logger=selection_module.__name__,
    ):
        window._ensure_page_words_loaded(0)
        window._ensure_page_words_loaded(0)

    assert page_words_calls == [0]
    assert 0 in window._words_loaded_pages
    assert "Failed to load text overlay for page 1" in caplog.text


def test_toolbar_mouse_mode_indicator_toggles_modes(tmp_path: Path) -> None:
    _app()
    source_path = tmp_path / "toggle-mode.pdf"
    _create_sample_pdf(source_path)

    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    window._open_document(source_path)

    assert window._is_text_selection_mode is True
    assert window._mouse_mode_indicator.text() == "Mode: Read"


def test_edit_pdf_action_switches_to_edit_mode(tmp_path: Path) -> None:
    _app()
    source_path = tmp_path / "edit-mode.pdf"
    _create_sample_pdf(source_path)

    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    window._open_document(source_path)

    window._edit_pdf_action.trigger()

    assert window._is_pdf_edit_mode is True
    assert window._is_text_selection_mode is False
    assert window._mouse_mode_indicator.text() == "Mode: Edit PDF"

    window._mouse_mode_indicator.click()

    assert window._is_text_selection_mode is True
    assert window._mouse_mode_indicator.text() == "Mode: Read"

    window._mouse_mode_indicator.click()

    assert window._is_text_selection_mode is True
    assert window._mouse_mode_indicator.text() == "Mode: Read"


def test_page_overlay_selection_can_add_bookmark(
    tmp_path: Path,
) -> None:
    _app()
    source_path = tmp_path / "page-overlay-bookmark.pdf"
    _create_sample_pdf(source_path)

    window = MainWindow()
    window._thread_pool = _ImmediateThreadPool()
    window._open_document(source_path)
    window._text_selection_mouse_mode_action.trigger()

    page_label = window._page_labels[0]

    assert isinstance(page_label, PageSurfaceLabel)

    page_label.text_layer().select_word_range(0, 0)

    window._add_bookmark_from_selected_text(
        page_label.selected_text(),
        page_index=0,
    )

    assert window._bookmark_tree.topLevelItemCount() == 1
    assert window._bookmark_tree.topLevelItem(0).text(0) == "first"


def test_page_overlay_context_menu_includes_selection_actions(
    monkeypatch,
    tmp_path: Path,
) -> None:
    _app()
    source_path = tmp_path / "page-overlay-context-menu.pdf"
    _create_sample_pdf(source_path)

    window = MainWindow()
    window._thread_pool = _ImmediateThreadPool()
    window._open_document(source_path)
    window._text_selection_mouse_mode_action.trigger()

    page_label = window._page_labels[0]

    assert isinstance(page_label, PageSurfaceLabel)

    page_label.text_layer().select_word_range(0, 0)

    captured_action_texts: list[str] = []

    def _fake_popup(self, *_args, **_kwargs):
        captured_action_texts.extend(
            action.text()
            for action in self.actions()
            if not action.isSeparator()
        )
        self.close()

    monkeypatch.setattr(QMenu, "popup", _fake_popup)

    window._show_page_surface_context_menu(
        page_index=0,
        page_label=page_label,
        position=page_label.text_layer().rect().center(),
    )

    assert captured_action_texts == [
        "Copy",
        "Select All",
        "Add Bookmark From Selection",
        "Replace Text...",
        "Remove Content",
    ]


def test_edit_pdf_action_redacts_selected_rectangle(
    tmp_path: Path,
) -> None:
    _app()
    source_path = tmp_path / "edit-pdf-redact-selection.pdf"
    _create_sample_pdf(source_path)

    window = MainWindow()
    window._thread_pool = _ImmediateThreadPool()
    window._open_document(source_path)

    window._edit_pdf_action.trigger()

    assert window._is_pdf_edit_mode is True

    page_label = window._page_labels[0]

    assert isinstance(page_label, PageSurfaceLabel)

    page_label.select_region_rectangle((60.0, 60.0, 120.0, 90.0))

    window._remove_selected_text_action.trigger()

    updated_text = window._document.page_text(0)

    assert "first" not in updated_text
    assert "page" not in updated_text
    assert window._document.is_dirty() is True
    assert window._save_action.isEnabled() is True


def test_remove_content_toolbar_action_keeps_selected_page_active(
    tmp_path: Path,
) -> None:
    _app()
    source_path = tmp_path / "edit-pdf-keep-page.pdf"
    _create_three_page_pdf(source_path)

    window = MainWindow()
    window._thread_pool = _ImmediateThreadPool()
    window._open_document(source_path)
    window._edit_pdf_action.trigger()

    page_label = window._page_labels[1]

    assert isinstance(page_label, PageSurfaceLabel)

    page_label.select_region_rectangle((60.0, 60.0, 120.0, 90.0))

    window._remove_selected_text_action.trigger()

    assert window._document.state.current_page_index == 1
    assert window._thumbnail_list.spacing() == 8
    assert window._thumbnail_list.gridSize().width() >= (
        window._thumbnail_item_size().width()
    )
    assert window._thumbnail_list.gridSize().height() == (
        window._thumbnail_item_size().height()
    )
    assert all(
        window._thumbnail_list.item(row).sizeHint()
        == window._thumbnail_list.gridSize()
        for row in range(window._thumbnail_list.count())
    )


def test_remove_content_keeps_scroll_near_selected_page_region(
    tmp_path: Path,
) -> None:
    app = _app()
    source_path = tmp_path / "edit-pdf-keep-scroll.pdf"
    _create_three_page_pdf(source_path)

    window = MainWindow()
    window._thread_pool = _ImmediateThreadPool()
    window.resize(700, 400)
    window.show()
    window._open_document(source_path)
    app.processEvents()

    window._edit_pdf_action.trigger()
    page_label = window._page_labels[1]

    assert isinstance(page_label, PageSurfaceLabel)

    window._document_scroll_area.verticalScrollBar().setValue(
        max(page_label.y() - 40, 0),
    )
    app.processEvents()
    original_scroll_value = (
        window._document_scroll_area.verticalScrollBar().value()
    )

    page_label.select_region_rectangle((60.0, 60.0, 120.0, 90.0))

    window._remove_selected_text_action.trigger()
    app.processEvents()

    assert window._document.state.current_page_index == 1
    assert (
        window._document_scroll_area.verticalScrollBar().value()
        == original_scroll_value
    )


def test_bookmark_context_menu_can_rename_bookmark(
    monkeypatch,
    tmp_path: Path,
) -> None:
    _app()
    source_path = tmp_path / "rename-bookmark-ui.pdf"
    _create_sample_pdf(source_path)

    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    window._open_document(source_path)
    window._document.add_bookmark("Original bookmark", page_index=0)
    window._populate_bookmarks()

    item = window._bookmark_tree.topLevelItem(0)

    assert item is not None

    monkeypatch.setattr(
        window,
        "_prompt_for_bookmark_title",
        lambda current_title: "Renamed bookmark",
    )

    captured_action_texts: list[str] = []

    class _CapturingContextMenu(_FakeContextMenu):
        def exec(self, *_args, **_kwargs):
            captured_action_texts.extend(
                action.text()
                for action in self.actions()
                if not action.isSeparator()
            )
            return super().exec()

    monkeypatch.setattr(bookmarks_module, "QMenu", _CapturingContextMenu)

    window._show_bookmark_context_menu(
        window._bookmark_tree.visualItemRect(item).center(),
    )

    assert captured_action_texts == ["Rename Bookmark", "Delete Bookmark"]
    assert window._bookmark_tree.topLevelItem(0).text(0) == "Renamed bookmark"


def test_bookmark_context_menu_can_delete_bookmark(
    monkeypatch,
    tmp_path: Path,
) -> None:
    _app()
    source_path = tmp_path / "delete-bookmark-ui.pdf"
    _create_sample_pdf(source_path)

    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    window._open_document(source_path)
    window._document.add_bookmark("Delete me", page_index=0)
    window._document.add_bookmark("Keep me", page_index=1)
    window._populate_bookmarks()

    item = window._bookmark_tree.topLevelItem(0)

    assert item is not None

    monkeypatch.setattr(
        bookmarks_module.QMessageBox,
        "question",
        lambda *_args, **_kwargs: (
            bookmarks_module.QMessageBox.StandardButton.Yes
        ),
    )

    class _DeleteBookmarkContextMenu(_FakeContextMenu):
        def exec(self, *_args, **_kwargs):
            return self.actions()[1]

    monkeypatch.setattr(bookmarks_module, "QMenu", _DeleteBookmarkContextMenu)

    window._show_bookmark_context_menu(
        window._bookmark_tree.visualItemRect(item).center(),
    )

    assert window._bookmark_tree.topLevelItemCount() == 1
    assert window._bookmark_tree.topLevelItem(0).text(0) == "Keep me"


def test_pane_visibility_persists_between_windows() -> None:
    _app()
    first_window = MainWindow()

    first_window._bookmarks_pane_action.setChecked(False)
    first_window._text_pane_action.setChecked(False)
    first_window._notes_pane_action.setChecked(False)
    first_window.close()

    persisted_settings = json.loads(
        app_module._PANE_SETTINGS_PATH.read_text(encoding="utf-8"),
    )

    assert persisted_settings == {
        "bookmarks_visible": False,
        "thumbnails_visible": True,
        "text_visible": False,
        "notes_visible": False,
        "text_selection_mode": True,
        "thumbnails_on_left": False,
    }

    second_window = MainWindow()

    assert second_window._bookmarks_pane_action.isChecked() is False
    assert second_window._bookmark_tree.isHidden() is True
    assert second_window._thumbnails_pane_action.isChecked() is True
    assert second_window._thumbnail_list.isHidden() is False
    assert second_window._text_pane_action.isChecked() is False
    assert second_window._page_text.isHidden() is True
    assert second_window._notes_pane_action.isChecked() is False
    assert second_window._notes_text.isHidden() is True


def test_notes_pane_edits_are_saved_inside_pdf(tmp_path: Path) -> None:
    _app()
    source_path = tmp_path / "notes-pane.pdf"
    _create_sample_pdf(source_path)
    window = MainWindow(initial_path=source_path)

    for partial_text in ("h", "he", "hel", "hell", "hello world"):
        window._notes_text.setPlainText(partial_text)

    assert window._document.is_dirty() is True
    assert window._save_action.isEnabled() is True
    assert window._document_tabs.tabText(0) == "* notes-pane.pdf"
    assert window.windowTitle() == "Plain Open PDF - * notes-pane.pdf"

    window._save()
    window.close()

    reopened_window = MainWindow(initial_path=source_path)

    assert reopened_window._notes_text.toPlainText() == "hello world"
    assert reopened_window._document.is_dirty() is False


def test_mouse_mode_persists_between_windows() -> None:
    _app()
    first_window = MainWindow()

    first_window._browse_mouse_mode_action.trigger()
    first_window.close()

    second_window = MainWindow()

    assert second_window._is_text_selection_mode is True
    assert second_window._text_selection_mouse_mode_action.isChecked() is False
    assert second_window._browse_mouse_mode_action.isChecked() is True
    assert second_window._mouse_mode_indicator.text() == "Mode: Read"


def test_side_pane_layout_persists_between_windows() -> None:
    _app()
    first_window = MainWindow()

    first_window._thumbnails_left_side_action.trigger()
    first_window.close()

    second_window = MainWindow()
    main_splitter = second_window.centralWidget()

    assert isinstance(main_splitter, QSplitter)
    assert second_window._thumbnails_on_left is True
    assert second_window._thumbnails_left_side_action.isChecked() is True
    assert second_window._bookmarks_left_side_action.isChecked() is False
    assert main_splitter.widget(0) is second_window._thumbnail_pane
    assert main_splitter.widget(2) is second_window._bookmark_tree


def test_save_as_switches_to_new_file(monkeypatch, tmp_path: Path) -> None:
    _app()
    source_path = tmp_path / "source.pdf"
    target_path = tmp_path / "saved-as.pdf"
    _create_sample_pdf(source_path)

    window = MainWindow(initial_path=source_path)
    window._document.go_to_page(1)
    window._document.state.set_zoom(1.5)

    monkeypatch.setattr(
        QFileDialog,
        "getSaveFileName",
        staticmethod(
            lambda *_args, **_kwargs: (str(target_path), "PDF Files (*.pdf)")
        ),
    )

    window._save_as()

    assert window._document.source_path == target_path
    assert window._document.state.current_page_index == 1
    assert window._document.state.zoom == 1.5
    assert target_path.exists()


def test_rename_file_replaces_old_path(monkeypatch, tmp_path: Path) -> None:
    _app()
    source_path = tmp_path / "very long original name.pdf"
    target_path = tmp_path / "wiley gaap 2026.pdf"
    _create_sample_pdf(source_path)

    window = MainWindow(initial_path=source_path)
    window._document.go_to_page(1)
    window._document.state.set_zoom(1.5)

    monkeypatch.setattr(
        QFileDialog,
        "getSaveFileName",
        staticmethod(
            lambda *_args, **_kwargs: (str(target_path), "PDF Files (*.pdf)")
        ),
    )

    window._rename_file()

    assert window._document.source_path == target_path
    assert window._document.state.current_page_index == 1
    assert window._document.state.zoom == 1.5
    assert target_path.exists()
    assert source_path.exists() is False


def test_rename_file_can_overwrite_existing_destination(
    monkeypatch,
    tmp_path: Path,
) -> None:
    _app()
    source_path = tmp_path / "very long original name.pdf"
    target_path = tmp_path / "wiley gaap 2026.pdf"
    _create_sample_pdf(source_path)
    _create_three_page_pdf(target_path)

    window = MainWindow(initial_path=source_path)

    monkeypatch.setattr(
        QFileDialog,
        "getSaveFileName",
        staticmethod(
            lambda *_args, **_kwargs: (str(target_path), "PDF Files (*.pdf)"),
        ),
    )
    monkeypatch.setattr(
        QMessageBox,
        "question",
        staticmethod(
            lambda *_args, **_kwargs: QMessageBox.StandardButton.Yes,
        ),
    )

    window._rename_file()

    assert window._document.source_path == target_path
    assert source_path.exists() is False
    assert target_path.exists()
    assert open_pdf(target_path).page_count == 2


def test_rename_file_leaves_existing_destination_when_overwrite_cancelled(
    monkeypatch,
    tmp_path: Path,
) -> None:
    _app()
    source_path = tmp_path / "very long original name.pdf"
    target_path = tmp_path / "wiley gaap 2026.pdf"
    _create_sample_pdf(source_path)
    _create_three_page_pdf(target_path)

    window = MainWindow(initial_path=source_path)

    monkeypatch.setattr(
        QFileDialog,
        "getSaveFileName",
        staticmethod(
            lambda *_args, **_kwargs: (str(target_path), "PDF Files (*.pdf)"),
        ),
    )
    monkeypatch.setattr(
        QMessageBox,
        "question",
        staticmethod(
            lambda *_args, **_kwargs: QMessageBox.StandardButton.No,
        ),
    )

    window._rename_file()

    assert window._document.source_path == source_path
    assert source_path.exists()
    assert open_pdf(target_path).page_count == 3


def test_properties_action_updates_pdf_metadata(
    monkeypatch,
    tmp_path: Path,
) -> None:
    _app()
    source_path = tmp_path / "properties.pdf"
    _create_sample_pdf(source_path)

    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    window._open_document(source_path)

    updated_info_dict = dict(window._document.metadata().info_dict)
    updated_info_dict["title"] = "Project Plan"
    updated_info_dict["author"] = "Alex Writer"
    updated_custom_info_dict = {"edition": "2"}
    updated_xmp_xml = (
        '<?xpacket begin="\ufeff" id="W5M0MpCehiHzreSzNTczkc9d"?>\n'
        '<x:xmpmeta xmlns:x="adobe:ns:meta/">\n'
        '  <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">\n'
        '    <rdf:Description rdf:about="" '
        'xmlns:dc="http://purl.org/dc/elements/1.1/">\n'
        "      <dc:title>\n"
        "        <rdf:Alt>\n"
        '          <rdf:li xml:lang="x-default">Project Plan</rdf:li>\n'
        "        </rdf:Alt>\n"
        "      </dc:title>\n"
        "    </rdf:Description>\n"
        "  </rdf:RDF>\n"
        "</x:xmpmeta>\n"
        '<?xpacket end="w"?>'
    )

    monkeypatch.setattr(
        window,
        "_prompt_for_document_properties",
        lambda **_kwargs: (
            updated_info_dict,
            updated_custom_info_dict,
            updated_xmp_xml,
        ),
    )

    window._properties_action.trigger()

    metadata = window._document.metadata()

    assert metadata.title == "Project Plan"
    assert metadata.author == "Alex Writer"
    assert metadata.custom_info_dict == {"edition": "2"}
    assert metadata.xmp_xml == updated_xmp_xml
    assert window._save_action.isEnabled() is True


def test_properties_dialog_edits_custom_properties() -> None:
    _app()
    dialog = DocumentPropertiesDialog(
        current_info_dict={},
        current_custom_info_dict={"edition": "2"},
        current_xmp_xml="",
    )

    assert dialog._custom_table.rowCount() == 1
    assert dialog._custom_table.item(0, 0).text() == "edition"
    assert dialog._custom_table.item(0, 1).text() == "2"

    dialog._custom_table.selectRow(0)
    dialog._delete_custom_property()
    dialog._add_custom_property()
    dialog._custom_table.item(0, 0).setText("print_edition")
    dialog._custom_table.item(0, 1).setText("Second")

    _info_dict, custom_info_dict, _xmp_xml = dialog.values()

    assert custom_info_dict == {"print_edition": "Second"}


def test_rotate_rerenders_dirty_page_without_background_workers(
    tmp_path: Path,
) -> None:
    _app()
    source_path = tmp_path / "rotated.pdf"
    _create_single_page_pdf(source_path, width=200, height=100)

    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    window._open_document(source_path)
    window._thread_pool = _FailingThreadPool()

    window._rotate_current_page()

    page_label = window._page_labels[0]
    page_pixmap = page_label.pixmap()
    thumbnail_item = window._thumbnail_list.item(0)

    assert page_pixmap is not None
    assert not page_pixmap.isNull()
    assert page_label.width() == 110
    assert page_label.height() == 210
    assert thumbnail_item is not None
    assert thumbnail_item.icon().isNull() is False


def test_save_disables_save_action_after_success(tmp_path: Path) -> None:
    _app()
    source_path = tmp_path / "save.pdf"
    _create_sample_pdf(source_path)

    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()
    window._open_document(source_path)
    window._document.rotate_clockwise()
    window._refresh_actions()

    assert window._save_action.isEnabled() is True

    window._save()

    assert window._save_action.isEnabled() is False


def test_open_document_cancel_keeps_unsaved_document(
    monkeypatch,
    tmp_path: Path,
) -> None:
    _app()
    source_path = tmp_path / "source.pdf"
    next_path = tmp_path / "next.pdf"
    _create_sample_pdf(source_path)
    _create_sample_pdf(next_path)

    window = MainWindow()
    try:
        window._thread_pool = _NoOpThreadPool()
        window._open_document(source_path)
        window._document.rotate_clockwise()

        monkeypatch.setattr(
            QMessageBox,
            "question",
            staticmethod(
                lambda *_args, **_kwargs: QMessageBox.StandardButton.Cancel,
            ),
        )

        window._open_document(next_path)

        assert window._document.source_path == source_path
        assert window._document.is_dirty() is True
    finally:
        window._document.close_document()


def test_unsaved_changes_save_routes_to_save_as_when_needed(
    monkeypatch,
    tmp_path: Path,
) -> None:
    _app()
    source_path = tmp_path / "source.pdf"
    _create_sample_pdf(source_path)

    window = MainWindow()
    try:
        window._thread_pool = _NoOpThreadPool()
        window._open_document(source_path)
        window._document.rotate_clockwise()

        save_calls: list[str] = []

        monkeypatch.setattr(
            QMessageBox,
            "question",
            staticmethod(
                lambda *_args, **_kwargs: QMessageBox.StandardButton.Save,
            ),
        )
        monkeypatch.setattr(window._document, "can_save", lambda: False)

        def fake_save() -> None:
            save_calls.append("save")

        def fake_save_as() -> None:
            save_calls.append("save_as")
            window._document._is_dirty = False

        monkeypatch.setattr(window, "_save", fake_save)
        monkeypatch.setattr(window, "_save_as", fake_save_as)

        assert window._confirm_unsaved_changes() is True
        assert save_calls == ["save_as"]
    finally:
        window._document.close_document()


def test_opening_second_document_creates_new_tab_and_preserves_first_state(
    tmp_path: Path,
) -> None:
    _app()
    first_path = tmp_path / "first.pdf"
    second_path = tmp_path / "second.pdf"
    _create_sample_pdf(first_path)
    _create_sample_pdf(second_path)

    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()

    window.open_document_in_tab(first_path)
    window._document.go_to_page(1)
    window._document.state.set_zoom(1.5)

    window.open_document_in_tab(second_path)

    assert window._document_tabs.count() == 2
    assert window._document_tabs.tabText(0) == "first.pdf"
    assert window._document_tabs.tabText(1) == "second.pdf"
    assert window._document.source_path == second_path

    window._activate_document_tab(0)

    assert window._document.source_path == first_path
    assert window._document.state.current_page_index == 1
    assert window._document.state.zoom == 1.5


def test_switching_tabs_restores_each_documents_notes(tmp_path: Path) -> None:
    _app()
    first_path = tmp_path / "first-notes.pdf"
    second_path = tmp_path / "second-notes.pdf"
    _create_sample_pdf(first_path)
    _create_sample_pdf(second_path)
    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()

    window.open_document_in_tab(first_path)
    window._notes_text.setPlainText("first document notes")
    window.open_document_in_tab(second_path)
    window._notes_text.setPlainText("second document notes")

    window._activate_document_tab(0)
    assert window._notes_text.toPlainText() == "first document notes"

    window._activate_document_tab(1)
    assert window._notes_text.toPlainText() == "second document notes"


def test_opening_existing_document_path_reuses_existing_tab(
    tmp_path: Path,
) -> None:
    _app()
    first_path = tmp_path / "first.pdf"
    second_path = tmp_path / "second.pdf"
    _create_sample_pdf(first_path)
    _create_sample_pdf(second_path)

    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()

    window.open_document_in_tab(first_path)
    window.open_document_in_tab(second_path)
    window.open_document_in_tab(first_path)

    assert window._document_tabs.count() == 2
    assert window._document_tabs.currentIndex() == 0
    assert window._document.source_path == first_path


def test_drag_reordering_tabs_preserves_session_state(tmp_path: Path) -> None:
    _app()
    first_path = tmp_path / "first.pdf"
    second_path = tmp_path / "second.pdf"
    _create_sample_pdf(first_path)
    _create_sample_pdf(second_path)

    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()

    window.open_document_in_tab(first_path)
    window._document.go_to_page(1)
    window._document.state.set_zoom(1.5)
    window.open_document_in_tab(second_path)
    window._document.go_to_page(1)
    window._document.state.set_zoom(2.0)

    window._document_tabs.moveTab(1, 0)

    assert window._document_tabs.tabText(0) == "second.pdf"
    assert window._document_tabs.tabText(1) == "first.pdf"
    assert window._document.source_path == second_path
    assert window._document.state.current_page_index == 1
    assert window._document.state.zoom == 2.0

    window._activate_document_tab(1)

    assert window._document.source_path == first_path
    assert window._document.state.current_page_index == 1
    assert window._document.state.zoom == 1.5


def test_close_tab_action_closes_current_document_tab(tmp_path: Path) -> None:
    _app()
    first_path = tmp_path / "first.pdf"
    second_path = tmp_path / "second.pdf"
    _create_sample_pdf(first_path)
    _create_sample_pdf(second_path)

    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()

    window.open_document_in_tab(first_path)
    window.open_document_in_tab(second_path)

    assert window._document.source_path == second_path

    window._close_tab_action.trigger()

    assert window._document_tabs.count() == 1
    assert window._document.source_path == first_path
    assert window._document_tabs.currentIndex() == 0


def test_duplicate_filenames_use_full_paths_in_tab_labels(
    tmp_path: Path,
) -> None:
    _app()
    first_directory = tmp_path / "first"
    second_directory = tmp_path / "second"
    first_directory.mkdir()
    second_directory.mkdir()
    first_path = first_directory / "shared.pdf"
    second_path = second_directory / "shared.pdf"
    _create_sample_pdf(first_path)
    _create_sample_pdf(second_path)

    window = MainWindow()
    window._thread_pool = _NoOpThreadPool()

    window.open_document_in_tab(first_path)
    window.open_document_in_tab(second_path)

    assert window._document_tabs.tabText(0) == str(first_path)
    assert window._document_tabs.tabText(1) == str(second_path)


def test_close_event_cancel_keeps_unsaved_document_open(
    monkeypatch,
    tmp_path: Path,
) -> None:
    _app()
    source_path = tmp_path / "source.pdf"
    _create_sample_pdf(source_path)

    window = MainWindow()
    try:
        window._thread_pool = _NoOpThreadPool()
        window._open_document(source_path)
        window._document.rotate_clockwise()

        monkeypatch.setattr(
            QMessageBox,
            "question",
            staticmethod(
                lambda *_args, **_kwargs: QMessageBox.StandardButton.Cancel,
            ),
        )

        event = QCloseEvent()
        window.closeEvent(event)

        assert event.isAccepted() is False
        assert window._document.is_dirty() is True
    finally:
        window._document.close_document()


def test_save_releases_cached_render_handles_before_writing(
    monkeypatch,
    tmp_path: Path,
) -> None:
    _app()
    source_path = tmp_path / "save-handles.pdf"
    _create_sample_pdf(source_path)

    window = MainWindow()
    try:
        window._thread_pool = _NoOpThreadPool()
        window._open_document(source_path)
        window._document.rotate_clockwise()

        events: list[str] = []
        monkeypatch.setattr(
            app_module.RENDER_HANDLE_CACHE,
            "close_all",
            lambda: events.append("close_all"),
        )
        monkeypatch.setattr(
            window._document,
            "save",
            lambda: events.append("save"),
        )

        window._save()

        assert events == ["close_all", "save"]
    finally:
        window._document.close_document()


def test_rename_file_releases_cached_render_handles_before_cleanup(
    monkeypatch,
    tmp_path: Path,
) -> None:
    _app()
    source_path = tmp_path / "rename-handles.pdf"
    target_path = tmp_path / "renamed-handles.pdf"
    _create_sample_pdf(source_path)

    window = MainWindow()
    try:
        window._thread_pool = _NoOpThreadPool()
        window._open_document(source_path)

        calls: list[str] = []
        monkeypatch.setattr(
            app_module.RENDER_HANDLE_CACHE,
            "close_all",
            lambda: calls.append("close_all"),
        )
        monkeypatch.setattr(
            QFileDialog,
            "getSaveFileName",
            staticmethod(
                lambda *_args, **_kwargs: (
                    str(target_path),
                    "PDF Files (*.pdf)",
                ),
            ),
        )

        window._rename_file()

        assert calls == ["close_all"]
        assert source_path.exists() is False
        assert window._document.source_path == target_path
    finally:
        window._document.close_document()


class _RecordingThreadPool:
    """Run each task at once and keep it for later inspection."""

    def __init__(self) -> None:
        self.tasks: list[object] = []

    def start(self, task: object) -> None:
        self.tasks.append(task)
        task.run()


def _page_render_requests(pool: object, *, page_index: int = 0) -> list:
    return [
        task._request
        for task in pool.tasks
        if hasattr(task, "_request")
        and task._request.page_index == page_index
    ]


def _region_requests(pool: object, *, page_index: int = 0) -> list:
    return [
        request
        for request in _page_render_requests(pool, page_index=page_index)
        if request.region is not None
    ]


def _open_large_page_window(
    monkeypatch,
    tmp_path: Path,
    pool: object,
    *,
    page_count: int = 2,
) -> MainWindow:
    """Open pages of 1500 x 2500 points in a window far smaller than one.

    The pixel count that switches a page to region rendering is lowered so
    the pages count as large at zoom 1.0 on the offscreen platform.
    """
    _app()
    monkeypatch.setattr(region_geometry, "REGION_MIN_PAGE_PIXELS", 1_000_000)
    source_path = tmp_path / "large-pages.pdf"
    builder = PdfBuilder()
    for _ in range(page_count):
        page_index = builder.new_page(width=1500, height=2500)
        builder.insert_text(page_index, (60, 80), "alpha beta")
        builder.insert_text(page_index, (60, 2000), "gamma delta")
        builder.insert_text(page_index, (900, 2000), "omega sigma")
    builder.insert_uri_link(0, (60, 60, 200, 100), "https://example.com/a")
    builder.save(source_path)

    window = MainWindow()
    window._thread_pool = pool
    window.resize(900, 700)
    window.show()
    _app().processEvents()
    window._open_document(source_path)
    _app().processEvents()
    return window


def _search_open_document(window: MainWindow, query: str) -> None:
    """Search the open document and mark the matches on its pages."""
    window._find_action.trigger()
    window._search_input.setText(query)
    window._search_debounce_timer.stop()
    window._run_search()
    _app().processEvents()


def _pending_region_task(pool: object, window: MainWindow) -> object:
    """Return the queued region task that is page 0's pending render."""
    return next(
        task
        for task in pool.tasks
        if hasattr(task, "_request")
        and task._request.region is not None
        and task._request.page_index == 0
        and task._request.request_token == window._pending_page_indexes[0]
    )


def _visible_page_rect(window: MainWindow, page_index: int = 0) -> QRect:
    """Return the part of a page inside the viewport, in page pixels."""
    view = window._viewport_in_page(page_index)
    assert view is not None
    page_label = window._page_labels[page_index]
    return view.intersected(
        QRect(
            0,
            0,
            page_label.width() - PAGE_CONTENT_CHROME,
            page_label.height() - PAGE_CONTENT_CHROME,
        ),
    )


def _has_ink(page_label: PageSurfaceLabel, page_rect: QRect) -> bool:
    """Return whether the label paints anything dark in a page rectangle."""
    inset = PAGE_CONTENT_CHROME // 2
    image = page_label.grab(page_rect.translated(inset, inset)).toImage()
    return any(
        image.pixelColor(x, y).lightness() < 200
        for y in range(image.height())
        for x in range(image.width())
    )


def test_small_page_is_rendered_whole_without_a_base(tmp_path: Path) -> None:
    _app()
    source_path = tmp_path / "small-pages.pdf"
    _create_sample_pdf(source_path)

    window = MainWindow()
    pool = _CollectingThreadPool()
    window._thread_pool = pool
    window._open_document(source_path)

    requests = _page_render_requests(pool)
    assert len(requests) == 1
    assert requests[0].region is None
    assert requests[0].is_base is False


def test_large_page_queues_its_visible_region_then_a_base(
    monkeypatch,
    tmp_path: Path,
) -> None:
    pool = _CollectingThreadPool()
    window = _open_large_page_window(monkeypatch, tmp_path, pool)

    region_request, base_request = _page_render_requests(pool)

    assert region_request.region is not None
    assert region_request.is_base is False
    assert region_request.zoom == window._document.state.zoom
    assert QRect(*region_request.region).contains(_visible_page_rect(window))
    assert base_request.region is None
    assert base_request.is_base is True
    assert base_request.device_pixel_ratio == 1.0
    assert base_request.zoom < window._document.state.zoom
    assert base_request.request_token != region_request.request_token


def test_laying_out_large_pages_never_queues_a_whole_page(
    monkeypatch,
    tmp_path: Path,
) -> None:
    pool = _CollectingThreadPool()
    window = _open_large_page_window(monkeypatch, tmp_path, pool)

    # A discrete zoom rebuilds every label; the viewport is resized while
    # the new pages are laid out.
    window._apply_zoom(2.0)
    _app().processEvents()
    window.resize(1000, 800)
    _app().processEvents()

    requests = [
        task._request for task in pool.tasks if hasattr(task, "_request")
    ]
    assert any(request.zoom == 2.0 for request in requests)
    assert all(
        request.region is not None or request.is_base for request in requests
    )


def test_large_page_shows_a_sharp_region_over_a_base(
    monkeypatch,
    tmp_path: Path,
) -> None:
    window = _open_large_page_window(
        monkeypatch,
        tmp_path,
        _ImmediateThreadPool(),
    )
    page_label = window._page_labels[0]
    assert isinstance(page_label, PageSurfaceLabel)
    plan = window._calc_page_plan(0)
    sharp_rect = page_label.sharp_region()

    assert plan is not None
    assert sharp_rect is not None
    shown_region = window._sharp_regions[0]
    assert shown_region.contains(plan.needed)
    assert shown_region.contains(_visible_page_rect(window))
    assert shown_region != _visible_page_rect(window)
    assert sharp_rect.toRect() == shown_region
    assert not page_label.pixmap().isNull()
    assert page_label.size() == QSize(1510, 2510)
    assert 0 not in window._rendered_page_indexes
    assert 0 in window._region_page_indexes
    assert 0 in window._words_loaded_pages
    assert len(page_label._page_links) == 1

    # The region holds the same pixels as that part of the whole page.
    whole_page = qimage_from_render(window._document.render_page(0))
    sharp_image = page_label._sharp_pixmap.toImage().convertToFormat(
        whole_page.format(),
    )
    assert sharp_image == whole_page.copy(shown_region)
    assert len({sharp_image.pixel(x, 75) for x in range(60, 120)}) > 1

    # The blurry base has fewer pixels than the page and is stretched
    # over the page area of the label: text far outside the region shows
    # at its place on the page.
    assert page_label.pixmap().width() < 1500
    assert _has_ink(page_label, QRect(55, 1975, 120, 35))
    assert not _has_ink(page_label, QRect(55, 1200, 120, 35))


def test_selection_mode_loads_words_for_a_region_page(
    monkeypatch,
    tmp_path: Path,
) -> None:
    window = _open_large_page_window(
        monkeypatch,
        tmp_path,
        _ImmediateThreadPool(),
    )
    window._words_loaded_pages.clear()

    window._apply_text_selection_mode()

    assert 0 in window._words_loaded_pages


def test_scrolling_inside_the_region_queues_nothing(
    monkeypatch,
    tmp_path: Path,
) -> None:
    pool = _RecordingThreadPool()
    window = _open_large_page_window(monkeypatch, tmp_path, pool)
    vertical_bar = window._document_scroll_area.verticalScrollBar()
    requests = _page_render_requests(pool)
    request_count = len(requests)
    # The view alone first, then the view with its scroll margins, then
    # the blurry whole page.
    assert [request.is_base for request in requests] == [False, False, True]
    assert QRect(*requests[1].region).contains(QRect(*requests[0].region))
    assert requests[1].region != requests[0].region

    vertical_bar.setValue(vertical_bar.value() + 20)

    assert len(_page_render_requests(pool)) == request_count

    vertical_bar.setValue(vertical_bar.value() + 600)

    new_requests = _page_render_requests(pool)[request_count:]
    assert len(new_requests) == 1
    assert QRect(*new_requests[0].region).contains(_visible_page_rect(window))
    assert window._sharp_regions[0] == QRect(*new_requests[0].region)


def test_scrolling_while_a_region_is_pending_keeps_that_render(
    monkeypatch,
    tmp_path: Path,
) -> None:
    pool = _CollectingThreadPool()
    window = _open_large_page_window(monkeypatch, tmp_path, pool)
    vertical_bar = window._document_scroll_area.verticalScrollBar()
    pending_task = _pending_region_task(pool, window)
    assert len(_region_requests(pool)) == 1

    vertical_bar.setValue(vertical_bar.value() + 600)
    vertical_bar.setValue(vertical_bar.value() + 20)

    # The render on its way is neither replaced nor joined by another,
    # although it no longer covers the view.
    pending_region = QRect(*pending_task._request.region)
    assert not pending_region.contains(_visible_page_rect(window))
    assert len(_region_requests(pool)) == 1
    assert (
        window._pending_page_indexes[0] == pending_task._request.request_token
    )

    pending_task.run()
    _app().processEvents()

    # Its image is shown, and the view of that moment is asked for next.
    page_label = window._page_labels[0]
    assert page_label.sharp_region().toRect() == pending_region
    _, next_request = _region_requests(pool)
    assert QRect(*next_request.region).contains(_visible_page_rect(window))
    assert window._pending_page_indexes[0] == next_request.request_token
    assert sum(r.is_base for r in _page_render_requests(pool)) == 1


def test_every_region_render_is_shown_while_scrolling_continues(
    monkeypatch,
    tmp_path: Path,
) -> None:
    pool = _CollectingThreadPool()
    window = _open_large_page_window(monkeypatch, tmp_path, pool)
    vertical_bar = window._document_scroll_area.verticalScrollBar()
    page_label = window._page_labels[0]
    shown_regions = []

    # Each render takes as long as two scroll steps of 200 pixels. That
    # is more than the 256 pixels a region reaches past its view, so no
    # render still covers the view when it is done.
    for _ in range(4):
        pending_task = _pending_region_task(pool, window)
        pending_region = QRect(*pending_task._request.region)
        vertical_bar.setValue(vertical_bar.value() + 200)
        vertical_bar.setValue(vertical_bar.value() + 200)
        assert not pending_region.contains(_visible_page_rect(window))
        pending_task.run()
        _app().processEvents()
        shown_regions.append(page_label.sharp_region().toRect())
        assert shown_regions[-1] == pending_region

    # Every render reached the screen. From the second on, which has
    # scroll margins, each still overlaps the view it arrives in.
    assert len(set(map(QRect.getRect, shown_regions))) == 4
    assert shown_regions[-1].intersects(_visible_page_rect(window))
    assert len(_region_requests(pool)) == 5


def test_region_render_from_before_a_zoom_change_skips_rasterization(
    monkeypatch,
    tmp_path: Path,
) -> None:
    pool = _CollectingThreadPool()
    window = _open_large_page_window(monkeypatch, tmp_path, pool)
    stale_task = _pending_region_task(pool, window)
    window._apply_zoom(2.0)
    _app().processEvents()
    newest_request = _region_requests(pool)[-1]
    assert newest_request is not stale_task._request

    def _unexpected_render(*_args, **_kwargs):
        msg = "An outdated region render should be dropped before drawing."
        raise AssertionError(msg)

    monkeypatch.setattr(
        "plain_open_pdf.gui.views.rendering.page_rendering.render_page_region",
        _unexpected_render,
    )
    stale_task.run()
    _app().processEvents()

    page_label = window._page_labels[0]
    assert window._pending_page_indexes[0] == newest_request.request_token
    assert page_label.sharp_region() is None


def test_scrolling_sideways_renders_the_newly_visible_region(
    monkeypatch,
    tmp_path: Path,
) -> None:
    pool = _RecordingThreadPool()
    window = _open_large_page_window(monkeypatch, tmp_path, pool)
    horizontal_bar = window._document_scroll_area.horizontalScrollBar()
    region_before = QRect(window._sharp_regions[0])

    horizontal_bar.setValue(700)

    visible_rect = _visible_page_rect(window)
    assert visible_rect.left() > 600
    assert not region_before.contains(visible_rect)
    assert window._sharp_regions[0].contains(visible_rect)
    assert window._page_labels[0].sharp_region().toRect().contains(
        visible_rect,
    )


def test_growing_the_viewport_renders_the_newly_visible_region(
    monkeypatch,
    tmp_path: Path,
) -> None:
    pool = _RecordingThreadPool()
    window = _open_large_page_window(monkeypatch, tmp_path, pool)
    region_before = QRect(window._sharp_regions[0])

    window.resize(1500, 1300)
    _app().processEvents()

    visible_rect = _visible_page_rect(window)
    assert not region_before.contains(visible_rect)
    assert window._sharp_regions[0].contains(visible_rect)
    assert 0 not in window._rendered_page_indexes


def test_ctrl_wheel_zoom_keeps_the_region_until_it_settles(
    monkeypatch,
    tmp_path: Path,
) -> None:
    app = _app()
    pool = _RecordingThreadPool()
    window = _open_large_page_window(monkeypatch, tmp_path, pool)
    page_label = window._page_labels[0]
    labels_before = list(window._page_labels)
    generation_before = window._generations.render
    zoom_before = window._document.state.zoom
    region_before = page_label.sharp_region()
    request_count = len(_page_render_requests(pool))
    viewport = window._document_scroll_area.viewport()

    for _ in range(3):
        wheel_event = _ctrl_wheel_event(
            x=viewport.width() / 2,
            y=viewport.height() / 2,
            angle_delta_y=120,
        )
        assert window.eventFilter(viewport, wheel_event) is True
    app.processEvents()

    zoom_after = window._document.state.zoom
    assert zoom_after > zoom_before
    assert window._page_labels == labels_before
    assert window._generations.render == generation_before
    assert page_label.sharp_region() == region_before
    assert page_label._sharp_zoom == zoom_before
    assert page_label._page_zoom == zoom_after
    assert len(_page_render_requests(pool)) == request_count
    # Both images follow the page: the text in the region and the text
    # on the blurry base sit at their zoomed places.
    scale = zoom_after / zoom_before
    assert _has_ink(
        page_label,
        QRect(int(55 * scale), int(60 * scale), int(120 * scale), 35),
    )
    assert _has_ink(
        page_label,
        QRect(int(55 * scale), int(1975 * scale), int(120 * scale), 35),
    )
    assert not _has_ink(
        page_label,
        QRect(int(55 * scale), int(1200 * scale), int(120 * scale), 35),
    )

    window._settle_zoom_preview()
    app.processEvents()

    new_requests = _page_render_requests(pool)[request_count:]
    assert window._generations.render > generation_before
    assert window._page_labels == labels_before
    assert new_requests
    assert all(request.region is not None for request in new_requests)
    assert all(request.zoom == zoom_after for request in new_requests)
    assert page_label._sharp_zoom == zoom_after
    assert window._sharp_regions[0].contains(_visible_page_rect(window))


def test_zooming_out_replaces_the_region_with_the_whole_page(
    monkeypatch,
    tmp_path: Path,
) -> None:
    app = _app()
    window = _open_large_page_window(
        monkeypatch,
        tmp_path,
        _ImmediateThreadPool(),
    )
    page_label = window._page_labels[0]
    assert page_label.sharp_region() is not None

    # At half size the page has fewer pixels than the region threshold.
    window._apply_zoom_preview(0.5, anchor_position=QPointF(10.0, 10.0))
    window._settle_zoom_preview()
    app.processEvents()

    assert window._document.state.zoom == 0.5
    assert 0 in window._rendered_page_indexes
    assert 0 not in window._region_page_indexes
    assert 0 not in window._sharp_regions
    assert page_label.sharp_region() is None
    assert page_label.pixmap().size() == QSize(750, 1250)
    assert page_label.size() == QSize(760, 1260)


def test_far_off_region_pages_are_evicted(
    monkeypatch,
    tmp_path: Path,
) -> None:
    window = _open_large_page_window(
        monkeypatch,
        tmp_path,
        _ImmediateThreadPool(),
        page_count=8,
    )
    first_label = window._page_labels[0]
    assert first_label.sharp_region() is not None
    assert not first_label.pixmap().isNull()

    vertical_bar = window._document_scroll_area.verticalScrollBar()
    vertical_bar.setValue(vertical_bar.maximum())

    assert first_label.sharp_region() is None
    assert first_label.pixmap().isNull()
    assert first_label.text() == "Loading page 1..."
    assert 0 not in window._region_page_indexes
    assert 0 not in window._sharp_regions
    assert window._page_labels[7].sharp_region() is not None
    assert 7 in window._region_page_indexes


def test_search_match_is_marked_at_its_place_in_a_region(
    monkeypatch,
    tmp_path: Path,
) -> None:
    window = _open_large_page_window(
        monkeypatch,
        tmp_path,
        _ImmediateThreadPool(),
    )
    window._find_action.trigger()
    window._search_input.setText("alpha")
    window._search_debounce_timer.stop()
    window._run_search()
    _app().processEvents()

    page_label = window._page_labels[0]
    sharp_rect = page_label.sharp_region()
    rectangles = window._search_rectangles_for_page(0)
    assert sharp_rect is not None
    assert rectangles
    x0, y0, x1, y1 = rectangles[0]
    center = QPoint(round((x0 + x1) / 2), round((y0 + y1) / 2))
    outside = QPoint(round(x0) - 12, round(y0) - 12)
    origin = sharp_rect.topLeft().toPoint()
    assert sharp_rect.toRect().contains(center)
    assert sharp_rect.toRect().contains(outside)

    plain_page = qimage_from_render(window._document.render_page(0))
    sharp_image = page_label._sharp_pixmap.toImage()
    base_image = page_label.pixmap().toImage()
    base_scale = base_image.width() / 1500

    assert sharp_image.pixelColor(center - origin) != plain_page.pixelColor(
        center,
    )
    assert sharp_image.pixelColor(outside - origin) == plain_page.pixelColor(
        outside,
    )
    # The mark on the blurry base sits over the same part of the page.
    marked = base_image.pixelColor(
        int(center.x() * base_scale),
        int(center.y() * base_scale),
    )
    assert marked.blue() < marked.red()


def test_dirty_large_page_is_rendered_whole_without_background_work(
    monkeypatch,
    tmp_path: Path,
) -> None:
    window = _open_large_page_window(
        monkeypatch,
        tmp_path,
        _ImmediateThreadPool(),
    )
    window._thread_pool = _FailingThreadPool()

    window._rotate_current_page()

    page_label = window._page_labels[0]
    assert 0 in window._rendered_page_indexes
    assert page_label.sharp_region() is None
    assert page_label.pixmap().size() == QSize(2500, 1500)
    assert not window._region_page_indexes


def test_failed_region_render_clears_the_region(
    monkeypatch,
    tmp_path: Path,
) -> None:
    window = _open_large_page_window(
        monkeypatch,
        tmp_path,
        _ImmediateThreadPool(),
    )
    page_label = window._page_labels[0]
    assert page_label.sharp_region() is not None

    def _failing_render(*_args, **_kwargs):
        msg = "damaged page"
        raise ValueError(msg)

    monkeypatch.setattr(
        "plain_open_pdf.gui.views.rendering.page_rendering.render_page_region",
        _failing_render,
    )
    vertical_bar = window._document_scroll_area.verticalScrollBar()
    vertical_bar.setValue(vertical_bar.value() + 600)

    assert page_label.sharp_region() is None
    assert 0 not in window._sharp_regions
    assert 0 not in window._pending_page_indexes


def test_small_page_is_rendered_whole_while_pages_are_laid_out(
    tmp_path: Path,
) -> None:
    _app()
    source_path = tmp_path / "small-pages.pdf"
    _create_sample_pdf(source_path)
    window = MainWindow()
    pool = _CollectingThreadPool()
    window._thread_pool = pool
    window._open_document(source_path)
    pool.tasks.clear()
    window._pending_page_indexes.clear()
    window._page_top_offsets = []
    window._page_bottom_offsets = []

    window._handle_document_scrolled(0)

    requests = _page_render_requests(pool)
    assert len(requests) == 1
    assert requests[0].region is None
    assert requests[0].is_base is False


def test_base_arriving_during_a_zoom_gesture_follows_the_page(
    monkeypatch,
    tmp_path: Path,
) -> None:
    app = _app()
    pool = _CollectingThreadPool()
    window = _open_large_page_window(monkeypatch, tmp_path, pool)
    page_label = window._page_labels[0]
    base_task = next(
        task
        for task in pool.tasks
        if hasattr(task, "_request") and task._request.is_base
    )
    anchor = QPointF(10.0, 10.0)

    window._apply_zoom_preview(1.5, anchor_position=anchor)
    assert page_label.pixmap().isNull()
    base_task.run()
    app.processEvents()
    assert not page_label.pixmap().isNull()
    assert _has_ink(page_label, QRect(80, 2960, 190, 55))

    window._apply_zoom_preview(2.0, anchor_position=anchor)

    assert window._document.state.zoom == 2.0
    assert page_label.size() == QSize(3010, 5010)
    assert _has_ink(page_label, QRect(110, 3950, 250, 70))
    assert not _has_ink(page_label, QRect(80, 2960, 190, 55))


def test_page_that_fails_to_render_keeps_its_message_while_scrolling(
    monkeypatch,
    tmp_path: Path,
) -> None:
    def _failing_render(*_args, **_kwargs):
        msg = "a page image of this size"
        raise ValueError(msg)

    # Only the sharp region fails, as it does for a page whose whole
    # image is over the engine's size limit; the small blurry page works.
    monkeypatch.setattr(
        "plain_open_pdf.gui.views.rendering.page_rendering.render_page_region",
        _failing_render,
    )
    pool = _RecordingThreadPool()
    window = _open_large_page_window(monkeypatch, tmp_path, pool)
    page_label = window._page_labels[0]
    vertical_bar = window._document_scroll_area.verticalScrollBar()
    request_count = len(pool.tasks)
    message = page_label.text()
    assert message.startswith("Failed to render page 1")
    assert page_label.pixmap().isNull()
    assert not any(r.is_base for r in _page_render_requests(pool))

    for _ in range(5):
        vertical_bar.setValue(vertical_bar.value() + 40)

        assert len(pool.tasks) == request_count
        assert page_label.text() == message
        assert page_label.pixmap().isNull()
        assert page_label.sharp_region() is None

    # A new zoom is a new chance: the page is asked for again.
    window._apply_zoom(1.25)
    _app().processEvents()
    assert len(_region_requests(pool)) > 0
    assert len(pool.tasks) > request_count


def test_blurry_page_arriving_after_a_failure_leaves_the_message(
    monkeypatch,
    tmp_path: Path,
) -> None:
    pool = _CollectingThreadPool()
    window = _open_large_page_window(monkeypatch, tmp_path, pool)
    page_label = window._page_labels[0]
    region_task = _pending_region_task(pool, window)
    base_task = next(
        task
        for task in pool.tasks
        if hasattr(task, "_request")
        and task._request.is_base
        and task._request.page_index == 0
    )

    def _failing_render(*_args, **_kwargs):
        msg = "a page image of this size"
        raise ValueError(msg)

    monkeypatch.setattr(
        "plain_open_pdf.gui.views.rendering.page_rendering.render_page_region",
        _failing_render,
    )
    region_task.run()
    base_task.run()
    _app().processEvents()

    assert page_label.text().startswith("Failed to render page 1")
    assert page_label.pixmap().isNull()


def test_page_mostly_covered_by_the_view_is_whole_before_it_scrolls_in(
    monkeypatch,
    tmp_path: Path,
) -> None:
    # With this share a view of the test window, plus its margins, covers
    # enough of a page to render it whole.
    monkeypatch.setattr(region_geometry, "REGION_MAX_SHARE", 0.1)
    pool = _CollectingThreadPool()
    window = _open_large_page_window(monkeypatch, tmp_path, pool)

    # Page 1 is still below the view. It gets the same whole-page render
    # as page 0, not a blurry stand-in.
    assert window._visible_label_range() == (0, 0)
    assert window._is_large_page(1)
    for page_index in (0, 1):
        (request,) = _page_render_requests(pool, page_index=page_index)
        assert request.region is None
        assert request.is_base is False


def test_large_page_is_not_rendered_whole_while_pages_are_laid_out(
    monkeypatch,
    tmp_path: Path,
) -> None:
    pool = _CollectingThreadPool()
    window = _open_large_page_window(monkeypatch, tmp_path, pool)
    pool.tasks.clear()
    window._pending_page_indexes.clear()
    window._pending_base_pages.clear()
    window._page_top_offsets = []
    window._page_bottom_offsets = []

    window._handle_document_scrolled(0)

    assert window._is_large_page(0)
    assert pool.tasks == []


def test_search_match_is_marked_at_its_place_in_a_scrolled_region(
    monkeypatch,
    tmp_path: Path,
) -> None:
    window = _open_large_page_window(
        monkeypatch,
        tmp_path,
        _ImmediateThreadPool(),
    )
    plain_page = qimage_from_render(window._document.render_page(0))
    _search_open_document(window, "omega")
    window._document_scroll_area.horizontalScrollBar().setValue(600)
    window._document_scroll_area.verticalScrollBar().setValue(1700)
    _app().processEvents()

    page_label = window._page_labels[0]
    sharp_rect = page_label.sharp_region()
    (rectangle,) = window._search_rectangles_for_page(0)
    assert sharp_rect is not None
    x0, y0, x1, y1 = rectangle
    center = QPoint(round((x0 + x1) / 2), round((y0 + y1) / 2))
    outside = QPoint(round(x0) - 12, round(y0) - 12)
    origin = sharp_rect.topLeft().toPoint()
    # The region starts well inside the page in both directions.
    assert origin.x() >= 64
    assert origin.y() >= 64
    assert sharp_rect.toRect().contains(center)
    assert sharp_rect.toRect().contains(outside)

    sharp_image = page_label._sharp_pixmap.toImage()
    marked = sharp_image.pixelColor(center - origin)
    assert marked != plain_page.pixelColor(center)
    assert marked.blue() < marked.red()
    assert sharp_image.pixelColor(outside - origin) == plain_page.pixelColor(
        outside,
    )


def test_region_landing_during_a_zoom_gesture_keeps_its_own_zoom(
    monkeypatch,
    tmp_path: Path,
) -> None:
    app = _app()
    window = _open_large_page_window(
        monkeypatch,
        tmp_path,
        _ImmediateThreadPool(),
    )
    plain_page = qimage_from_render(window._document.render_page(0))
    _search_open_document(window, "gamma")
    # From here on renders wait until the test runs them. Scrolling to
    # the match asks for a region that starts far down the page.
    pool = _CollectingThreadPool()
    window._thread_pool = pool
    window._document_scroll_area.verticalScrollBar().setValue(1900)
    region_task = _pending_region_task(pool, window)
    page_label = window._page_labels[0]
    assert region_task._request.region[1] >= 64

    window._apply_zoom_preview(1.5, anchor_position=QPointF(10.0, 10.0))
    region_task.run()
    app.processEvents()

    # The region is placed at the zoom it was rendered for and stretched
    # by the label; it does not count as covering the view at the new
    # zoom.
    assert window._document.state.zoom == 1.5
    assert page_label._sharp_zoom == 1.0
    assert page_label._page_zoom == 1.5
    assert page_label.sharp_region().toRect() == QRect(
        *region_task._request.region,
    )
    assert not window._sharp_regions[0].contains(
        QRect(*region_task._request.region),
    )

    # The search mark sits on the matched text inside the region image.
    (rectangle,) = window._search_rectangles_for_page(0)
    x0, y0, x1, y1 = rectangle
    center = QPoint(round((x0 + x1) / 2), round((y0 + y1) / 2))
    scaled_center = QPoint(round(center.x() * 1.5), round(center.y() * 1.5))
    origin = page_label.sharp_region().topLeft().toPoint()
    sharp_image = page_label._sharp_pixmap.toImage()
    marked = sharp_image.pixelColor(center - origin)
    assert marked != plain_page.pixelColor(center)
    assert marked.blue() < marked.red()
    assert sharp_image.pixelColor(
        scaled_center - origin,
    ) == plain_page.pixelColor(scaled_center)


def test_zooming_in_by_gesture_replaces_the_coarse_page_image(
    monkeypatch,
    tmp_path: Path,
) -> None:
    app = _app()
    pool = _RecordingThreadPool()
    window = _open_large_page_window(monkeypatch, tmp_path, pool)
    page_label = window._page_labels[0]
    anchor = QPointF(10.0, 10.0)

    # At this size the page is rendered whole, with few pixels.
    window._apply_zoom_preview(0.4, anchor_position=anchor)
    window._settle_zoom_preview()
    app.processEvents()
    assert 0 in window._rendered_page_indexes
    assert page_label.pixmap().size() == QSize(600, 1000)
    request_count = len(_page_render_requests(pool))

    window._apply_zoom_preview(2.0, anchor_position=anchor)
    window._settle_zoom_preview()
    app.processEvents()

    # The page is shown in part now. The small image from before would
    # be far coarser than the blurry page a freshly opened page gets.
    new_requests = _page_render_requests(pool)[request_count:]
    assert 0 not in window._rendered_page_indexes
    assert page_label.sharp_region() is not None
    assert sum(request.is_base for request in new_requests) == 1
    base_pixmap = page_label.pixmap()
    assert base_pixmap.width() * base_pixmap.height() > 1_300_000
    assert _has_ink(page_label, QRect(110, 3950, 250, 70))

    # Scrolling on does not ask for the blurry page again.
    vertical_bar = window._document_scroll_area.verticalScrollBar()
    vertical_bar.setValue(vertical_bar.value() + 900)
    later_requests = _page_render_requests(pool)[request_count:]
    assert sum(request.is_base for request in later_requests) == 1


def test_zoom_gesture_renders_nothing_when_shrinking_pages_move_the_view(
    monkeypatch,
    tmp_path: Path,
) -> None:
    app = _app()
    pool = _RecordingThreadPool()
    window = _open_large_page_window(monkeypatch, tmp_path, pool)
    scroll_area = window._document_scroll_area
    scroll_area.verticalScrollBar().setValue(
        scroll_area.verticalScrollBar().maximum(),
    )
    scroll_area.horizontalScrollBar().setValue(
        scroll_area.horizontalScrollBar().maximum(),
    )
    app.processEvents()
    request_count = len(pool.tasks)
    rendered_before = set(window._rendered_page_indexes)
    anchor = QPointF(10.0, 10.0)

    # Each step shrinks the pages, which pulls the scroll bars back. The
    # last step is small enough for whole-page rendering.
    for zoom in (0.9, 0.6, 0.5):
        window._apply_zoom_preview(zoom, anchor_position=anchor)

        assert len(pool.tasks) == request_count
        assert window._rendered_page_indexes == rendered_before

    window._settle_zoom_preview()
    app.processEvents()

    new_requests = [task._request for task in pool.tasks[request_count:]]
    assert new_requests
    assert all(request.zoom == 0.5 for request in new_requests)


@pytest.mark.parametrize(
    ("points", "zoom", "ratio"),
    [
        (1500.0, 1.0, 1.0),
        (612.0, 4.0, 1.75),
        (595.3, 2.6, 1.75),
        (841.89, 3.17, 1.5),
        (100.2, 1.37, 1.25),
        (100.9, 1.0, 2.0),
    ],
)
@pytest.mark.parametrize("is_whole_page_size", [False, True])
def test_only_regions_starting_on_the_page_image_are_asked_for(
    points: float,
    zoom: float,
    ratio: float,
    *,
    is_whole_page_size: bool,
) -> None:
    _app()
    # What the engine makes of the page: its sides rounded up.
    image_width = math.ceil(points * zoom * ratio)
    image_height = math.ceil(points * 1.5 * zoom * ratio)
    # A label is sized from the page before its render, and again from
    # the whole-page image once that arrives.
    if is_whole_page_size:
        content = QSize(round(image_width / ratio), round(image_height / ratio))
    else:
        content = QSize(
            *display_size_for_page_size((points, points * 1.5), zoom=zoom),
        )
    page_label = PageSurfaceLabel()
    page_label.setFixedSize(
        content.width() + PAGE_CONTENT_CHROME,
        content.height() + PAGE_CONTENT_CHROME,
    )
    planned = region_queue._calc_page_pixel_size(page_label, ratio)
    assert planned.width() >= image_width
    assert planned.height() >= image_height

    def _is_asked_for(left: int, top: int) -> bool:
        return region_queue._is_region_on_page(
            QRect(left, top, 64, 64),
            page_label,
            ratio,
        )

    lefts = [x for x in range(planned.width() + 1) if _is_asked_for(x, 0)]
    tops = [y for y in range(planned.height() + 1) if _is_asked_for(0, y)]

    # Every start that is asked for lies on the image the engine makes,
    # and only the last few pixels of the page are given up for that.
    slack = math.ceil(2 * ratio) + 3
    assert lefts == list(range(len(lefts)))
    assert tops == list(range(len(tops)))
    assert image_width - slack <= lefts[-1] < image_width
    assert image_height - slack <= tops[-1] < image_height
    assert not _is_asked_for(lefts[-1], tops[-1] + 1)
    assert not region_queue._is_region_on_page(QRect(), page_label, ratio)
