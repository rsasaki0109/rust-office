# Development notes (v0.1+)

## Verified locally

* `cargo build`
* `cargo test --workspace`
* `cargo test -p office-format --test golden`
* `cargo clippy --workspace --all-targets`
* GUI launch via `cargo run -p office-ui`

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

* No section breaks / mid-document page style changes yet
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
