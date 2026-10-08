# Development notes (v0.1+)

## Development checks

The repository pins Rust 1.99.0 in `rust-toolchain.toml`; CI uses the same version.
The locked egui/eframe dependencies require Rust 1.95 or newer.

Run these checks before submitting changes:

* `cargo build --locked --workspace`
* `cargo test --locked --workspace`
* `cargo test --locked -p office-format --test golden`
* `cargo clippy --locked --workspace --all-targets -- -D warnings`
* GUI launch via `cargo run --locked -p office-ui`

## Save and close behavior

* Saves and exports write a temporary file in the destination directory and replace
  the destination only after writing succeeds. Failed writes preserve the original.
* A successful save updates the destination and clears the unsaved marker. A failed
  or cancelled save keeps the document, destination, and unsaved edits.
* Calc saves include text still being edited in the formula bar. Imported XLS, XLSM,
  and ODS files require Save As to an explicit XLSX/CSV destination.
  Multi-sheet workbooks must use XLSX so CSV cannot silently discard other sheets.
* After importing a PPTX, Impress Save opens Save As for a native JSON file, preserving
  the source PPTX. Export PPTX remains a separate operation with MVP format coverage.
* New/Open ask before replacing edited documents in all three apps. Quit/window close
  checks every edited document, including inactive apps. Cancelling retains edits,
  even after choosing Don't Save for another document in the same close attempt.

## Recently added

* Visual-line ↑/↓ caret navigation (preferred column remembered)
* Home/End move within the visual line
* Unsaved-changes modal on New / Open / Quit (and window close)
* Multi-page A4 layout — overflow creates additional pages; status bar shows `Page N of M`
* Minimal ODT round-trip (`.odt`) via `office-format::OdtFormat`
* Tables, embedded images, editable cells, lists, hyperlinks
* Stronger IME preedit (active-range underline + composition caret)
* Headless layout smoke tests in `office-render`
* Explicit page breaks (`Block::PageBreak`, Insert → Page Break / Ctrl+Enter)
* Document header / footer (margin paint; `{page}` / `{pages}` fields; JSON + ODT write)
* Nested lists (Tab / Shift+Tab indent; ODT nested `<text:list>`; level markers)
* Ctrl+click opens hyperlinks in the system browser
* Golden fixtures for JSON + ODT (`crates/office-format/tests/fixtures`; refresh with `UPDATE_GOLDEN=1`)
* ODT `styles.xml` header/footer read (round-trip with write)
* In-margin header/footer editing (`EditFocus::Header` / `Footer`; click the margin band)
* Minimal DOCX (`.docx`) open/save — paragraphs, B/I/U, alignment, lists, tables, page breaks, hyperlinks
* PDF export (File → Export PDF…) — multi-page A4, header/footer fields, page breaks
* DOCX images (`word/media` + DrawingML) and header/footer parts (`header1.xml` / `footer1.xml`)
* Calc scaffold — sparse grid, A1 refs, `SUM`/`AVERAGE`/`MIN`/`MAX`/`+−*/`, CSV open/save, UI mode switch
* Impress scaffold — slides, themes (Light/Dark/Ocean), JSON + PPTX export, UI mode switch
* Writer Print… (Ctrl+P) — PDF spool via `lp`/`lpr` or preview fallback; GitHub Actions CI + Linux release artifact
* Calc XLSX open/save (multi-sheet) + `IF` / `COUNT`
* Impress PPTX import (title/body text + theme accent) alongside export
* Layout-faithful PDF — Export PDF / Print use `office-render` page geometry (same pagination as canvas)
* Named paragraph styles — Normal / Heading 1–3 (toolbar + Format menu; JSON round-trip; bakes run metrics)
* DOCX `w:pStyle` + ODT `Heading1`–`3` named-style round-trip (LO `Heading_20_N` on read)
* Layout PDF embeds real images (PNG/JPEG via printpdf; placeholder fallback on decode failure)
* Section breaks — Insert → Section Break; multi-section JSON; layout/PDF across sections (DOCX/ODT flatten to page breaks)

## ODT / DOCX coverage (MVP)

Supported (ODT + DOCX unless noted):

* Paragraphs
* Character runs: bold / italic / underline / font size / hyperlinks
* Paragraph alignment
* Lists (bullet / numbered; ODT nested; DOCX via indent + marker prefix)
* Tables
* Explicit page breaks
* Embedded images — ODT `Pictures/`; DOCX `word/media/` + inline DrawingML
* Header / footer — ODT `styles.xml`; DOCX `word/header1.xml` / `footer1.xml` (plain text)
* ZIP packages with required package parts

Not yet:

* Named styles / numbering.xml fidelity
* Floating/anchored images, wrap, alt-text fidelity beyond basics
* Styled runs inside header/footer on write (plain paragraph only)

## Known limitations

* Section breaks work in the native model/layout; DOCX/ODT still flatten per-section page styles
* Header / footer are single-paragraph (Enter inserts a line break, not a new para)
* Bold rendering is still approximated in egui
* Per-section page geometry in DOCX/ODT (export flattens section breaks to page breaks)
* Named styles beyond Normal/Heading 1–3; custom user style registry
* Layout PDF glyph metrics still approximate (egui ≠ embedded PDF fonts)
* Calc: charting / pivot not started; formula set still limited vs Excel
* Impress: shapes beyond title+body / animations not started
* Packaging: CI builds Linux binary; no signed macOS/Windows installers yet

## Suggested next PRs

1. True DOCX `sectPr` / ODT master-page section styles
2. Win/macOS packaging artifacts + accessibility tree stubs
3. Calc charting MVP / Impress richer shapes
