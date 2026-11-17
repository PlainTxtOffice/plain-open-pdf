# Plain Open PDF for Windows

Developer setup for the Windows desktop viewer. You need Windows, Python 3.12
or newer, PowerShell 7.3 or newer, and the stable Rust toolchain from
<https://rustup.rs>, which builds the app's PDF engine.

Run these commands from `platform/windows/`:

```powershell
python -m venv .venv
.venv/Scripts/python.exe -m pip install -e ".[dev]"
./scripts/engine/build.ps1
.venv/Scripts/python.exe -m plain_open_pdf.main
.venv/Scripts/python.exe -m ruff check .
.venv/Scripts/python.exe -m pytest
.venv/Scripts/python.exe -m mypy plain_open_pdf
.venv/Scripts/rumdl.exe check
```

To open a PDF at startup, pass its path:

```powershell
.venv/Scripts/python.exe -m plain_open_pdf.main "C:/Users/YourName/Documents/report.pdf"
```

Application modules live in `plain_open_pdf/` and use imports such as
`plain_open_pdf.config.path_handler`. The window icon and bundled fonts live in
`assets/`; README screenshots live in `assets/screenshots/` and are excluded
from builds. Development data and logs are relative to this Windows project
folder. Add application tests under `tests/`.

The app reads, renders, and edits PDFs with the MIT-licensed engine in
`plain_open_pdf_engine/`. Rebuild it with `scripts/engine/build.ps1` after
changing it; see the [engine build guide](scripts/engine/README.md). Build
scripts live in `scripts/nuitka/`; see the
[Nuitka build guide](scripts/nuitka/README.md).

See the [repository README](../../README.md) for project-wide documentation.

## Architecture

- `plain_open_pdf/domains/document/` owns engine-independent rules and values
  for page indexes, navigation, notes, text matching, and metadata
  normalization.
- `plain_open_pdf/application/document/` coordinates opening, editing, saving,
  notes, OCR, and rename workflows. `session.PdfDocument` remains the document
  facade used by the GUI; it coordinates state updates after successful
  operations.
- `plain_open_pdf/adapters/` owns PDF engine operations, filesystem access, OCR
  engines, persistence, and Windows integration. The PDF adapter in
  `adapters/document/engine/` is split into lifecycle, rendering, links, and
  content operations, and converts between the engine's coordinates and the
  app's page space.
- `plain_open_pdf/gui/` owns Qt presentation, selection, and dialogs. Text
  editing presentation lives in `views/workflows/text_editing.py`, and page
  selection lives in `views/rendering/selection.py`.

The GUI calls application workflows; workflows use domain rules and adapters.
Domain modules never import outer layers, and adapters never import application
workflows or GUI modules. Search rectangles cross boundaries as four-coordinate
tuples in PDF units, not engine objects.

Startup explicitly migrates legacy storage folders and selects portable or
installed storage before importing logging and GUI consumers. A legacy folder
that cannot be renamed, for example while an earlier build is still running,
stays in use under its old name until a later start can rename it. Importing
`config/path_handler.py` alone does not create or move storage folders.

Run `tests/test_architecture.py` and `tests/test_gui_imports.py` to check these
boundaries, alongside the behavioral regression tests.

Pytest isolates temporary files and its cache by project and process account
through `conftest.py`. Run it without a fixed `--basetemp` so Windows user
and sandbox accounts do not share permission-sensitive test directories.
