"""Exercise the release validator through its public PowerShell entrypoint."""

import sys

import pytest

from packaging_support import POWERSHELL, make_project, run_script

pytestmark = pytest.mark.skipif(not POWERSHELL, reason="PowerShell 7 is required")


def clean_dist(project):
    dist = project / "build" / "standalone" / "main.dist"
    dist.mkdir(parents=True)
    (dist / "plain-open-pdf.exe").write_bytes(b"MZ")
    return dist


def test_clean_dist_passes_validation(tmp_path):
    project, _ = make_project(tmp_path)
    clean_dist(project)
    result = run_script(project, sys.executable, "validate_release.ps1")
    assert result.returncode == 0, result.stdout + result.stderr
    assert "Clean distribution" in result.stdout


def test_validation_reports_runtime_data_and_private_inputs(tmp_path):
    project, _ = make_project(tmp_path)
    dist = clean_dist(project)
    names = ["data", "logs", "plain-open-pdf-data", "tests", "plain_open_pdf/tests",
             "portable.marker", ".env", "plain-open-pdf.exe.key"]
    for name in names:
        path = dist / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.touch()
    before = {p.relative_to(dist): p.read_bytes() for p in dist.rglob("*") if p.is_file()}
    result = run_script(project, sys.executable, "validate_release.ps1")
    assert result.returncode == 1
    for name in names:
        assert name in result.stdout.replace("\\", "/")
    after = {p.relative_to(dist): p.read_bytes() for p in dist.rglob("*") if p.is_file()}
    assert before == after


def test_validation_allows_dependency_data_and_tests(tmp_path):
    project, _ = make_project(tmp_path)
    dist = clean_dist(project)
    (dist / "PySide6" / "data").mkdir(parents=True)
    (dist / "PySide6" / "tests").mkdir()
    result = run_script(project, sys.executable, "validate_release.ps1")
    assert result.returncode == 0, result.stdout + result.stderr


def test_validation_requires_an_executable(tmp_path):
    project, _ = make_project(tmp_path)
    dist = clean_dist(project)
    (dist / "plain-open-pdf.exe").unlink()
    result = run_script(project, sys.executable, "validate_release.ps1")
    assert result.returncode == 1
    assert "Missing or empty release executable: plain-open-pdf.exe" in result.stdout


# Pre-rename executable names: the product was "Open PDF" before it became
# Plain Open PDF, and a payload must not ship an executable left from then.
@pytest.mark.parametrize("stale", ["Open PDF.exe", "open-pdf.exe"])
def test_validation_rejects_old_executable(tmp_path, stale):
    project, _ = make_project(tmp_path)
    dist = clean_dist(project)
    (dist / stale).write_bytes(b"MZ")
    result = run_script(project, sys.executable, "validate_release.ps1")
    assert result.returncode == 1
    assert f"Stale {stale} remains" in result.stdout
    assert (dist / stale).exists()


def test_validation_reports_missing_distribution(tmp_path):
    project, _ = make_project(tmp_path)
    result = run_script(project, sys.executable, "validate_release.ps1")
    assert result.returncode == 1
    assert "Distribution folder does not exist" in result.stdout
