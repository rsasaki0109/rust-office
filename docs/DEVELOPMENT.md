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
  Foreign title/body imports use defaults when optional `ppt/theme/theme1.xml` /
  `docProps/core.xml` are missing; present but unreadable or malformed optional
  parts also fail. The rust-office basic profile requires its theme part. A valid empty slide list
  opens one editable blank slide, and may omit the presentation relationships part.
* PPTX text import retains empty titles, paragraph breaks (including blank lines),
  escaped XML text and CDATA. Foreign PPTX coverage remains title/body text and
  a basic theme accent/background; arbitrary shapes, notes, layouts and theme
  relationship selection are not preserved. The basic profile described below
  additionally supports rust-office's own object/notes round trips. UTF-8 XML parts with Transitional or Strict main namespaces
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
* ODT file imports require complete UTF-8 `content.xml` with an
  `office:document-content` root and `office:body/office:text`. Referenced
  `draw:image` package parts are loaded by namespace-qualified `xlink:href`;
  missing parts and ZIP read/CRC failures reject the whole import with the part
  name. Relative/absolute package paths, dot segments and percent-encoded names
  are resolved within the ZIP. Unreferenced images are ignored. External image
  links and inline binary image data are unsupported and fail explicitly.
* ODT `styles.xml` is optional when absent, but a present empty, unreadable,
  invalid UTF-8 or malformed part fails the import. Body/header/footer CDATA,
  escaped links, spaces, tabs and line breaks are retained. Compact `text:s`
  expansion is limited to 1,000,000 spaces per XML part; invalid counts fail
  before repeated strings are allocated. Failed opens retain the Writer document,
  selection, caret preference, destination, dirty state and Undo/Redo history.
  XML-only parsing helpers may still omit image bytes, and failed
  `apply_styles_xml` calls leave their document unchanged. This is MVP structure
  validation, not full ODF schema/manifest validation: rich header/footer
  resources, master-page selection, per-section variants, floating images and
  images within table cells remain outside the current model.
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
* Calc stores numbers as `f64`. Direct cell number formats and bold/italic are
  preserved in XLSX; borders, fills, alignment, inherited row/column styling and
  richer fonts are not. Only General, integer/two-decimal number/percent and
  `yyyy-mm-dd` codes have formatted display; other imported codes are retained
  for export with General display. XLSX 1904-date workbooks are rejected; XLS/ODS
  imports still preserve values/formulas only. Numeric text may be normalized.
* Calc copies raw values/formulas as tab-separated text. Formatting is not
  exchanged through the clipboard. Copy/paste is limited to 1,000,000 cells
  per operation and XLSX worksheet bounds; history retains the latest 100 operations.
* Impress: arbitrary third-party PPTX object/layout interchange, searchable/vector slide PDF and animations remain unsupported
* Packaging: CI builds Linux binary; no signed macOS/Windows installers yet

## Suggested next PRs

1. True DOCX `sectPr` / ODT master-page section styles
2. Win/macOS packaging artifacts + accessibility tree stubs
3. Calc charting MVP / Impress richer shapes

## Calc editing

The Sheet menu can rename and delete worksheets. Names must be unique (case
insensitive) and satisfy XLSX naming rules; Add Sheet chooses an unused default
name even after deletions or renames. Deletion requires confirmation and the last
worksheet cannot be deleted. Rename/delete are undoable, including deleted cell
contents, formulas, sheet position and the saved revision's dirty state. Opening
these actions commits a pending formula-bar edit to its original worksheet.
Cancel/Escape leaves the sheet name and sheet list unchanged.

XLSX preserves the resulting sheet names and contents; CSV still stores only one
sheet. Cross-sheet formulas are not supported yet; rename does not rewrite their
raw text. Row heights and column widths are editable and preserved in XLSX.

The Format menu applies General, two-decimal number, percentage, ISO date,
bold/italic and Clear Formatting to the selected range as one undoable edit.
Ctrl+B/Ctrl+I toggle decoration when the grid has keyboard focus. Formatting
commits a pending formula-bar draft first. Values and formula results remain
unchanged, and clearing values retains cell presentation. Blank-cell formatting
is also saved in XLSX. Formatting is limited to 1,000,000 cells per operation.

Dates use Excel's 1900 serial convention (including its fictitious 1900-02-29).
The date format displays numeric serials; it does not parse date strings or show
times. Invalid/out-of-range serials use General display. XLSX presentation follows
worksheet/styles relationships; missing referenced styles/fonts/number formats,
malformed XML and unsupported 1904 epochs reject the entire import. Presentation
XML is limited to 32 MiB per part; this is not a global workbook memory limit.

