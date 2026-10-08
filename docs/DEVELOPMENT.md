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
* Calc imports fail if any sheet's values or formulas cannot be read. The error
  identifies the sheet and read stage; the existing workbook remains open.
  A formula read failure never silently substitutes its cached result.
* XLSX round-trip tests require exact formula text, including absolute/mixed/range
  references and `#REF!`. Imported numbers retain the available `f64` precision
  rather than being rounded for display before storage.
* Imported spreadsheet text that resembles a number, formula or boolean is
  protected with the literal-text input marker (`'`). XLSX exports remove the
  marker and write an actual string cell, retaining leading zeros, whitespace
  and literal apostrophes. CSV exports remove the marker too; CSV itself does
  not encode cell types, so reopening it still infers numbers/formulas.
* Imported spreadsheet error values become constant error formulas (for example,
  `=#REF!`) in the raw-input model. Their tokens and error propagation survive
  XLSX save/reload; they are exported as formulas rather than standalone error cells.
* CSV imports support quoted multiline cells, doubled quotes, LF/CRLF/CR record
  endings, ragged rows and a leading UTF-8 BOM. Embedded line endings remain part
  of the cell value. Blank records retain row positions, and whitespace-only
  values are protected as literal text. Invalid quotes report the logical record
  and field; imports beyond XLSX worksheet row/column limits fail.
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
* Calc stores numbers as `f64`; XLSX number formatting and cell styling are not
  preserved. Numeric text may be normalized on import, and display uses rounded values.
* Calc copies raw values/formulas as tab-separated text. Formatting is not
  exchanged through the clipboard. Copy/paste is limited to 1,000,000 cells
  per operation and XLSX worksheet bounds; history retains the latest 100 operations.
* Impress: shapes beyond title+body / animations not started
* Packaging: CI builds Linux binary; no signed macOS/Windows installers yet

## Suggested next PRs

1. True DOCX `sectPr` / ODT master-page section styles
2. Win/macOS packaging artifacts + accessibility tree stubs
3. Calc charting MVP / Impress richer shapes

## Calc editing

The CSV library API `load_csv_str` returns `Result<Workbook, CsvError>`, matching
`load_csv_path`, so callers can handle parse errors instead of accepting partial data.
CSV and TSV share the quoted-field parser. CSV requires quotes to enclose a whole
field; TSV keeps accepting unquoted formula text containing string literals and
retains its existing cell-count limit and rectangular padding.

Drag across cells, Shift+click, or use Shift+arrow keys to select a rectangle.
Ctrl/Cmd+C, X and V copy, cut and paste tables (including quoted fields and
Windows line endings). Blank fields overwrite destination cells. Paste starts
at the top-left corner of the selection. Delete clears the selected range.
The Edit menu provides the same clipboard actions. While the formula bar has
focus, text editing and its clipboard shortcuts apply to that single cell.

Ctrl/Cmd+Z undoes a cell edit, a whole paste/clear, or adding a sheet.
Ctrl/Cmd+Shift+Z (also Ctrl/Cmd+Y) redoes it. Saving retains history; returning
to the saved revision removes the unsaved marker. A new edit after Undo drops
the redo branch. Opening or creating a workbook resets history.
F2 or double-click focuses the formula bar; typing on the grid starts a new
cell value. Enter commits it and moves down; Escape cancels the draft.

Start cell input with an apostrophe to force literal text: `'00123` displays
`00123`, and `'=A1` displays `=A1` without evaluating it. The formula bar and
raw TSV clipboard retain the marker so internal copy/paste and Undo/Redo keep
the text interpretation. Type two initial apostrophes to display one literal
apostrophe. Imported CSV apostrophes are treated as data, not input markers.

Copying a cell range within the current Calc workbook adjusts formula references
by the displacement from the copied top-left cell to the pasted top-left cell.
For example, copying D2's `=B2*C2` to D3 produces `=B3*C3`. `$A$1` fixes both
axes, `$A1` fixes the column, and `A$1` fixes the row. Both endpoints of ranges
are adjusted. String literals, function names and structured-reference contents
are preserved. A reference (or range endpoint) leaving XLSX bounds becomes
`#REF!`, which propagates through evaluation and survives XLSX round-trips.

The native clipboard carries unchanged TSV plus an HTML marker identifying the
captured copy in this Calc session. Matching plain text alone never authorizes
formula translation. Cut, external/plain-text paste and formula-bar editing keep
formulas verbatim. New/Open clears the retained copy context. If the platform
cannot exchange HTML clipboard data, plain-text copy/paste remains available
without reference translation. Sheet-qualified references can be translated,
but evaluating cross-sheet formulas, named ranges and structured references is
still outside the current formula engine.
