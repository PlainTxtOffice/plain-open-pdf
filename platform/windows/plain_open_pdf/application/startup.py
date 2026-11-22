"""Prepare runtime storage before loading desktop consumers."""

from typing import TYPE_CHECKING

from plain_open_pdf.adapters.runtime.storage import (
    adopt_legacy_folder,
    is_writable_dir,
)
from plain_open_pdf.config import path_handler as paths

if TYPE_CHECKING:
    from pathlib import Path


def prepare_storage() -> None:
    """Migrate existing folders, then select the writable storage root.

    - Call before importing logging or GUI modules that capture path
      constants. Development never migrates user folders.
    - Only the folder this run stores its data in is migrated, so a portable
      build leaves an installed build's per-user folder alone unless it
      falls back to it.
    - A pre-rename folder that cannot be renamed stays the storage root for
      this run; `pre_rename_storage_root` reports that.
    """
    portable_root = None
    if paths.IS_COMPILED and paths.IS_PORTABLE:
        portable_root = adopt_legacy_folder(
            paths.EXE_DIR / paths.PORTABLE_DATA_DIR_NAME,
            paths.LEGACY_PORTABLE_DATA_DIR_NAMES,
        )
    mode, root = paths.resolve_storage(
        is_compiled=paths.IS_COMPILED,
        is_portable=paths.IS_PORTABLE,
        exe_dir=paths.EXE_DIR,
        user_data_dir=paths.USER_DATA_DIR,
        is_writable=is_writable_dir,
        portable_root=portable_root,
    )
    if mode in ("installed", "portable-fallback"):
        root = adopt_legacy_folder(root, paths.LEGACY_APP_DATA_DIR_NAMES)
    paths.configure_storage(mode, root)


def pre_rename_storage_root() -> Path | None:
    """Return the storage root when it still has a pre-rename name."""
    if paths.STORAGE_MODE == "development":
        return None
    legacy_names = (
        *paths.LEGACY_APP_DATA_DIR_NAMES,
        *paths.LEGACY_PORTABLE_DATA_DIR_NAMES,
    )
    return paths.ROOT_DIR if paths.ROOT_DIR.name in legacy_names else None
