"""Protect startup ordering and migration before portable-folder probing."""

import runpy
from pathlib import Path
from types import SimpleNamespace

import pytest

from plain_open_pdf.application import startup
from plain_open_pdf.config import path_handler as paths


@pytest.fixture
def storage_paths(tmp_path, monkeypatch):
    for name in ("STORAGE_MODE", "ROOT_DIR", "LOGS_DIR", "DATA_DIR"):
        monkeypatch.setattr(paths, name, getattr(paths, name))
    monkeypatch.setattr(paths, "EXE_DIR", tmp_path / "exe")
    monkeypatch.setattr(paths, "USER_DATA_DIR", tmp_path / "local" / "plain-open-pdf")
    monkeypatch.setattr(paths, "IS_COMPILED", True)
    monkeypatch.setattr(paths, "IS_PORTABLE", True)
    return paths


# The legacy names are pre-rename names: "open-pdf" and "open-pdf-data" from
# when the product was Open PDF, and the spaced names from its first builds.
@pytest.mark.parametrize("legacy_name", ["open-pdf-data", "Open PDF Data"])
def test_migration_precedes_creation_of_portable_folder(
    storage_paths, legacy_name
):
    assert legacy_name in paths.LEGACY_PORTABLE_DATA_DIR_NAMES
    legacy = paths.EXE_DIR / legacy_name
    legacy.mkdir(parents=True)
    (legacy / "settings.txt").write_text("keep", encoding="utf-8")

    startup.prepare_storage()

    assert not legacy.exists()
    assert paths.ROOT_DIR == paths.EXE_DIR / paths.PORTABLE_DATA_DIR_NAME
    assert (paths.ROOT_DIR / "settings.txt").read_text(encoding="utf-8") == "keep"
    assert paths.DATA_DIR == paths.ROOT_DIR / "data"
    assert paths.LOGS_DIR == paths.ROOT_DIR / "logs"


@pytest.mark.parametrize("legacy_name", ["open-pdf", "Open PDF"])
def test_portable_fallback_uses_migrated_user_storage(
    storage_paths, monkeypatch, legacy_name
):
    assert legacy_name in paths.LEGACY_APP_DATA_DIR_NAMES
    legacy = paths.USER_DATA_DIR.with_name(legacy_name)
    legacy.mkdir(parents=True)
    (legacy / "settings.txt").write_text("keep", encoding="utf-8")
    monkeypatch.setattr(startup, "is_writable_dir", lambda path: False)

    startup.prepare_storage()

    assert paths.STORAGE_MODE == "portable-fallback"
    assert paths.ROOT_DIR == paths.USER_DATA_DIR
    assert (paths.DATA_DIR.parent / "settings.txt").read_text(encoding="utf-8") == "keep"


def test_installed_build_migrates_the_pre_rename_folder(storage_paths, monkeypatch):
    monkeypatch.setattr(paths, "IS_PORTABLE", False)
    legacy = paths.USER_DATA_DIR.with_name("open-pdf")
    (legacy / "data").mkdir(parents=True)
    (legacy / "data" / "ui-state.json").write_text("{}", encoding="utf-8")

    startup.prepare_storage()

    assert not legacy.exists()
    assert paths.STORAGE_MODE == "installed"
    assert paths.ROOT_DIR == paths.USER_DATA_DIR
    assert paths.USER_DATA_DIR.name == "plain-open-pdf"
    assert (paths.DATA_DIR / "ui-state.json").read_text(encoding="utf-8") == "{}"


def test_newest_pre_rename_folder_wins_and_older_one_is_kept(
    storage_paths, monkeypatch
):
    monkeypatch.setattr(paths, "IS_PORTABLE", False)
    newest = paths.USER_DATA_DIR.with_name("open-pdf")
    oldest = paths.USER_DATA_DIR.with_name("Open PDF")
    for folder, text in ((newest, "newest"), (oldest, "oldest")):
        folder.mkdir(parents=True)
        (folder / "settings.txt").write_text(text, encoding="utf-8")

    startup.prepare_storage()

    assert (paths.ROOT_DIR / "settings.txt").read_text(encoding="utf-8") == "newest"
    assert not newest.exists()
    assert (oldest / "settings.txt").read_text(encoding="utf-8") == "oldest"


def test_pre_rename_folder_never_overwrites_current_data(
    storage_paths, monkeypatch
):
    monkeypatch.setattr(paths, "IS_PORTABLE", False)
    legacy = paths.USER_DATA_DIR.with_name("open-pdf")
    for folder, text in ((legacy, "old"), (paths.USER_DATA_DIR, "current")):
        folder.mkdir(parents=True)
        (folder / "settings.txt").write_text(text, encoding="utf-8")

    startup.prepare_storage()

    assert (paths.ROOT_DIR / "settings.txt").read_text(encoding="utf-8") == "current"
    assert (legacy / "settings.txt").read_text(encoding="utf-8") == "old"


