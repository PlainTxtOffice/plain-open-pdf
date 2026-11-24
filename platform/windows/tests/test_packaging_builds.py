"""Check build variants, preflights, and privacy at the compiler boundary."""

import tomllib

import pytest

from packaging_support import (
    POWERSHELL,
    arguments,
    engine_build_interpreter,
    engine_signing_thumbprint,
    make_project,
    run_script,
)

pytestmark = pytest.mark.skipif(not POWERSHELL, reason="PowerShell 7 is required")


@pytest.mark.parametrize("variant", ["onefile", "standalone", "standalone-debug"])
@pytest.mark.parametrize("commercial", ["", "yes"])
def test_build_variant_flags(tmp_path, variant, commercial):
    project, shim = make_project(tmp_path)
    result = run_script(project, shim, variant + ".ps1", "-Compiler", "msvc",
                        TEST_COMMERCIAL=commercial)
    assert result.returncode == 0, result.stdout + result.stderr
    assert engine_build_interpreter(project) == str(shim)
    args = arguments(project)
    debug = variant == "standalone-debug"
    portable = variant == "onefile"
    assert ("--onefile" in args) == portable
    assert ("--standalone" in args) == (not portable)
    assert [a for a in args if a.startswith("--windows-console-mode=")] == [
        "--windows-console-mode=" + ("force" if debug else "disable")]
    assert any("portable.marker=" in a for a in args) == portable
    version = tomllib.loads((project / "pyproject.toml").read_text("utf-8"))["project"]["version"]
    filename = {
        "onefile": f"plain-open-pdf-{version}-portable.exe",
        "standalone": "plain-open-pdf.exe",
        "standalone-debug": "plain-open-pdf-debug.exe",
    }[variant]
    assert f"--output-filename={filename}" in args
    assert "--file-description=Plain Open PDF" in args
    assert "--output-dir=" + str(project / "build" / variant) in args
    assert ("--enable-plugin=data-hiding" in args) == bool(commercial)
    assert ("--enable-plugin=anti-debugger" in args) == bool(commercial and not debug)
    assert ("--enable-plugin=signtool" in args) == bool(commercial and not debug)
    # Nuitka signs only the executable, so the same builds sign the engine
    # module with the certificate the executable is signed with.
    signed_with = [
        a.split("=", 1)[1] for a in args
        if a.startswith("--windows-certificate-sha1=")
    ]
    assert [engine_signing_thumbprint(project)] == (signed_with or [""])
    assert not any("traceback-encryption" in a or "--embed-data" in a for a in args)
    assert any(a.startswith("--report=") for a in args)
    assert result.stdout.count("Building Plain Open PDF:") == 1
    assert result.stdout.count("Output directory:") == 1


@pytest.mark.parametrize("compression", ["stdlib", "package", "none"])
def test_onefile_compression_preflight(tmp_path, compression):
    project, shim = make_project(tmp_path)
    result = run_script(project, shim, "onefile.ps1", "-Compiler", "msvc",
                        TEST_COMPRESSION=compression)
    if compression == "none":
        assert result.returncode != 0
        assert "pip install zstandard" in result.stderr
        assert not (project / "build-arguments.json").exists()
    else:
        assert result.returncode == 0, result.stdout + result.stderr


def test_debug_forwards_output_overrides(tmp_path):
    project, shim = make_project(tmp_path)
    result = run_script(project, shim, "standalone-debug.ps1", "-Compiler", "msvc",
                        "-OutputDirectory", "build/custom", "-OutputFilename", "Reader.exe")
    assert result.returncode == 0, result.stdout + result.stderr
    args = arguments(project)
    assert "--output-dir=" + str(project / "build" / "custom") in args
    assert "--output-filename=Reader.exe" in args


@pytest.mark.parametrize("failure", ["TEST_PLUGIN_ERROR", "TEST_BUILD_ERROR", "TEST_DIRTY"])
def test_build_and_validation_failures_propagate(tmp_path, failure):
    project, shim = make_project(tmp_path)
    result = run_script(project, shim, "standalone.ps1", "-Compiler", "msvc",
                        **{failure: "yes"})
    assert result.returncode != 0
    if failure == "TEST_PLUGIN_ERROR":
        assert not (project / "build-arguments.json").exists()
    if failure == "TEST_DIRTY":
        assert "Runtime data or private input: .env" in result.stdout
        assert (project / "build" / "standalone" / "main.dist" / ".env").exists()


def test_privacy_scan_encodings_and_chunk_boundary(tmp_path):
    project, shim = make_project(tmp_path)
    result = run_script(project, shim, "standalone.ps1", "-Compiler", "msvc", TEST_PRIVATE="yes")
    assert result.returncode == 0, result.stdout + result.stderr
    assert result.stdout.count("Privacy scan found 'test-private-user'") == 2
    assert "notes.txt" in result.stdout
    assert "settings.json" in result.stdout
    assert "image.png" not in result.stdout


def test_persisted_data_is_preserved_by_default(tmp_path):
    project, shim = make_project(tmp_path)
    data = project / "local-appdata" / "plain-open-pdf"
    data.mkdir(parents=True)
    settings = data / "settings.json"
    settings.write_text("private preferences", encoding="utf-8")
    result = run_script(project, shim, "standalone.ps1", "-Compiler", "msvc")
    assert result.returncode == 0, result.stdout + result.stderr
    assert "Persisted data at" in result.stdout
    assert settings.read_text("utf-8") == "private preferences"


# "open-pdf", "Open PDF" and their "-data" forms are pre-rename names that a
# compiled build adopts at startup, so a reset has to remove them as well.
def test_reset_also_deletes_pre_rename_data_folders(tmp_path):
    from plain_open_pdf.config import path_handler

    project, shim = make_project(tmp_path)
    local = [
        project / "local-appdata" / name
        for name in ("plain-open-pdf", *path_handler.LEGACY_APP_DATA_DIR_NAMES)
    ]
    portable = [
        project / "build" / "onefile" / name
        for name in (
            "plain-open-pdf-data",
            *path_handler.LEGACY_PORTABLE_DATA_DIR_NAMES,
        )
    ]
    unrelated = project / "local-appdata" / "other-app"
    for folder in (*local, *portable, unrelated):
        folder.mkdir(parents=True)
        (folder / "settings.json").write_text("old", encoding="utf-8")

    result = run_script(
        project, shim, "onefile.ps1", "-Compiler", "msvc", "-ResetRuntimeDefaults"
    )

    assert result.returncode == 0, result.stdout + result.stderr
    assert [folder.name for folder in (*local, *portable) if folder.exists()] == []
    assert unrelated.is_dir()


def test_pre_rename_data_folders_are_reported_without_reset(tmp_path):
    project, shim = make_project(tmp_path)
    data = project / "local-appdata" / "open-pdf"
    data.mkdir(parents=True)
    result = run_script(project, shim, "standalone.ps1", "-Compiler", "msvc")
    assert result.returncode == 0, result.stdout + result.stderr
    assert "Persisted data at" in result.stdout
    assert data.is_dir()
