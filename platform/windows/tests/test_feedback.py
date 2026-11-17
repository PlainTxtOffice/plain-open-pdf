# ruff: noqa: E402

import os
import sys

from importlib import metadata
from pathlib import Path
from urllib.parse import parse_qs, urlsplit

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")
import pytest

from PySide6.QtGui import QAction
from PySide6.QtGui import QDesktopServices
from PySide6.QtWidgets import QApplication
from PySide6.QtWidgets import QMenu
from PySide6.QtWidgets import QPushButton

import plain_open_pdf.gui.views.root_window.window as app_module
from plain_open_pdf.gui.app import MainWindow
from plain_open_pdf.gui.views.menus.help import feedback


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


@pytest.fixture
def opened_urls(monkeypatch) -> list[str]:
    urls: list[str] = []

    def open_url(url) -> bool:
        urls.append(url.toString())
        return True

    monkeypatch.setattr(QDesktopServices, "openUrl", open_url)
    return urls


def _app() -> QApplication:
    return QApplication.instance() or QApplication(sys.argv)


def test_mailto_carries_subject_and_summary_only() -> None:
    summary = feedback.format_system_summary(
        app_name="Plain Open PDF",
        version="1.0.1",
        windows_release="11",
        windows_build="10.0.26300",
        runtime="AMD64, Qt 6.12.0",
    )

    url = feedback.feedback_mailto(
        "Plain Open PDF",
        feedback.FeedbackKind.BUG_REPORT,
        summary,
    )

    parts = urlsplit(url)
    query = parse_qs(parts.query)
    assert parts.scheme == "mailto"
    assert parts.path == feedback.FEEDBACK_ADDRESS
    assert query["subject"] == ["Plain Open PDF bug report (Windows)"]
    assert query["body"] == [
        "\r\n\r\nPlain Open PDF 1.0.1\r\nWindows 11 (10.0.26300)\r\nAMD64, Qt 6.12.0",
    ]
    assert set(query) == {"subject", "body"}


def test_mailto_names_feature_requests_and_encodes_reserved_characters() -> None:
    url = feedback.feedback_mailto(
        "A&B ?=#",
        feedback.FeedbackKind.FEATURE_REQUEST,
        "x",
    )

    query = parse_qs(urlsplit(url).query)
    assert query["subject"] == ["A&B ?=# feature request (Windows)"]
    assert query["body"] == ["\r\n\r\nx"]


def test_version_comes_from_installed_metadata() -> None:
    assert feedback.app_version("plain-open-pdf") == metadata.version("plain-open-pdf")
    assert feedback.app_version("no-such-distribution") == "unknown"


def test_bug_button_drafts_email_and_closes(opened_urls: list[str]) -> None:
    _app()
    dialog = feedback.FeedbackDialog("Plain Open PDF", "plain-open-pdf")

    dialog.findChild(QPushButton, "feedbackReportBugButton").click()

    assert len(opened_urls) == 1
    query = parse_qs(urlsplit(opened_urls[0]).query)
    assert query["subject"] == ["Plain Open PDF bug report (Windows)"]
    version = metadata.version("plain-open-pdf")
    assert query["body"][0].startswith(
        f"\r\n\r\nPlain Open PDF {version}\r\nWindows ",
    )
    assert dialog.result() == feedback.FeedbackDialog.DialogCode.Accepted


def test_contact_button_opens_contact_form(opened_urls: list[str]) -> None:
    _app()
    dialog = feedback.FeedbackDialog("Plain Open PDF", "plain-open-pdf")

    dialog.findChild(QPushButton, "feedbackContactFormButton").click()

    assert opened_urls == [feedback.CONTACT_URL]
    assert dialog.result() == feedback.FeedbackDialog.DialogCode.Accepted


def test_missing_email_app_keeps_dialog_open_with_alternative(
    monkeypatch,
) -> None:
    _app()
    monkeypatch.setattr(QDesktopServices, "openUrl", lambda _url: False)
    dialog = feedback.FeedbackDialog("Plain Open PDF", "plain-open-pdf")

    dialog.findChild(QPushButton, "feedbackRequestFeatureButton").click()

    assert not dialog.error_label.isHidden()
    assert "Open Contact Form" in dialog.error_label.text()
    assert dialog.result() == feedback.FeedbackDialog.DialogCode.Rejected


def test_failed_contact_form_suggests_email(monkeypatch) -> None:
    _app()
    monkeypatch.setattr(QDesktopServices, "openUrl", lambda _url: False)
    dialog = feedback.FeedbackDialog("Plain Open PDF", "plain-open-pdf")

    dialog.findChild(QPushButton, "feedbackContactFormButton").click()

    assert not dialog.error_label.isHidden()
    assert "Report a Bug" in dialog.error_label.text()
    assert dialog.result() == feedback.FeedbackDialog.DialogCode.Rejected


def test_help_menu_send_feedback_opens_dialog(monkeypatch) -> None:
    _app()
    shown: list[tuple[object, str, str]] = []
    monkeypatch.setattr(
        feedback,
        "show_feedback_dialog",
        lambda *args: shown.append(args),
    )
    window = MainWindow()

    menus = {
        menu.title(): menu for menu in window.menuBar().findChildren(QMenu)
    }
    help_actions = menus["Help"].actions()
    labels = [
        "-" if action.isSeparator() else action.text()
        for action in help_actions
    ]
    action = window.findChild(QAction, "sendFeedbackAction")

    assert labels == ["Send &Feedback...", "-", "About Plain Open PDF"]
    assert action is help_actions[0]

    action.trigger()

    assert shown == [(window, "Plain Open PDF", "plain-open-pdf")]
