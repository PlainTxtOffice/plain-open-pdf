<!-- last_updated: 2026-11-22 -->
<!-- Authoring guidance: dev-docs/specs/README_SPEC.instructions.md -->
<!-- rumdl-disable MD033 MD041 -->
<p align="center">
  <img src="assets/icons/icon-github-readme.svg" alt="Plain Open PDF logo: a white printer on a red tile, printing a page with a peeled corner" width="96">
</p>

<h1 align="center">Plain Open PDF</h1>

<p align="center">
  An open source, lightweight PDF viewer with light editing, for people who
  want to read, search, and touch up PDFs without a heavyweight suite.
</p>

<p align="center">
  <a href="https://github.com/PlainTxtOffice/plain-open-pdf/releases/latest"><img src="https://img.shields.io/github/v/release/PlainTxtOffice/plain-open-pdf?label=release" alt="Latest release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-informational" alt="License: MIT"></a>
  <a href="#download"><img src="https://img.shields.io/badge/platforms-Windows-informational" alt="Platforms: Windows"></a>
</p>

<p align="center">
  <a href="#download">Download</a> ·
  <a href="docs/USER_MANUAL.md">User manual</a> ·
  <a href="https://plaintxtoffice.com/apps/">Website</a>
</p>
<!-- rumdl-enable MD033 MD041 -->

---

## About

Plain Open PDF is a lightweight reader for PDF files on Windows.
Open a document from your file manager or the command line and start reading
right away: pages render as you reach them instead of waiting for the whole
file. Light editing is intuitive: redact a region, replace a line of text,
reorder pages, and save the result back over the original. The portable build
can be run from a flash drive and keeps its settings in a `plain-open-pdf-data`
folder next to the program.

Linux and macOS versions are planned but not released yet.

## Features

- Open several PDFs at once in tabs you can drag to reorder; launching again
  forwards the file to the running window
- Read in continuous scrolling or single-page view, and navigate with page
  jumps, thumbnails, bookmarks, and links inside the PDF
- Zoom to actual size, fit width, fit page, or with `Ctrl` + mouse wheel
- Search the whole document as you type and copy selectable text
- Redact a page region or replace the text it covers
- Insert blank pages, reorder, rotate, or delete pages, and edit bookmarks
- Edit document metadata and keep plain-text notes stored inside the PDF
- Run optional Tesseract OCR on one page or the whole document
- Save in place, save a copy, rename the file, or print all or part of it
- Remember passwords for encrypted PDFs using Windows protection

## App Screenshot

![Plain Open PDF main window on Windows, with page thumbnails on the left, the continuous document view in the center with document notes below it, and bookmarks on the right](platform/windows/assets/screenshots/plain-open-pdf-main-window.png)

## Download

| Platform | Download | Notes |
| --- | --- | --- |
| Windows (portable) | [plain-open-pdf.exe](https://github.com/PlainTxtOffice/plain-open-pdf/releases/latest/download/plain-open-pdf.exe) | Runs without installation. |
| Windows (installer) | [plain-open-pdf-setup.exe](https://github.com/PlainTxtOffice/plain-open-pdf/releases/latest/download/plain-open-pdf-setup.exe) | |

The Windows builds are for x64 (64-bit). Checksums and release notes are on the
[latest release](https://github.com/PlainTxtOffice/plain-open-pdf/releases/latest).

The executables are unsigned. To check a download, compare the output of
`Get-FileHash -Algorithm SHA256 PATH_TO_DOWNLOAD` with
[SHA256SUMS.txt](https://github.com/PlainTxtOffice/plain-open-pdf/releases/latest/download/SHA256SUMS.txt).
No Linux or macOS download has been released.

## User Manual

See the [User Manual](docs/USER_MANUAL.md) for installation, setup, usage, and
troubleshooting.

## Feedback

Send bug reports and feature requests to
[support@plaintxtoffice.com](mailto:support@plaintxtoffice.com), through the
[contact page](https://plaintxtoffice.com/contact/), or in
[GitHub Issues](https://github.com/PlainTxtOffice/plain-open-pdf/issues). In the
app, `Help > Send Feedback...` drafts the email in your email app with your
app version and Windows version filled in, or opens the contact page. If Plain
Open PDF showed its crash dialog, include the log file it names.

## License

Plain Open PDF's original code is licensed under the [MIT License](LICENSE).

The Windows app reads, renders, and edits PDFs with our own MIT-licensed PDF
engine and does not use PyMuPDF or MuPDF. Linux and macOS versions are planned;
if one is released before it moves to the engine, it will use PyMuPDF, and the
dependency license terms below will apply to it.

Third-party dependencies retain their own licenses. In particular,
[PyMuPDF and MuPDF](https://pymupdf.readthedocs.io/en/latest/about.html#license-and-copyright)
are offered under AGPL or a commercial license. This repository's MIT license
does not replace those terms; distributing a build with these dependencies
requires compliance with their applicable licenses.