def test_development_does_not_touch_user_storage(storage_paths, monkeypatch):
    monkeypatch.setattr(paths, "IS_COMPILED", False)

    def unexpected(*args):
        raise AssertionError("Development must not migrate or probe user storage")

    monkeypatch.setattr(startup, "adopt_legacy_folder", unexpected)
    monkeypatch.setattr(startup, "is_writable_dir", unexpected)
    startup.prepare_storage()
    assert paths.STORAGE_MODE == "development"
    assert paths.ROOT_DIR == paths.DEV_ROOT
    assert startup.pre_rename_storage_root() is None


def _refuse_rename_of(monkeypatch, *locked: Path) -> None:
    """Make renaming the given folders fail as Windows does for one in use."""
    real_rename = Path.rename

    def rename(self: Path, target: Path) -> Path:
        if self in locked:
            raise PermissionError("in use")
        return real_rename(self, target)

    monkeypatch.setattr(Path, "rename", rename)


def test_failed_rename_keeps_using_the_pre_rename_folder_and_retries(
    storage_paths, monkeypatch
):
    monkeypatch.setattr(paths, "IS_PORTABLE", False)
    legacy = paths.USER_DATA_DIR.with_name("open-pdf")
    (legacy / "data").mkdir(parents=True)
    (legacy / "data" / "document-passwords.json").write_text("{}", encoding="utf-8")

    with monkeypatch.context() as locked:
        _refuse_rename_of(locked, legacy)
        startup.prepare_storage()

        assert paths.STORAGE_MODE == "installed"
        assert paths.ROOT_DIR == legacy
        assert paths.DATA_DIR == legacy / "data"
        assert startup.pre_rename_storage_root() == legacy
        assert not paths.USER_DATA_DIR.exists()

    # The run above wrote its logs into the folder it kept using.
    (legacy / "logs").mkdir()
    startup.prepare_storage()

    assert not legacy.exists()
    assert paths.ROOT_DIR == paths.USER_DATA_DIR
    assert startup.pre_rename_storage_root() is None
    assert (paths.DATA_DIR / "document-passwords.json").is_file()
    assert paths.LOGS_DIR.is_dir()


def test_failed_rename_of_newest_folder_does_not_adopt_an_older_one(
    storage_paths, monkeypatch
):
    monkeypatch.setattr(paths, "IS_PORTABLE", False)
    newest = paths.USER_DATA_DIR.with_name("open-pdf")
    oldest = paths.USER_DATA_DIR.with_name("Open PDF")
    for folder, text in ((newest, "newest"), (oldest, "oldest")):
        folder.mkdir(parents=True)
        (folder / "settings.txt").write_text(text, encoding="utf-8")
    _refuse_rename_of(monkeypatch, newest)

    startup.prepare_storage()

    assert paths.ROOT_DIR == newest
    assert not paths.USER_DATA_DIR.exists()
    assert (oldest / "settings.txt").read_text(encoding="utf-8") == "oldest"


def test_failed_rename_keeps_using_the_pre_rename_portable_folder(
    storage_paths, monkeypatch
):
    legacy = paths.EXE_DIR / "open-pdf-data"
    stale = paths.EXE_DIR / "Open PDF Data"
    for folder, text in ((legacy, "newest"), (stale, "oldest")):
        folder.mkdir(parents=True)
        (folder / "settings.txt").write_text(text, encoding="utf-8")
    _refuse_rename_of(monkeypatch, legacy)

    startup.prepare_storage()

    assert paths.STORAGE_MODE == "portable"
    assert paths.ROOT_DIR == legacy
    assert not (paths.EXE_DIR / paths.PORTABLE_DATA_DIR_NAME).exists()
    assert (stale / "settings.txt").read_text(encoding="utf-8") == "oldest"


def test_empty_current_folder_does_not_block_the_migration(
    storage_paths, monkeypatch
):
    monkeypatch.setattr(paths, "IS_PORTABLE", False)
    legacy = paths.USER_DATA_DIR.with_name("open-pdf")
    legacy.mkdir(parents=True)
    (legacy / "settings.txt").write_text("keep", encoding="utf-8")
    paths.USER_DATA_DIR.mkdir()

    startup.prepare_storage()

    assert not legacy.exists()
    assert (paths.ROOT_DIR / "settings.txt").read_text(encoding="utf-8") == "keep"


def test_portable_build_leaves_the_installed_user_folder_alone(storage_paths):
    installed = paths.USER_DATA_DIR.with_name("open-pdf")
    installed.mkdir(parents=True)
    (installed / "settings.txt").write_text("installed", encoding="utf-8")

    startup.prepare_storage()

    assert paths.STORAGE_MODE == "portable"
    assert paths.ROOT_DIR == paths.EXE_DIR / paths.PORTABLE_DATA_DIR_NAME
    assert (installed / "settings.txt").read_text(encoding="utf-8") == "installed"
    assert not paths.USER_DATA_DIR.exists()


def test_importing_compiled_paths_does_not_write(tmp_path, monkeypatch):
    def unexpected(*args, **kwargs):
        raise AssertionError("Path import must not create or move folders")

    monkeypatch.setattr(Path, "mkdir", unexpected)
    monkeypatch.setattr(Path, "rename", unexpected)
    module = runpy.run_path(
        paths.__file__,
        init_globals={"__compiled__": SimpleNamespace(containing_dir=str(tmp_path))},
    )
    assert module["IS_COMPILED"] is True
