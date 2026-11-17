"""Help menu helpers for the PySide6 PDF window."""

# ruff: noqa: SLF001

from __future__ import annotations

from typing import TYPE_CHECKING, cast

from PySide6.QtGui import QAction

from plain_open_pdf.gui.views.menus.help.feedback import add_feedback_action

if TYPE_CHECKING:
    from PySide6.QtCore import QObject
    from PySide6.QtWidgets import QMenu, QWidget

    from plain_open_pdf.gui.views.menus.menu_window_protocol import MenuWindow

APP_NAME = "Plain Open PDF"
DISTRIBUTION = "plain-open-pdf"


def populate_help_menu(help_menu: QMenu, window: MenuWindow) -> None:
    """Populate the Help menu actions."""
    add_help_actions(window)
    add_feedback_action(
        help_menu, cast("QWidget", window), APP_NAME, DISTRIBUTION
    )
    help_menu.addSeparator()
    help_menu.addAction(window._about_action)


def add_help_actions(window: MenuWindow) -> None:
    """Create Help menu actions."""
    action_parent = cast("QObject", window)

    window._about_action = QAction("About Plain Open PDF", action_parent)
    window._about_action.triggered.connect(
        window._show_about_dialog,
    )
