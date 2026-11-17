# PDF Engine Build

`build.ps1` builds the Python module of the PDF engine in
[`plain_open_pdf_engine/`](../../plain_open_pdf_engine/README.md) and installs
it into an interpreter's site-packages as `plain_open_pdf_engine.pyd`. The app,
its tests, and the Nuitka builds import it from there; pip does not install it.

Use PowerShell 7.3 or newer and the stable Rust toolchain from
<https://rustup.rs>. Run it after creating the virtual environment and again
after changing the engine:

```powershell
./scripts/engine/build.ps1
```

- `-PythonExecutable <path>` selects the interpreter; it defaults to
  `.venv/Scripts/python.exe`. Relative paths resolve from `platform/windows/`.
- `-Offline` builds from crates Cargo has already downloaded.

The module is built against the selected interpreter through `PYO3_PYTHON`.
After switching between Python 3.14 or earlier and 3.15 or later at the same
path, run `cargo clean` in `plain_open_pdf_engine/python/` first.

`scripts/nuitka/standalone.ps1` and `onefile.ps1` run this script before they
compile, so release builds always bundle the current engine.
