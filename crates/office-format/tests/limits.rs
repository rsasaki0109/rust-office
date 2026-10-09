use office_core::{Block, Document, Image};
use office_format::{DocumentFormat, DocxFormat, JsonFormat, OdtFormat};
use std::io::{Cursor, Read, Write};
fn package(parts: &[(&str, &[u8])]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in parts {
        zip.start_file(
            *name,
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated),
        )
        .unwrap();
        zip.write_all(data).unwrap();
    }
    zip.finish().unwrap().into_inner()
}
#[test]
fn bounded_reader_does_not_consume_more_than_the_limit_plus_one() {
    struct Endless {
        read: usize,
    }
    impl Read for Endless {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            buf.fill(b' ');
            self.read += buf.len();
            Ok(buf.len())
        }
    }
    for format in [&JsonFormat as &dyn DocumentFormat, &DocxFormat, &OdtFormat] {
        let mut reader = Endless { read: 0 };
        assert!(format
            .load_from_reader(&mut reader)
            .unwrap_err()
            .to_string()
            .contains("64 MiB"));
        assert_eq!(reader.read, 64 * 1024 * 1024 + 1);
    }
}
#[test]
fn zip_directory_counts_duplicate_parts_and_expansion_are_checked() {
    let good = DocxFormat
        .save_to_bytes(&Document::with_text("safe"))
        .unwrap();
    let mut expanded = good.clone();
    let central = expanded
        .windows(4)
        .position(|w| w == b"PK\x01\x02")
        .unwrap();
    expanded[central + 24..central + 28].copy_from_slice(&(129u32 * 1024 * 1024).to_le_bytes());
    assert!(DocxFormat.load_from_bytes(&expanded).is_err());
    let mut entries = good.clone();
    let end = entries
        .windows(4)
        .rposition(|w| w == b"PK\x05\x06")
        .unwrap();
    entries[end + 8..end + 10].copy_from_slice(&4097u16.to_le_bytes());
    entries[end + 10..end + 12].copy_from_slice(&4097u16.to_le_bytes());
    assert!(DocxFormat.load_from_bytes(&entries).is_err());
    let duplicate = package(&[
        ("word/document.xml", b"<w/>"),
        ("WORD/DOCUMENT.XML", b"<w/>"),
    ]);
    assert!(DocxFormat
        .load_from_bytes(&duplicate)
        .unwrap_err()
        .to_string()
        .contains("Duplicate"));
}
#[test]
fn xml_size_depth_and_aggregate_space_expansion_are_checked() {
    let large = vec![b' '; 8 * 1024 * 1024 + 1];
    let bytes = package(&[("content.xml", &large)]);
    assert!(OdtFormat
        .load_from_bytes(&bytes)
        .unwrap_err()
        .to_string()
        .contains("content.xml"));
    let deep = format!("{}{}", "<n>".repeat(129), "</n>".repeat(129));
    let bytes = package(&[("word/document.xml", deep.as_bytes())]);
    assert!(DocxFormat
        .load_from_bytes(&bytes)
        .unwrap_err()
        .to_string()
        .contains("nesting"));
    let spaces=format!("<office:document-content xmlns:office=\"urn:oasis:names:tc:opendocument:xmlns:office:1.0\" xmlns:text=\"urn:oasis:names:tc:opendocument:xmlns:text:1.0\"><office:body><office:text><text:p>{}</text:p></office:text></office:body></office:document-content>","<text:s text:c=\"1000000\"/>".repeat(5));
    let bytes = package(&[("content.xml", spaces.as_bytes())]);
    assert!(OdtFormat
        .load_from_bytes(&bytes)
        .unwrap_err()
        .to_string()
        .contains("aggregate"));
}
#[test]
fn failed_export_keeps_the_destination_and_model_limits_cover_images_and_metrics() {
    let path =
        std::env::temp_dir().join(format!("office-writer-limit-{}.docx", std::process::id()));
    std::fs::write(&path, b"original").unwrap();
    let mut doc = Document::with_text("safe");
    doc.sections[0].blocks[0].as_paragraph_mut().unwrap().runs[0]
        .style
        .font_size = f32::INFINITY;
    assert!(DocxFormat.save_path(&doc, &path).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"original");
    std::fs::remove_file(path).unwrap();
    let mut doc = Document::with_text("safe");
    doc.sections[0]
        .blocks
        .push(Block::Image(Image::from_embedded(
            "image/png",
            vec![0; 16 * 1024 * 1024 + 1],
            "oversize",
            20.0,
            20.0,
        )));
    assert!(JsonFormat.save_to_writer(&doc, &mut Vec::new()).is_err());
    assert!(OdtFormat.save_to_bytes(&doc).is_err());
}

#[test]
fn repeated_picture_references_are_bounded_before_model_cloning() {
    let xml = r#"<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:xlink="http://www.w3.org/1999/xlink"><office:body><office:text>IMAGES</office:text></office:body></office:document-content>"#;
    let xml = xml.replace(
        "IMAGES",
        &"<draw:frame><draw:image xlink:href=\"Pictures/a.png\"/></draw:frame>".repeat(5),
    );
    let image = vec![0; 16 * 1024 * 1024];
    let bytes = package(&[("content.xml", xml.as_bytes()), ("Pictures/a.png", &image)]);
    assert!(OdtFormat
        .load_from_bytes(&bytes)
        .unwrap_err()
        .to_string()
        .contains("repeated images"));
}