CSV cannot store formatting: Save/Save As refuses a styled workbook and asks for
XLSX instead, preserving the existing file, destination, draft and dirty state.
Clipboard copy/paste still exchanges raw text/formulas and retains destination
presentation; it does not transfer source formatting.

Format → Column Width / Row Height applies a size to all columns/rows intersecting
the selection, as one Undo/Redo edit. Widths use integer pixels (1–1790), heights
use points (1–409.5; 1 point is 4/3 screen pixels). Reset to Default restores the
current sheet default; Cancel/Escape changes no sizes. Opening the dialog commits
a pending formula-bar draft to its original cell. Clearing cell formatting does
not reset row/column sizes.

Sizing is sparse. Drawing, hit testing and scrolling share prefix offsets for
custom sizes and only visit visible cells; there is no offset array per empty row.
XLSX retains individual/range sizes, blank rows/columns and sheet defaults. Column
conversion assumes the standard Calibri-11 seven-pixel digit metric and rounds to
physical pixels. Hidden rows/columns and sizes outside the supported limits reject
an XLSX import to preserve layout; hiding, merged cells and font-dependent autofit
remain unsupported. CSV Save/Save As also refuses custom sizes rather than silently
losing them, preserving the current file, destination and pending edits.

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


Impress slide operations support duplicate, move up/down, confirmed deletion
(with the last slide protected), and bounded 100-operation Undo/Redo. History
covers existing title/body text boxes, notes and themes; consecutive typing in
one field is grouped until focus changes, another operation or a save boundary.
Undo back to the saved revision clears the unsaved marker. Native JSON stores
slide order, text box geometry and notes, but never runtime history. The slide
history implementation does not itself expand PPTX coverage; the basic-object
PPTX profile described below provides separate interchange support. Direct model mutation followed by
`mark_dirty` clears history to avoid replaying operations against unrelated data.


Impress native objects include added text boxes, rectangles, ellipses and embedded
images. Objects paint in insertion order; the frontmost bounding box wins clicks.
Select on the canvas or in the Object selector, drag to move and use the lower-right
handle to resize. X/Y/width/height fields use percentages of the slide, constrained
to fit inside it. Text wraps and clips to its box. Added text has editable size and
color; rectangles/ellipses have editable fill. Remove object and all geometry/text
operations participate in history; one canvas drag is one Undo operation.

Native JSON preserves the object list, geometry and embedded bytes. Old JSON with
no object list still opens. Image bytes are shared across document/history clones;
active-slide textures are cached and released on deck replacement or slide changes.
Images accept PNG/JPEG/GIF/WebP/BMP, with an 8 MiB encoded limit and 4096-pixel edge
limit. Animated images use the decoded first frame. Image insertion and native
Open validate decoding and geometry before replacing the document. Saving invalid
geometry or images fails before touching the destination. This does not add global
archive/document input limits from the separate stability milestone.

The basic PPTX profile described below exports added objects and retains their
geometry on reimport. Native JSON remains the format for complete native state.
Rotation, grouping, object reordering, aspect-lock resize and animations are later
work. Foreign title/body PPTX imports retain their existing geometry limitations.


Impress slide show starts with F5 (beginning) or Shift+F5 (current slide), or the
Slide Show menu. The window becomes fullscreen and uses the same renderer as the
editor for title/body boxes, additional text, shapes and embedded images. A 16:9
slide fits centrally with black letterboxing and no editing handles or notes.
Previous/Next buttons, left/right clicks and keyboard navigation control the show;
Space/Enter/PageDown/right/down advance, PageUp/Backspace/left/up go back, Home/End
select the endpoints, and Esc/Exit returns to the editor. Endpoints do not wrap.

Show navigation is transient: it never changes document contents, active editing
slide, object selection, file destination, dirty revision or Undo/Redo history.
Text events are processed before a pending show starts, then focus is released;
editing/New/Open/Save/Undo shortcuts are inactive during the show. Closing the
window ends the show before the existing suite-wide unsaved confirmation. Cancel
keeps the edited documents and history in the normal editor.

