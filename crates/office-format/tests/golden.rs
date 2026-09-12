//! Golden / snapshot fixtures for JSON + ODT interchange.
//!
//! Checked-in files live under `tests/fixtures/`.
//! Regenerate with:
//!
//! ```bash
//! UPDATE_GOLDEN=1 cargo test -p office-format --test golden
//! ```

use std::fs;
use std::io::Read;
use std::path::PathBuf;

use office_core::{Alignment, Document, DocumentEditor, DocPosition, Selection};
use office_format::{build_content_xml, DocumentFormat, JsonFormat, OdtFormat};

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn update_golden() -> bool {
    matches!(
        std::env::var("UPDATE_GOLDEN").as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    )
}

fn assert_golden_text(relative: &str, actual: &str) {
    let path = fixtures_dir().join(relative);
    if update_golden() {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut out = actual.to_string();
        if !out.ends_with('\n') {
            out.push('\n');
        }
        fs::write(&path, out).unwrap();
        return;
    }
    let expected = fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "missing golden `{relative}` ({e}). Create it with UPDATE_GOLDEN=1 cargo test -p office-format --test golden"
        )
    });
    assert_eq!(
        expected, actual,
        "golden mismatch for `{relative}`.\nRe-run with UPDATE_GOLDEN=1 if the change is intentional."
    );
}

fn json_pretty(doc: &Document) -> String {
    let mut buf = Vec::new();
    JsonFormat.save_to_writer(doc, &mut buf).unwrap();
    String::from_utf8(buf).unwrap()
}

fn odt_zip_entry(bytes: &[u8], name: &str) -> String {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut file = zip.by_name(name).unwrap();
    let mut s = String::new();
    file.read_to_string(&mut s).unwrap();
    s
}

fn assert_case(name: &str, doc: &Document) {
    // JSON golden
    let json = json_pretty(doc);
    assert_golden_text(&format!("json/{name}.roffice.json"), &json);

    // JSON load → structural sanity → re-save matches golden (idempotent)
    let loaded = JsonFormat
        .load_from_reader(&mut json.as_bytes())
        .unwrap_or_else(|e| panic!("{name} JSON load failed: {e}"));
    assert_eq!(
        loaded.plain_text(),
        doc.plain_text(),
        "{name}: plain_text after JSON load"
    );
    assert_eq!(json_pretty(&loaded), json, "{name}: JSON not idempotent");

    // ODT content.xml golden (deterministic; no ZIP timestamps)
    let content = build_content_xml(doc);
    assert_golden_text(&format!("odt/{name}.content.xml"), &content);

    // Full package must still round-trip body text through OdtFormat.
    let package = OdtFormat.save_to_bytes(doc).unwrap();
    assert_eq!(
        odt_zip_entry(&package, "content.xml"),
        content,
        "{name}: package content.xml != build_content_xml"
    );
    let restored = OdtFormat.load_from_bytes(&package).unwrap();
    assert_eq!(
        restored.plain_text(),
        doc.plain_text(),
        "{name}: ODT plain_text round-trip"
    );

    // styles.xml golden when header/footer present
    if doc.header().is_some() || doc.footer().is_some() {
        let styles = odt_zip_entry(&package, "styles.xml");
        assert_golden_text(&format!("odt/{name}.styles.xml"), &styles);
    }
}

fn sample_styled() -> Document {
    let mut ed = DocumentEditor::default();
    ed.insert_text("Hello").unwrap();
    ed.set_selection(Selection {
        anchor: DocPosition::new(0, 0),
        focus: DocPosition::new(0, 5),
    });
    ed.toggle_bold();
    ed.set_selection(Selection::caret(DocPosition::new(0, 5)));
    ed.insert_text(" world").unwrap();
    ed.set_selection(Selection {
        anchor: DocPosition::new(0, 6),
        focus: DocPosition::new(0, 11),
    });
    ed.set_hyperlink(Some("https://example.com".into()))
        .unwrap();
    ed.set_selection(Selection::caret(DocPosition::new(0, 11)));
    ed.insert_text("\nCentered").unwrap();
    ed.set_alignment(Alignment::Center).unwrap();
    ed.document().clone()
}

fn sample_lists_nested() -> Document {
    let mut ed = DocumentEditor::default();
    ed.insert_text("Parent\nChild\nSibling").unwrap();
    ed.select_all();
    ed.toggle_bullet_list().unwrap();
    ed.set_selection(Selection::caret(DocPosition::new(1, 0)));
    ed.indent_list().unwrap();
    ed.document().clone()
}

fn sample_page_break_header() -> Document {
    let mut ed = DocumentEditor::default();
    ed.insert_text("One").unwrap();
    ed.insert_page_break().unwrap();
    ed.insert_text("Two").unwrap();
    ed.set_header_text("Hdr").unwrap();
    ed.set_footer_text("Page {page} of {pages}").unwrap();
    ed.document().clone()
}

