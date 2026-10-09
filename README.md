# rust-office

<p align="center">
  <img src="docs/screenshots/rust-office-hero.png" alt="rust-office — native Writer, Calc, and Impress in Rust" width="100%">
</p>

<p align="center">
  <strong>A native office suite written in Rust</strong><br>
  Writer · Calc · Impress — one binary, one shared core
</p>

<p align="center">
  <a href="https://github.com/rsasaki0109/rust-office/actions/workflows/ci.yml"><img src="https://github.com/rsasaki0109/rust-office/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <img src="https://img.shields.io/badge/rust-1.99%2B-orange" alt="Rust 1.99+">
  <img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue" alt="License">
</p>

## Overview

**rust-office** brings document editing, spreadsheets and presentations together
in a native egui/eframe application. The long-term goal is an open-source
alternative to LibreOffice and Microsoft Office; current format support covers a
documented MVP subset rather than full compatibility.

**Current acceptance score: 85% (17 of 20 daily-use milestones).** The
[daily-use roadmap](docs/ROADMAP_90.md) tracks merged features and verified
behavior, not full Office compatibility. Linux is the validated platform;
Windows/macOS runtime, Japanese IME and accessibility acceptance remain pending.

## Quick start

Install Rust through [rustup](https://rustup.rs/). The repository pins Rust 1.99.0.
On Ubuntu/Debian, install the Linux GUI build dependencies and a Japanese font:

```bash
sudo apt-get update
sudo apt-get install -y build-essential pkg-config libgtk-3-dev \
  libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev \
  libxkbcommon-dev libssl-dev fonts-noto-cjk
```

Then clone and run in a graphical desktop session:

```bash
git clone https://github.com/rsasaki0109/rust-office.git
cd rust-office
cargo run --locked -p office-ui
```

The suite starts in Writer. Use the File menu to create a Calc workbook or Impress
presentation. Linux release binaries are also available as artifacts in successful
[CI runs](https://github.com/rsasaki0109/rust-office/actions/workflows/ci.yml).
Windows/macOS distribution artifacts and signed installers are not yet provided.

## What you can do

### Writer

- Edit text with selection, clipboard, Undo/Redo, basic character formatting,
  alignment and Normal / Heading 1–3 styles.
- Add tables, embedded images, nested lists, hyperlinks and page/section breaks.
- Find and replace body text, including Japanese; Replace All is one Undo step.
- Set paper size, orientation, margins and basic headers/footers per section;
  preserve these settings through DOCX/ODT and PDF output.
- Print or export PDF using the editor's page layout.

### Calc

- Edit multiple sheets in a sparse grid with rectangular selection, tabular
  copy/cut/paste and Undo/Redo.
- Evaluate A1 references, arithmetic, SUM, AVERAGE, MIN, MAX, IF and COUNT;
  formula copying adjusts relative, absolute and mixed references.
- Reference other sheets, including quoted Japanese names and ranges:
  `=SUM('売上'!$B$1:B10)`. Renaming/deleting sheets updates qualified formulas
  with Undo/Redo; cycles and excessive evaluation are rejected.
- Apply basic number/date formats, bold/italic, column widths and row heights;
  retain them in XLSX and recovery copies.

### Impress

- Create, duplicate, reorder and delete slides with Light, Dark or Ocean themes.
- Select, move and resize text boxes, rectangles, ellipses and embedded images;
  retain object geometry and speaker notes with Undo/Redo.
- Present from the beginning with F5 or the current slide with Shift+F5;
  navigate with arrows/Space, Home/End and Esc.
- Export editable basic PPTX objects or raster PDF slides. rust-office's own
  basic PPTX profile preserves supported geometry, styling, images and notes.

## File formats

| App | Open | Save / export |
| --- | --- | --- |
| Writer | Native `.roffice.json`, DOCX, ODT | Native JSON, DOCX, ODT; PDF / Print |
| Calc | CSV, XLSX; bounded XLSM, BIFF8 XLS and ODS import | XLSX, CSV |
| Impress | Native JSON, supported PPTX subset | Native JSON; separate PPTX and PDF export |

Format support is intentionally limited. Calc imports from XLSM/XLS/ODS require
Save As to XLSX/CSV; macros are not supported. Multi-sheet workbooks must use XLSX
to avoid losing sheets. Impress uses native JSON for Save and separate PPTX export.
See [development notes](docs/DEVELOPMENT.md) for detailed interchange coverage.

## Data protection

All three apps confirm before replacing unsaved work and keep the current document
when an open or save fails. Saves use a temporary file and replace the destination
only after writing succeeds.

Recovery copies preserve unsaved document data, including Calc formula-bar drafts
and Impress objects/notes. Recovered work opens without the original save path and
asks for a destination. Recovery does not restore Undo/Redo history; edits since
the last successful snapshot can be lost. It is not a substitute for backups.

Input and model limits reject oversized files, archive expansion and unsupported
structures before replacing the current document. See
[INPUT_LIMITS.md](docs/INPUT_LIMITS.md) for bounds and caveats. Linux crash/restart
and failed-import/save behavior have been exercised; Windows/macOS acceptance is
still outstanding.

## Remaining work

- Calc bar/line charts with updates and save/reload. Cross-sheet formulas are
  merged in [PR #31](https://github.com/rsasaki0109/rust-office/pull/31), but the
  combined spreadsheet roadmap milestone remains incomplete.
- Windows/macOS builds and distribution artifacts, followed by actual startup,
  Japanese input, Open, Save and close checks.
- Keyboard access, accessibility information and per-OS IME/clipboard/focus checks.

Broader limitations include the small Excel function set, external-workbook/3D
references, pivots, richer DOCX/ODT styles and floating images, arbitrary PPTX
layouts/animations, and spell checking. Writer PDF glyph metrics are approximate;
Impress PDF uses raster slides without selectable/searchable text. Full format
fidelity and signed installers remain longer-term goals.

## Development

```bash
cargo build --locked --workspace
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
```

CI runs workspace tests, Clippy and a Linux release build. The cross-sheet merge
was validated locally with **360 passing tests**, Clippy and the UI build.
See [DEVELOPMENT.md](docs/DEVELOPMENT.md) for implementation and verification notes.

```text
crates/
  office-core/    # Document model, selection, editing, Undo/Redo (GUI-free)
  office-calc/    # Spreadsheet model, formulas, CSV / XLSX
  office-impress/ # Presentation model, themes, PPTX
  office-format/  # Writer JSON / ODT / DOCX interchange
  office-render/  # Page layout, hit testing, layout-based PDF
  office-ui/      # egui Writer + Calc + Impress (`rust-office`)
```

Contributions should keep GUI code out of the shared model, use small reviewable
PRs, and cover meaningful editing, Undo/Redo and serialization behavior with tests.
Avoid `unsafe` without a clear, documented need.

## License

Dual-licensed under [MIT](LICENSE-MIT) or
[Apache 2.0](LICENSE-APACHE), at your option.