F11 or Slide Show → Window fullscreen toggles the editor window fullscreen.
Show exit restores the fullscreen flag reported at start. External window-manager
fullscreen changes were not reported by the X11 backend in the native test;
application F11 is the tested way to manage and restore fullscreen state. Other
OS/Wayland behavior still requires the separate platform acceptance checks.
Arbitrary third-party PPTX interchange, presenter-notes views and animations remain
later work. Raster PDF sharing is implemented below. Completing slide show alone does not complete the
roadmap milestone that also requires PDF and basic-shape PPTX output.


Impress PDF export is File → Export PDF (slides as images). Every slide produces
one 960×540-point PDF page with a 1920×1080 image (144 dpi, JPEG quality 95).
The editor, slide show and PDF use a shared scene for theme backgrounds, text
wrapping/clipping, object order, rectangles, ellipses and embedded images. Raster
text uses the application's font definitions, including installed CJK fallback;
no font substitution occurs in the PDF viewer. Speaker notes, editing handles
and show controls are excluded. Text is not selectable/searchable and JPEG can
introduce slight image artifacts; vector/searchable export remains future work.

The UI worker captures the document after the export frame's text events, copies
font definitions to an isolated egui context and renders off the UI thread.
Editing can continue; the output uses the captured snapshot. Only one export may
run at a time. Export never changes the document's native destination, dirty
revision, selection or history, even if later edits occur or generation fails.
Success/failure wakes the UI for status reporting. Cancelling the file picker
starts no worker. Closing the application can cancel an unfinished background
export; destination writes remain atomic rather than exposing a partial PDF.

PDF supports 1–200 slides per export. Invalid geometry, font sizes, corrupt images
and unsupported paint callbacks are errors. Image limits from the native object
model also apply. The writer generates the whole PDF before atomically replacing
the destination; failure leaves a prior file intact. The UI requires a .pdf
extension, protecting a selected native JSON destination from accidental export.
The combined presentation/PDF/PPTX roadmap milestone remains in progress until the
slide show, PDF and basic-object PPTX PRs are merged and accepted.


Foreign PPTX imports support title/body text only. Listed slides containing
images, shapes without text bodies, grouped objects, connectors, charts/tables or
content parts are rejected with the slide part path rather than silently losing
those objects. This applies to both transitional and strict OOXML namespaces,
including renamed prefixes. Unlisted slide parts remain ignored. Text formatting
and geometry remain subject to the existing foreign title/body MVP limitations.


Impress basic PPTX export writes editable DrawingML text boxes, rectangles,
ellipses and embedded pictures in the same drawing order as the editor. Each
slide has an explicit theme background. Title/body colors and sizes, added text
colors and sizes, shape fills, geometry and plain speaker notes are written to
standard OOXML parts; the deck title is written to core properties. No JSON copy
is embedded. PNG/JPEG bytes are retained. GIF/BMP/WebP images become PNGs of the
first decoded frame; decoded pixels are retained, but original source bytes are
not. Converted PNGs must also fit the 8 MiB image limit. Fonts are not embedded,
so another viewer's font selection, wrapping and rendering can differ.

The supported import profile is identified by `p:cSld name="rust-office:basic-v1"`
and the exported object names. Reopening these exports reads actual OOXML geometry,
text styles, relationships and image parts, restoring title/body layout, object
order, theme and plain notes. Geometry is rounded to integral EMUs (one point is
12,700 EMUs), and font sizes to 0.01 pt. The profile uses 960×540 pt slides; arbitrary
slide sizes, mixed title/body themes per slide, rotation/flip, groups, rich text,
gradient/outline effects, crop, transitions and animations are rejected when
recognized as unsupported. Renaming the profile marker or object names, or editing
the file with another application, can move it outside this supported subset.
Foreign decks continue to use the separate guarded title/body reader.

Missing/corrupt/external image relationships, invalid image decoding, malformed
notes, missing basic-profile themes and invalid geometry reject the entire import
before changing the current document. XML parts have an 8 MiB expanded limit;
slide/notes trees are limited to depth 128 and 100,000 nodes per part. Image limits
remain 8 MiB encoded, 4096 pixels per edge and 64 MiB decode allocation. The package/model admission limits below additionally bound total input sizes.
Neither these checks nor their totals are an exact process-memory bound or the
complete input-limit roadmap milestone.

File → Export PPTX captures text events from the export frame before generating
the file. Generation completes before atomic destination replacement. A failure
leaves any old output intact. The UI requires a .pptx destination, protecting
native JSON files from accidental export. Export preserves the native save path,
selection, dirty revision and Undo/Redo; after opening an exported PPTX, Save still
uses Save As for JSON. PPTX export currently runs synchronously on the UI thread.