fn sample_table() -> Document {
    let mut ed = DocumentEditor::default();
    ed.insert_text("Before").unwrap();
    ed.insert_table(2, 2).unwrap();
    // Focus is in first cell after insert_table.
    ed.insert_text("A1").unwrap();
    ed.move_cell(true).unwrap();
    ed.insert_text("B1").unwrap();
    ed.focus_body();
    let last = ed.document().paragraph_count().saturating_sub(1);
    let len = ed.document().paragraph(last).map(|p| p.char_len()).unwrap_or(0);
    ed.set_selection(Selection::caret(DocPosition::new(last, len)));
    ed.insert_text("After").unwrap();
    ed.document().clone()
}

fn sample_numbered() -> Document {
    let mut ed = DocumentEditor::default();
    ed.insert_text("First\nSecond").unwrap();
    ed.select_all();
    ed.toggle_numbered_list().unwrap();
    ed.document().clone()
}

#[test]
fn golden_styled() {
    assert_case("styled", &sample_styled());
}

#[test]
fn golden_lists_nested() {
    assert_case("lists_nested", &sample_lists_nested());
}

#[test]
fn golden_page_break_header() {
    assert_case("page_break_header", &sample_page_break_header());
}

#[test]
fn golden_table() {
    assert_case("table", &sample_table());
}

#[test]
fn golden_numbered() {
    assert_case("numbered", &sample_numbered());
}

#[test]
fn golden_legacy_json_v2_still_loads() {
    let path = fixtures_dir().join("json/legacy_v2.roffice.json");
    if update_golden() && !path.exists() {
        // Minimal v2-shaped document without header/footer fields.
        let json = r#"{
  "title": "",
  "sections": [
    {
      "blocks": [
        {
          "type": "paragraph",
          "runs": [
            {
              "text": "legacy",
              "style": {
                "bold": false,
                "italic": false,
                "underline": false,
                "font_size": 12.0
              }
            }
          ],
          "style": {
            "alignment": "left",
            "line_spacing": 1.15,
            "space_after": 6.0
          }
        }
      ],
      "page_style": {
        "width": 595.28,
        "height": 841.89,
        "margin_top": 72.0,
        "margin_bottom": 72.0,
        "margin_left": 72.0,
        "margin_right": 72.0
      }
    }
  ],
  "format_version": 2
}
"#;
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, json).unwrap();
    }
    let raw = fs::read_to_string(&path).expect("legacy_v2 fixture");
    let doc = JsonFormat
        .load_from_reader(&mut raw.as_bytes())
        .expect("legacy v2 must load");
    assert_eq!(doc.plain_text(), "legacy");
    assert!(doc.header().is_none());
    assert!(doc.format_version <= Document::CURRENT_FORMAT_VERSION);
}

#[test]
fn golden_odt_content_parses_back() {
    // Every committed content.xml must parse without error.
    let odt_dir = fixtures_dir().join("odt");
    if !odt_dir.exists() {
        if update_golden() {
            return;
        }
        panic!("missing fixtures/odt; run UPDATE_GOLDEN=1");
    }
    for entry in fs::read_dir(&odt_dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let name = path.file_name().unwrap().to_string_lossy();
        if !name.ends_with(".content.xml") {
            continue;
        }
        let xml = fs::read_to_string(&path).unwrap();
        let doc = office_format::parse_content_xml(&xml)
            .unwrap_or_else(|e| panic!("parse {} failed: {e}", path.display()));
        assert!(
            !doc.plain_text().is_empty() || doc.blocks().iter().any(|b| matches!(b, office_core::Block::PageBreak | office_core::Block::Table(_))),
            "{} produced an empty document",
            path.display()
        );
    }
}

#[test]
fn golden_docx_styled_document_xml() {
    let doc = sample_styled();
    let xml = office_format::build_document_xml(&doc);
    assert_golden_text("docx/styled.document.xml", &xml);
    let restored = office_format::parse_document_xml(&xml, "").unwrap();
    assert!(restored.plain_text().contains("Hello"));
    assert!(restored.plain_text().contains("Centered"));
}

#[test]
fn fixtures_readme_documents_update_flow() {
    let readme = fixtures_dir().join("README.md");
    if update_golden() {
        fs::create_dir_all(fixtures_dir()).unwrap();
        fs::write(
            &readme,
            r#"# Format fixtures (golden files)

Checked-in JSON and ODT XML snapshots for `office-format` regression tests.

## Layout

- `json/*.roffice.json` — native document snapshots
- `odt/*.content.xml` — deterministic `content.xml` from `build_content_xml`
- `odt/*.styles.xml` — `styles.xml` when the sample has header/footer
- `docx/*.document.xml` — deterministic `word/document.xml` from DOCX writer
- `json/legacy_v2.roffice.json` — older schema still accepted on load

## Updating

```bash
UPDATE_GOLDEN=1 cargo test -p office-format --test golden
```

Review the diff carefully before committing.
"#,
        )
        .unwrap();
    }
    assert!(
        readme.exists(),
        "fixtures README missing; run UPDATE_GOLDEN=1"
    );
}
