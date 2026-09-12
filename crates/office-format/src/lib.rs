//! Document file formats for rust-office.
//!
//! * JSON (`.roffice.json`) — native interchange
//! * ODT (`.odt`) — minimal OpenDocument Text round-trip
//! * DOCX (`.docx`) — minimal Office Open XML Word round-trip
//! * PDF — export-only multi-page A4

mod docx;
mod odt;
mod pdf;

use std::fs;
use std::io::{Read, Write};
use std::path::Path;

use office_core::Document;
use thiserror::Error;

pub use docx::{build_document_xml, parse_document_parts, parse_document_xml, DocxFormat};
pub use odt::{
    apply_styles_xml, build_content_xml, parse_content_xml, parse_master_header_footer, OdtFormat,
};
pub use pdf::{document_to_pdf_bytes, write_document_pdf_path, PdfError};

#[derive(Debug, Error)]
pub enum FormatError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("zip error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("unsupported format version {found} (supported up to {supported})")]
    UnsupportedVersion { found: u32, supported: u32 },
    #[error("invalid document: {0}")]
    InvalidDocument(String),
}

/// Abstraction for load/save backends.
pub trait DocumentFormat {
    fn extension(&self) -> &str;
    fn load_from_reader(&self, reader: &mut dyn Read) -> Result<Document, FormatError>;
    fn save_to_writer(&self, document: &Document, writer: &mut dyn Write)
        -> Result<(), FormatError>;

    fn load_path(&self, path: &Path) -> Result<Document, FormatError> {
        let mut file = fs::File::open(path)?;
        self.load_from_reader(&mut file)
    }

    fn save_path(&self, document: &Document, path: &Path) -> Result<(), FormatError> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        let mut file = fs::File::create(path)?;
        self.save_to_writer(document, &mut file)?;
        Ok(())
    }
}

/// Native JSON format used by rust-office.
#[derive(Debug, Default, Clone, Copy)]
pub struct JsonFormat;

impl DocumentFormat for JsonFormat {
    fn extension(&self) -> &str {
        "roffice.json"
    }

    fn load_from_reader(&self, reader: &mut dyn Read) -> Result<Document, FormatError> {
        let mut buf = String::new();
        reader.read_to_string(&mut buf)?;
        let doc: Document = serde_json::from_str(&buf)?;
        if doc.format_version > Document::CURRENT_FORMAT_VERSION {
            return Err(FormatError::UnsupportedVersion {
                found: doc.format_version,
                supported: Document::CURRENT_FORMAT_VERSION,
            });
        }
        if doc.sections.is_empty() {
            return Err(FormatError::InvalidDocument(
                "document has no sections".into(),
            ));
        }
        Ok(doc)
    }

    fn save_to_writer(
        &self,
        document: &Document,
        writer: &mut dyn Write,
    ) -> Result<(), FormatError> {
        let json = serde_json::to_string_pretty(document)?;
        writer.write_all(json.as_bytes())?;
        writer.write_all(b"\n")?;
        Ok(())
    }
}

/// Guess a format from a file path extension.
pub fn format_for_path(path: &Path) -> Option<Box<dyn DocumentFormat>> {
    let name = path.file_name()?.to_string_lossy().to_lowercase();
    if name.ends_with(".odt") {
        Some(Box::new(OdtFormat))
    } else if name.ends_with(".docx") {
        Some(Box::new(DocxFormat))
    } else if name.ends_with(".roffice.json") || name.ends_with(".json") {
        Some(Box::new(JsonFormat))
    } else {
        None
    }
}

pub fn load_document(path: &Path) -> Result<Document, FormatError> {
    let format = format_for_path(path).unwrap_or_else(|| Box::new(JsonFormat));
    format.load_path(path)
}