Validation includes basic-object/notes round trips across Light/Dark/Ocean themes,
Japanese text and blank paragraphs, PNG/JPEG byte preservation, GIF/BMP/WebP pixel
preservation, strict namespaces and aliases, failed exports, missing/corrupt/
oversized images and notes, unsupported edits and changed slide size. Native
Xvfb/Openbox tests exported and reopened a four-slide object deck, checked native
Save and Undo/Redo after export, and retained the document/redo after a failed
image import. Independent python-pptx checks confirmed editable shape types,
geometry, fills, text styles, images, theme backgrounds, metadata and notes.
Microsoft PowerPoint / LibreOffice rendering has not been manually verified.


PPTX package admission now limits the compressed file to 64 MiB, the ZIP directory
to 4096 entries, total declared expanded parts to 128 MiB, and every XML/rels part
to 8 MiB. Entry counts are checked in the ZIP end record before constructing the
ZIP reader; expanded sizes and duplicate part names are checked in the directory
before reading XML or decoding pictures. ZIP64 central directories are unsupported
for these bounded packages. Orphan parts also count against package limits, even
though their slide contents are not imported. Declared sizes are admission checks;
actual XML/image reads retain bounded readers and format/CRC validation.

Editable PPTX data is limited to 1000 slides, 1000 objects per slide and 10,000
objects across the deck, counting title/body boxes, plus 64 MiB of UTF-8 title/body,
added text and speaker-note content. Repeated slide references count again against
model totals. Image relationships resolving to the same package part reuse one
validated Arc-backed byte buffer across slides; URI aliases resolve before cache
lookup. This prevents repeated references from copying or decoding the same asset
on each occurrence. The cache exists only during import; model/history references
retain the shared bytes afterward.

Path imports check file metadata first and use a bounded read, including a second
size check if the file grows during reading. In-memory imports apply the same
package and directory checks. Export applies the same model/package limits and
checks XML part sizes and accumulated output parts before atomic replacement,
so refused exports leave the old destination intact. A refusal never replaces the
current document; Undo/Redo and the native save path remain usable.

These limits bound admitted input/model sizes, not exact peak RAM or processing
time. Recovery copies and equivalent aggregate limits for other formats remain
unfinished, so this does not complete the data-protection roadmap milestone.


### Writer crash recovery

Writer records a native JSON recovery copy immediately on the first dirty frame,
then at most every 15 seconds, including while Calc or Impress is active. The copy
is limited to 32 MiB and atomically replaced; a failed write keeps the previous
copy and reports an error. Serialization and disk I/O currently run on the UI
thread, so large documents or slow storage can briefly pause interaction. Changes
since the last successful copy can be lost. This covers Writer only.

Copies live under `rust-office/recovery/writer` in Linux `XDG_STATE_HOME` (or
`~/.local/state`), macOS `~/Library/Application Support`, or Windows
`LOCALAPPDATA`. Each process holds a separate session file lock. Copies belonging
to running processes are excluded from recovery candidates. At startup, stale
copies can be recovered, kept for later, or deleted with confirmation. File →
Recovery copies reopens this list. Unreadable or oversized copies remain on disk
until explicitly deleted. Recovery is unavailable if the state directory cannot
be created or locked; normal editing and saving remain available.

Recovery preserves the native document model, including embedded image bytes and
headers/footers, but does not restore runtime Undo/Redo or the original save path.
The recovered document is dirty and its first Save asks for a destination. A new
recovery copy is written before retiring the old one, so another crash remains
recoverable. External image paths still depend on the referenced files being
available. Successful Save, New, Open, or an authorized normal close removes the
current session copy. Failed operations and cancelled close dialogs retain it.
Copies postponed from previous sessions remain available.

Regression coverage checks live-session exclusion, repeated recovery, bounded
write failure, corrupt copies, native model preservation, dirty-state/history
reset, save-path isolation and unavailable storage. Calc/Impress recovery and
aggregate limits for other formats remain pending; this work adds no roadmap
points until the complete stability milestone is accepted on main.
A Linux Xvfb/Openbox native test killed the Writer process with SIGKILL, restarted
it, recovered Japanese text plus unsaved edits, verified original-file bytes were
unchanged, saved to a newly selected path, and checked copy cleanup and normal
close. Native Windows/macOS recovery has not been verified.
