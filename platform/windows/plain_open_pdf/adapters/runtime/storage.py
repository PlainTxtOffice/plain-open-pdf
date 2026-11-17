"""Probe writable storage and migrate legacy folders at startup."""

from __future__ import annotations

import tempfile
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from collections.abc import Sequence
    from pathlib import Path


def is_writable_dir(path: Path) -> bool:
    """Return whether the app can create files in the folder.

    - Creates the folder when missing, then writes a temporary probe file,
      because os.access does not reflect Windows folder permissions.
    """
    try:
        path.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryFile(dir=path):
            pass
    except OSError:
        return False
    return True


def migrate_legacy_folder(legacy: Path, current: Path) -> bool:
    """Rename a data folder left under an earlier name to the current name.

    - Moves only when the legacy folder exists and the current one is missing
      or empty, so it never merges or overwrites data.
    - Leaves the legacy folder in place when the rename fails.
    """
    if not legacy.is_dir():
        return False
    try:
        if current.exists():
            # rmdir succeeds only on an empty folder, such as one left by a
            # writability probe.
            current.rmdir()
        legacy.rename(current)
    except OSError:
        return False
    return True


def adopt_legacy_folder(current: Path, legacy_names: Sequence[str]) -> Path:
    """Return the folder that holds the app's data for this run.

    - `legacy_names` are earlier names of `current` in the same parent
      folder, newest first.
    - Returns `current` when it already holds data, when no legacy folder
      exists, or after renaming the newest legacy folder to it.
    - Returns the newest legacy folder itself when it cannot be renamed, for
      example while an earlier build still holds a file in it. The app then
      keeps using the data under its old name and a later start retries, so
      a failed rename never strands the data or adopts an older folder.
    """
    for name in legacy_names:
        legacy = current.with_name(name)
        if not legacy.is_dir():
            continue
        if migrate_legacy_folder(legacy, current):
            return current
        try:
            current_in_use = any(current.iterdir())
        except OSError:
            current_in_use = False
        return current if current_in_use else legacy
    return current