pub fn save_document(document: &Document, path: &Path) -> Result<(), FormatError> {
    let format = format_for_path(path).unwrap_or_else(|| Box::new(JsonFormat));
    format.save_path(document, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use office_core::{Alignment, Document, DocumentEditor, DocPosition, Selection};

    #[test]
    fn round_trip_json() {
        let mut ed = DocumentEditor::default();
        ed.insert_text("Hello\n世界").unwrap();
        ed.toggle_bold();
        let doc = ed.document().clone();

        let mut buf = Vec::new();
        JsonFormat.save_to_writer(&doc, &mut buf).unwrap();
        let restored = JsonFormat
            .load_from_reader(&mut buf.as_slice())
            .unwrap();
        assert_eq!(restored.plain_text(), "Hello\n世界");
        assert_eq!(restored.format_version, Document::CURRENT_FORMAT_VERSION);
    }

    #[test]
    fn json_round_trip_named_heading() {
        use office_core::NamedParagraphStyle;
        let mut ed = DocumentEditor::default();
        ed.insert_text("Chapter").unwrap();
        ed.set_named_paragraph_style(NamedParagraphStyle::Heading2)
            .unwrap();
        let mut buf = Vec::new();
        JsonFormat
            .save_to_writer(ed.document(), &mut buf)
            .unwrap();
        let json = String::from_utf8(buf.clone()).unwrap();
        assert!(json.contains("\"named\": \"heading2\""), "json={json}");
        let restored = JsonFormat
            .load_from_reader(&mut buf.as_slice())
            .unwrap();
        assert_eq!(
            restored.paragraph(0).unwrap().style.named,
            NamedParagraphStyle::Heading2
        );
        assert_eq!(
            restored.paragraph(0).unwrap().runs[0].style.font_size,
            16.0
        );
    }

    #[test]
    fn path_helpers_json() {
        let dir = std::env::temp_dir().join("rust-office-format-json-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sample.roffice.json");

        let doc = Document::with_text("save me");
        save_document(&doc, &path).unwrap();
        let loaded = load_document(&path).unwrap();
        assert_eq!(loaded.plain_text(), "save me");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn odt_round_trip_styled() {
        let mut ed = DocumentEditor::default();
        ed.insert_text("Hello").unwrap();
        ed.set_selection(Selection {
            anchor: DocPosition::new(0, 0),
            focus: DocPosition::new(0, 5),
        });
        ed.toggle_bold();
        ed.set_selection(Selection::caret(DocPosition::new(0, 5)));
        ed.insert_text("\n世界").unwrap();
        ed.set_alignment(Alignment::Center).unwrap();
        let doc = ed.document().clone();

        let bytes = OdtFormat.save_to_bytes(&doc).unwrap();
        assert!(bytes.starts_with(b"PK"));
        let restored = OdtFormat.load_from_bytes(&bytes).unwrap();
        assert_eq!(restored.plain_text(), "Hello\n世界");
        assert!(restored.paragraph(0).unwrap().runs.iter().any(|r| r.style.bold));
        assert_eq!(
            restored.paragraph(1).unwrap().style.alignment,
            Alignment::Center
        );
    }

    #[test]
    fn odt_path_helpers() {
        let dir = std::env::temp_dir().join("rust-office-format-odt-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sample.odt");

        let doc = Document::with_text("odt path");
        save_document(&doc, &path).unwrap();
        let loaded = load_document(&path).unwrap();
        assert_eq!(loaded.plain_text(), "odt path");
        assert!(OdtFormat.is_implemented());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_minimal_content_xml() {
        let xml = r#"<?xml version="1.0"?>
<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
 xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"
 xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0"
 xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0">
 <office:automatic-styles>
  <style:style style:name="T1" style:family="text">
   <style:text-properties fo:font-weight="bold"/>
  </style:style>
 </office:automatic-styles>
 <office:body><office:text>
  <text:p>Plain <text:span text:style-name="T1">Bold</text:span></text:p>
 </office:text></office:body>
</office:document-content>"#;
        let doc = parse_content_xml(xml).unwrap();
        assert_eq!(doc.plain_text(), "Plain Bold");
        assert!(doc.paragraph(0).unwrap().runs.iter().any(|r| r.style.bold && r.text == "Bold"));
    }

    #[test]
    fn odt_round_trip_table() {
        use office_core::{Block, TableCell};

        let mut ed = DocumentEditor::default();
        ed.insert_text("Before").unwrap();
        ed.insert_table(2, 2).unwrap();
        if let Some(Block::Table(t)) = ed
            .document_mut()
            .blocks_mut()
            .iter_mut()
            .find(|b| matches!(b, Block::Table(_)))
        {
            t.rows[0].cells[0] = TableCell::from_text("A1");
            t.rows[0].cells[1] = TableCell::from_text("B1");
            t.rows[1].cells[0] = TableCell::from_text("A2");
            t.rows[1].cells[1] = TableCell::from_text("B2");
        }
        let doc = ed.document().clone();
        let bytes = OdtFormat.save_to_bytes(&doc).unwrap();
        let restored = OdtFormat.load_from_bytes(&bytes).unwrap();
        assert!(restored.plain_text().contains("Before"));
        assert!(restored.plain_text().contains("A1"));
        assert!(restored
            .blocks()
            .iter()
            .any(|b| matches!(b, Block::Table(_))));
    }

    #[test]
    fn json_round_trip_embedded_image() {
        use image::{ImageBuffer, ImageFormat, Rgb};
        use office_core::{Block, Image};

        let img: ImageBuffer<Rgb<u8>, _> =
            ImageBuffer::from_fn(4, 3, |x, y| Rgb([x as u8 * 40, y as u8 * 40, 180]));
        let mut bytes = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();

        let mut ed = DocumentEditor::default();
        ed.insert_text("cap").unwrap();
        ed.insert_image(Image::from_embedded(
            "image/png",
            bytes.clone(),
            "dot.png",
            80.0,
            60.0,
        ))
        .unwrap();

        let doc = ed.document().clone();
        let mut buf = Vec::new();
        JsonFormat.save_to_writer(&doc, &mut buf).unwrap();
        let restored = JsonFormat
            .load_from_reader(&mut buf.as_slice())
            .unwrap();
        let Block::Image(img) = &restored.blocks()[1] else {
            panic!("expected image block");
        };
        assert!(img.is_embedded());
        assert_eq!(img.alt_text, "dot.png");
        match &img.source {
            office_core::ImageSource::Embedded { mime, data } => {
                assert_eq!(mime, "image/png");
                assert_eq!(data, &bytes);
            }
            _ => panic!("expected embedded source"),
        }
    }

    #[test]
    fn odt_round_trip_embedded_image() {
        use image::{ImageBuffer, ImageFormat, Rgb};
        use office_core::{Block, Image, ImageSource};

        let img: ImageBuffer<Rgb<u8>, _> =
            ImageBuffer::from_fn(4, 3, |x, y| Rgb([x as u8 * 40, y as u8 * 40, 180]));
        let mut bytes = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();

        let mut ed = DocumentEditor::default();
        ed.insert_image(Image::from_embedded(
            "image/png",
            bytes.clone(),
            "dot.png",
            90.0,
            60.0,
        ))
        .unwrap();
        let doc = ed.document().clone();
        let package = OdtFormat.save_to_bytes(&doc).unwrap();
        assert!(package.starts_with(b"PK"));
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&package)).unwrap();
        assert!(
            (0..zip.len()).any(|i| zip.by_index(i).unwrap().name().starts_with("Pictures/")),
            "expected Pictures/ in ODT"
        );

        let restored = OdtFormat.load_from_bytes(&package).unwrap();
        let Block::Image(img) = restored
            .blocks()
            .iter()
            .find(|b| matches!(b, Block::Image(_)))
            .unwrap()
        else {
            unreachable!()
        };
        match &img.source {
            ImageSource::Embedded { mime, data } => {
                assert_eq!(mime, "image/png");
                assert_eq!(data, &bytes);
            }
            _ => panic!("expected embedded image after ODT round-trip"),
        }
        assert!((img.width_pt - 90.0).abs() < 0.5);
        assert!((img.height_pt - 60.0).abs() < 0.5);
    }

    #[test]
    fn odt_round_trip_list_and_hyperlink() {
        use office_core::{ListStyle, Selection};

        let mut ed = DocumentEditor::default();
        ed.insert_text("Item A").unwrap();
        ed.toggle_bullet_list().unwrap();
        ed.set_selection(Selection::caret(office_core::DocPosition::new(0, 6)));
        ed.insert_text("\nItem B").unwrap();
        ed.set_selection(Selection {
            anchor: office_core::DocPosition::new(0, 0),
            focus: office_core::DocPosition::new(0, 6),
        });
        ed.set_hyperlink(Some("https://example.com/a".into()))
            .unwrap();

        let doc = ed.document().clone();
        let bytes = OdtFormat.save_to_bytes(&doc).unwrap();
        let restored = OdtFormat.load_from_bytes(&bytes).unwrap();
        assert_eq!(
            restored.paragraph(0).unwrap().style.list,
            Some(ListStyle::bullet())
        );
        assert_eq!(
            restored.paragraph(1).unwrap().style.list,
            Some(ListStyle::bullet())
        );
        assert_eq!(
            restored.paragraph(0).unwrap().runs[0].link.as_deref(),
            Some("https://example.com/a")
        );
    }

    #[test]
    fn odt_round_trip_nested_list() {
        use office_core::{ListStyle, Selection};

        let mut ed = DocumentEditor::default();
        ed.insert_text("Parent\nChild\nSibling").unwrap();
        ed.select_all();
        ed.toggle_bullet_list().unwrap();
        ed.set_selection(Selection::caret(office_core::DocPosition::new(1, 0)));
        ed.indent_list().unwrap();

        let doc = ed.document().clone();
        let bytes = OdtFormat.save_to_bytes(&doc).unwrap();
        let content = {
            let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
            let mut file = zip.by_name("content.xml").unwrap();
            let mut s = String::new();
            std::io::Read::read_to_string(&mut file, &mut s).unwrap();
            s
        };
        // Nested list: more than one <text:list> open before close.
        assert!(content.matches("<text:list ").count() >= 2);

        let restored = OdtFormat.load_from_bytes(&bytes).unwrap();
        assert_eq!(
            restored.paragraph(0).unwrap().style.list,
            Some(ListStyle::bullet())
        );
        assert_eq!(
            restored.paragraph(1).unwrap().style.list,
            Some(ListStyle::bullet().with_level(1))
        );
        assert_eq!(
            restored.paragraph(2).unwrap().style.list,
            Some(ListStyle::bullet())
        );
    }

    #[test]
    fn json_and_odt_round_trip_page_break_and_header() {
        use office_core::Block;

        let mut ed = DocumentEditor::default();
        ed.insert_text("One").unwrap();
        ed.insert_page_break().unwrap();
        ed.insert_text("Two").unwrap();
        ed.set_header_text("Hdr").unwrap();
        ed.set_footer_text("Page {page}").unwrap();
        let doc = ed.document().clone();

        let mut buf = Vec::new();
        JsonFormat.save_to_writer(&doc, &mut buf).unwrap();
        let json = JsonFormat.load_from_reader(&mut buf.as_slice()).unwrap();
        assert!(json.blocks().iter().any(|b| matches!(b, Block::PageBreak)));
        assert_eq!(json.header().unwrap().plain_text(), "Hdr");
        assert_eq!(json.footer().unwrap().plain_text(), "Page {page}");
        assert_eq!(json.plain_text(), "One\nTwo");

        let bytes = OdtFormat.save_to_bytes(&doc).unwrap();
        let content = {
            let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
            let mut file = zip.by_name("content.xml").unwrap();
            let mut s = String::new();
            std::io::Read::read_to_string(&mut file, &mut s).unwrap();
            s
        };
        assert!(content.contains("P_pagebreak"));
        let styles = {
            let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
            let mut file = zip.by_name("styles.xml").unwrap();
            let mut s = String::new();
            std::io::Read::read_to_string(&mut file, &mut s).unwrap();
            s
        };
        assert!(styles.contains("Hdr"));
        assert!(styles.contains("Page {page}"));

        let restored = OdtFormat.load_from_bytes(&bytes).unwrap();
        assert!(
            restored
                .blocks()
                .iter()
                .any(|b| matches!(b, Block::PageBreak)),
            "expected PageBreak after ODT round-trip"
        );
        assert_eq!(restored.plain_text(), "One\nTwo");
        assert_eq!(restored.header().unwrap().plain_text(), "Hdr");
        assert_eq!(restored.footer().unwrap().plain_text(), "Page {page}");
    }

    #[test]
    fn docx_round_trip_named_heading() {
        use office_core::NamedParagraphStyle;

        let mut ed = DocumentEditor::default();
        ed.insert_text("Chapter One").unwrap();
        ed.set_named_paragraph_style(NamedParagraphStyle::Heading1)
            .unwrap();
        let xml = build_document_xml(ed.document());
        assert!(
            xml.contains(r#"w:pStyle w:val="Heading1""#),
            "xml snippet missing pStyle: {}",
            &xml[..xml.len().min(600)]
        );
        let bytes = DocxFormat.save_to_bytes(ed.document()).unwrap();
        let restored = DocxFormat.load_from_bytes(&bytes).unwrap();
        assert_eq!(
            restored.paragraph(0).unwrap().style.named,
            NamedParagraphStyle::Heading1
        );
        assert!(restored.paragraph(0).unwrap().style.space_after > 6.0);
    }

    #[test]
    fn odt_round_trip_named_heading() {
        use office_core::NamedParagraphStyle;

        let mut ed = DocumentEditor::default();
        ed.insert_text("Section").unwrap();
        ed.set_named_paragraph_style(NamedParagraphStyle::Heading2)
            .unwrap();
        let content = build_content_xml(ed.document());
        assert!(
            content.contains(r#"text:style-name="Heading2""#),
            "content={content}"
        );
        let bytes = OdtFormat.save_to_bytes(ed.document()).unwrap();
        let restored = OdtFormat.load_from_bytes(&bytes).unwrap();
        assert_eq!(
            restored.paragraph(0).unwrap().style.named,
            NamedParagraphStyle::Heading2
        );
    }

    #[test]
    fn docx_round_trip_styled_table_pagebreak() {
        use office_core::Block;

        let mut ed = DocumentEditor::default();
        ed.insert_text("Hello").unwrap();
        ed.set_selection(Selection {
            anchor: DocPosition::new(0, 0),
            focus: DocPosition::new(0, 5),
        });
        ed.toggle_bold();
        ed.set_selection(Selection::caret(DocPosition::new(0, 5)));
        ed.insert_text("\n世界").unwrap();
        ed.set_alignment(Alignment::Center).unwrap();
        ed.insert_page_break().unwrap();
        ed.insert_text("After").unwrap();
        ed.insert_table(2, 2).unwrap();
        ed.insert_text("A1").unwrap();

        let doc = ed.document().clone();
        let bytes = DocxFormat.save_to_bytes(&doc).unwrap();
        assert!(bytes.starts_with(b"PK"));
        let restored = DocxFormat.load_from_bytes(&bytes).unwrap();
        assert!(restored.plain_text().contains("Hello"));
        assert!(restored.plain_text().contains("世界"));
        assert!(restored.plain_text().contains("After"));
        assert!(restored.plain_text().contains("A1"));
        assert!(restored.paragraph(0).unwrap().runs.iter().any(|r| r.style.bold));
        assert_eq!(
            restored.paragraph(1).unwrap().style.alignment,
            Alignment::Center
        );
        assert!(restored
            .blocks()
            .iter()
            .any(|b| matches!(b, Block::PageBreak)));
        assert!(restored
            .blocks()
            .iter()
            .any(|b| matches!(b, Block::Table(_))));
    }

    #[test]
    fn docx_round_trip_image_and_header_footer() {
        use office_core::{Block, Image, ImageSource};

        // 1×1 PNG
        let png: Vec<u8> = vec![
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00,
            0x00, 0x90, 0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08,
            0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D,
            0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];

        let mut ed = DocumentEditor::default();
        ed.insert_text("Before").unwrap();
        ed.insert_image(Image::from_embedded(
            "image/png",
            png.clone(),
            "pixel",
            72.0,
            72.0,
        ))
        .unwrap();
        ed.insert_text("After").unwrap();
        ed.set_header_text("Hdr DOCX").unwrap();
        ed.set_footer_text("Page {page}").unwrap();

        let bytes = DocxFormat.save_to_bytes(ed.document()).unwrap();
        let restored = DocxFormat.load_from_bytes(&bytes).unwrap();
        assert!(restored.plain_text().contains("Before"));
        assert!(restored.plain_text().contains("After"));
        assert_eq!(restored.header().unwrap().plain_text(), "Hdr DOCX");
        assert_eq!(restored.footer().unwrap().plain_text(), "Page {page}");
        let img = restored
            .blocks()
            .iter()
            .find_map(|b| match b {
                Block::Image(i) => Some(i),
                _ => None,
            })
            .expect("image block");
        match &img.source {
            ImageSource::Embedded { mime, data } => {
                assert_eq!(mime, "image/png");
                assert_eq!(data, &png);
            }
            other => panic!("expected embedded image, got {other:?}"),
        }
        assert!((img.width_pt - 72.0).abs() < 0.5);
        assert!((img.height_pt - 72.0).abs() < 0.5);
    }
}
