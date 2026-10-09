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
* PPTX imports follow `ppt/presentation.xml`'s slide list, resolving relationship
  IDs by their namespace rather than numeric ID or ZIP filename order. Unlisted
  slide files are ignored. Listed slides require internal slide relationships;
  absolute package targets, relative targets, dot segments and percent-encoded
  names are resolved relative to the presentation part.
* A missing presentation, relationship or listed slide, unreadable part, or malformed
  XML rejects the entire PPTX import and identifies the affected part. The existing
  deck, active slide, destination and unsaved edits remain intact on failure.
  Missing optional `ppt/theme/theme1.xml` / `docProps/core.xml` use defaults; present
  but unreadable or malformed optional parts also fail. A valid empty slide list
  opens one editable blank slide, and may omit the presentation relationships part.
* PPTX text import retains empty titles, paragraph breaks (including blank lines),
  escaped XML text and CDATA. Coverage remains title/body text and a basic theme
  accent/background; rich shapes, notes, layouts and theme relationship selection
  are not preserved. UTF-8 XML parts with Transitional or Strict main namespaces
  are supported; arbitrary presentation-part locations are not yet supported.
* DOCX file imports validate complete `word/document.xml`, its relationships and
  referenced DrawingML images / header / footer parts. Missing, unreadable, invalid
  UTF-8 or malformed referenced parts reject the whole import with the part name.
  Relationships may be absent in documents without package references; a present
  but empty/broken relationships part is an error. Unused relationships do not load
  or replace document resources. Namespaced IDs, escaped attribute values and
  absolute/relative/percent-encoded package targets are supported. External images
  and header/footer parts fail explicitly rather than being silently omitted.
* Failed DOCX opens retain the current document, selection, Undo/Redo history,
  destination and unsaved edits. XML-only `parse_document_xml` / `parse_document_parts`
  callers may still omit package resources; the file loader resolves references
  before assigning a document. Body/header/footer text supports CDATA and escaped
  XML. UTF-8 Transitional/Strict main namespaces are accepted.
  DOCX remains an MVP reader, not a full OOXML schema/fidelity validator: one global
  plain-text header/footer is retained (first default reference, otherwise first
  reference). Rich header/footer contents, per-section variants, arbitrary main-part
  locations and richer image placement remain outside the current model.
* New/Open ask before replacing edited documents in all three apps. Quit/window close
  checks every edited document, including inactive apps. Cancelling retains edits,
  even after choosing Don't Save for another document in the same close attempt.

## Recently added

* Writer Find and Replace (Edit menu, Ctrl/Cmd+F or Ctrl/Cmd+H) searches literal,
  case-sensitive text in body paragraphs across all sections, including matches
  spanning character runs. Next/Previous wrap and scroll to the selected match.
  Tables and header/footer text are excluded and the dialog states this scope;
  queries/replacements cannot span paragraph breaks. Unicode scalar offsets match
  the editor's selections; Unicode normalization and regular expressions are not
  supported. Find never creates history or marks the document dirty. Replace
  requires a currently selected exact match; otherwise it selects the next match.
  An empty replacement deletes text. Replace All uses non-overlapping matches
  from the original text and is one Undo/Redo operation. Replacements inherit the
  first matched character's style and hyperlink; surrounding runs and paragraph
  properties remain intact. Identical replacement and no-match actions do not
  change the document or history. Search-field typing stays within the dialog;
  Escape/Close returns keyboard focus to the selected body text.

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

Aggregate functions (`SUM`, `AVERAGE`/`AVG`, `MIN`, `MAX`, `COUNT`) visit only
stored cells in ranges, in row/column order. Referenced empty cells and text
(including numeric-looking literal text and formula text results) are ignored.
`AVERAGE` counts only numbers and returns `#DIV/0!` if there are none; `MIN`/`MAX`
return zero when no numbers are present. Direct numeric string arguments are
converted to numbers. Other direct strings cause `#VALUE!`, except in `COUNT`,
which ignores them. `COUNT` ignores referenced spreadsheet errors; the other
aggregates propagate them. Circular dependencies still produce `#CYCLE!`.

Each cell evaluation has a fresh cache for numeric/error/empty dependency results,
so edits and Undo/Redo cannot reuse stale calculations. Large empty ranges do not
expand into cell-address arrays. To bound work and stack use, parsing allows at
most 32,768 source bytes, 512 parser nodes and 64 nested parser levels. Evaluation
allows 64 combined cell/expression levels and 100,000 work units (formula source
bytes, referenced literal bytes, cell/expression visits and stored-cell scans).
These are implementation limits, not full Excel formula compatibility. Exceeding
a limit returns `#NUM!` without modifying raw formulas or saved files. References
beyond XLSX worksheet bounds return `#REF!`. `IF` still evaluates only its chosen
branch, while the entire formula is parsed and subject to the parser limits.

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
