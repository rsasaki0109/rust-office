use super::*;
use office_core::{DocumentEditor, Image};
use zip::{write::FileOptions, ZipArchive, ZipWriter};

fn sample() -> Vec<u8> {
    let mut editor = DocumentEditor::default();
    editor.insert_text("Before & 日本語").unwrap();
    editor.set_header_text("Header & 日本語").unwrap();
    editor.set_footer_text("Footer {page}").unwrap();
    let mut png = Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(1, 1)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    editor
        .insert_image(Image::from_embedded(
            "image/png",
            png.into_inner(),
            "pixel",
            72.0,
            72.0,
        ))
        .unwrap();
    editor.insert_text("After").unwrap();
    DocxFormat.save_to_bytes(editor.document()).unwrap()
}

fn part(path: &str) -> String {
    let bytes = sample();
    let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut text = String::new();
    archive
        .by_name(path)
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    text
}

fn package(changes: &[(&str, Option<&[u8]>)]) -> Vec<u8> {
    let mut archive = ZipArchive::new(Cursor::new(sample())).unwrap();
    let names: std::collections::HashSet<String> =
        archive.file_names().map(str::to_owned).collect();
    let mut output = ZipWriter::new(Cursor::new(Vec::new()));
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).unwrap();
        let name = file.name().to_owned();
        let replacement = changes.iter().find(|(path, _)| *path == name);
        if matches!(replacement, Some((_, None))) {
            continue;
        }
        let mut data = Vec::new();
        file.read_to_end(&mut data).unwrap();
        output.start_file(name, FileOptions::default()).unwrap();
        output
            .write_all(replacement.and_then(|(_, bytes)| *bytes).unwrap_or(&data))
            .unwrap();
    }
    for (path, bytes) in changes {
        if let Some(bytes) = bytes.filter(|_| !names.contains(*path)) {
            output.start_file(*path, FileOptions::default()).unwrap();
            output.write_all(bytes).unwrap();
        }
    }
    output.finish().unwrap().into_inner()
}

#[test]
fn missing_referenced_parts_reject_the_entire_document() {
    for path in [
        "word/media/image1.png",
        "word/header1.xml",
        "word/footer1.xml",
    ] {
        let result = DocxFormat.load_from_bytes(&package(&[(path, None)]));
        assert!(result.is_err(), "missing {path} was accepted");
        assert!(result.unwrap_err().to_string().contains(path));
    }
}

#[test]
fn truncated_main_xml_does_not_return_a_partial_document() {
    let xml = part("word/document.xml").replace("</w:document>", "");
    assert!(DocxFormat
        .load_from_bytes(&package(&[("word/document.xml", Some(xml.as_bytes()))]))
        .is_err());
}

#[test]
fn malformed_header_footer_and_relationships_are_errors() {
    for path in [
        "word/header1.xml",
        "word/footer1.xml",
        "word/_rels/document.xml.rels",
    ] {
        let xml = part(path);
        let truncated = &xml[..xml.rfind("</").unwrap()];
        let result = DocxFormat.load_from_bytes(&package(&[(path, Some(truncated.as_bytes()))]));
        assert!(result.is_err(), "truncated {path} was accepted");
        assert!(result.unwrap_err().to_string().contains(path));
    }
}

#[test]
fn missing_relationship_ids_wrong_types_and_external_parts_are_errors() {
    let path = "word/_rels/document.xml.rels";
    let rels = part(path);
    for xml in [
        rels.replace("Id=\"rId1\"", "Id=\"unused\""),
        rels.replace("/relationships/image\"", "/relationships/hyperlink\""),
        rels.replace(
            "Target=\"media/image1.png\"",
            "TargetMode=\"External\" Target=\"media/image1.png\"",
        ),
        rels.replace("Id=\"rId2\"", "Id=\"rId1\""),
        rels.replace("Target=\"header1.xml\"", ""),
    ] {
        let error = DocxFormat
            .load_from_bytes(&package(&[(path, Some(xml.as_bytes()))]))
            .unwrap_err();
        assert!(error.to_string().contains(path), "{error}");
    }
    let error = DocxFormat
        .load_from_bytes(&package(&[(path, None)]))
        .unwrap_err();
    assert!(error.to_string().contains(path));
}

#[test]
fn xml_roots_attributes_entities_and_utf8_fail_with_part_context() {
    let path = "word/document.xml";
    let doc = part(path);
    for xml in [
        String::new(),
        format!("{doc}<extra/>"),
        doc.replace("Before &amp; 日本語", "bad &unknown; text"),
        doc.replace("<w:document ", "<w:wrong ")
            .replace("</w:document>", "</w:wrong>"),
        doc.replace("<w:body>", "<w:other>")
            .replace("</w:body>", "</w:other>"),
        doc.replace("r:embed=\"rId1\"", "r:embed=\"rId1\" r:embed=\"duplicate\""),
    ] {
        let error = DocxFormat
            .load_from_bytes(&package(&[(path, Some(xml.as_bytes()))]))
            .unwrap_err();
        assert!(error.to_string().contains(path));
    }
    for path in [
        "word/document.xml",
        "word/_rels/document.xml.rels",
        "word/header1.xml",
        "word/footer1.xml",
    ] {
        let error = DocxFormat
            .load_from_bytes(&package(&[(path, Some(b"\xff"))]))
            .unwrap_err();
        assert!(error.to_string().contains(path));
    }
    for path in ["word/header1.xml", "word/footer1.xml"] {
        let xml = part(path)
            .replace("Header &amp; 日本語", "bad &unknown;")
            .replace("Footer {page}", "bad &unknown;");
        assert!(DocxFormat
            .load_from_bytes(&package(&[(path, Some(xml.as_bytes()))]))
            .is_err());
    }
}

#[test]
fn unused_relationships_do_not_load_or_replace_document_parts() {
    let path = "word/_rels/document.xml.rels";
    let xml = part(path).replace("</Relationships>", r#"<Relationship Id="unusedHeader" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/header" Target="missing-header.xml"/><Relationship Id="unusedImage" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="missing-image.png"/></Relationships>"#);
    let loaded = DocxFormat
        .load_from_bytes(&package(&[(path, Some(xml.as_bytes()))]))
        .unwrap();
    assert_eq!(loaded.header().unwrap().plain_text(), "Header & 日本語");
    assert_eq!(loaded.footer().unwrap().plain_text(), "Footer {page}");
    assert!(loaded
        .blocks()
        .iter()
        .any(|block| matches!(block, office_core::Block::Image(_))));
}

#[test]
fn plain_documents_can_omit_relationships_and_xml_only_api_can_omit_resources() {
    let xml = r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p/></w:body></w:document>"#;
    let loaded = DocxFormat
        .load_from_bytes(&package(&[
            ("word/document.xml", Some(xml.as_bytes())),
            ("word/_rels/document.xml.rels", None),
            ("word/header1.xml", Some(b"unused broken header")),
        ]))
        .unwrap();
    assert_eq!(loaded.plain_text(), "");
    assert!(loaded.header().is_none());
    assert!(DocxFormat
        .load_from_bytes(&package(&[
            ("word/document.xml", Some(xml.as_bytes())),
            ("word/_rels/document.xml.rels", Some(b"")),
        ]))
        .is_err());
    let parsed = parse_document_xml(&part("word/document.xml"), "").unwrap();
    assert!(parsed.plain_text().contains("Before & 日本語"));
    assert!(parsed.plain_text().contains("After"));
}

#[test]
fn namespace_aliases_and_absolute_relative_encoded_targets_resolve() {
    let rels_path = "word/_rels/document.xml.rels";
    let paths = [
        "word/document.xml",
        "word/_rels/document.xml.rels",
        "word/header1.xml",
        "word/footer1.xml",
    ];
    let replacements: Vec<String> = paths
        .iter()
        .map(|path| {
            part(path)
                .replace(super::xml::WORD_NS[0], super::xml::WORD_NS[1])
                .replace(super::xml::DRAWING_NS[0], super::xml::DRAWING_NS[1])
                .replace(super::xml::REL_NS[0], super::xml::REL_NS[1])
        })
        .collect();
    let changes: Vec<(&str, Option<&[u8]>)> = paths
        .iter()
        .zip(&replacements)
        .map(|(path, xml)| (*path, Some(xml.as_bytes())))
        .collect();
    assert_eq!(
        DocxFormat
            .load_from_bytes(&package(&changes))
            .unwrap()
            .header()
            .unwrap()
            .plain_text(),
        "Header & 日本語"
    );
    for target in [
        "/word/header1.xml",
        "../word/./header1.xml",
        "%68eader1.xml",
    ] {
        let rels =
            part(rels_path).replace("Target=\"header1.xml\"", &format!("Target=\"{target}\""));
        let doc = part("word/document.xml")
            .replace("xmlns:r=", "xmlns:link=")
            .replace("r:embed=", "link:embed=")
            .replace("r:id=", "link:id=");
        let loaded = DocxFormat
            .load_from_bytes(&package(&[
                (rels_path, Some(rels.as_bytes())),
                ("word/document.xml", Some(doc.as_bytes())),
            ]))
            .unwrap();
        assert_eq!(loaded.header().unwrap().plain_text(), "Header & 日本語");
        assert!(loaded
            .blocks()
            .iter()
            .any(|block| matches!(block, office_core::Block::Image(_))));
    }
    for target in [
        "../../header.xml",
        "header.xml#fragment",
        "%XX.xml",
        "https://example.com/header.xml",
    ] {
        let rels =
            part(rels_path).replace("Target=\"header1.xml\"", &format!("Target=\"{target}\""));
        assert!(DocxFormat
            .load_from_bytes(&package(&[(rels_path, Some(rels.as_bytes()))]))
            .is_err());
    }
}

#[test]
fn referenced_image_crc_read_error_is_not_silently_dropped() {
    let mut bytes = package(&[]);
    let mut offset = bytes
        .windows(4)
        .position(|window| window == b"PK\x01\x02")
        .unwrap();
    let mut changed = false;
    while bytes.get(offset..offset + 4) == Some(b"PK\x01\x02") {
        let length = |at: usize| {
            usize::from(u16::from_le_bytes([
                bytes[offset + at],
                bytes[offset + at + 1],
            ]))
        };
        let name_len = length(28);
        let next = offset + 46 + name_len + length(30) + length(32);
        if &bytes[offset + 46..offset + 46 + name_len] == b"word/media/image1.png" {
            bytes[offset + 16] ^= 1; // Corrupt the recorded CRC while retaining readable ZIP metadata.
            changed = true;
            break;
        }
        offset = next;
    }
    assert!(changed);
    let error = DocxFormat.load_from_bytes(&bytes).unwrap_err();
    assert!(error.to_string().contains("word/media/image1.png"));
}

#[test]
fn normal_docx_source_and_resources_survive_two_round_trips() {
    let header =
        part("word/header1.xml").replace("Header &amp; 日本語", "<![CDATA[Header & 日本語]]>");
    let footer = part("word/footer1.xml").replace("Footer {page}", "<![CDATA[Footer {page}]]>");
    let original = DocxFormat
        .load_from_bytes(&package(&[
            ("word/header1.xml", Some(header.as_bytes())),
            ("word/footer1.xml", Some(footer.as_bytes())),
        ]))
        .unwrap();
    assert_eq!(original.header().unwrap().plain_text(), "Header & 日本語");
    assert_eq!(original.footer().unwrap().plain_text(), "Footer {page}");
    let saved = DocxFormat.save_to_bytes(&original).unwrap();
    let restored = DocxFormat.load_from_bytes(&saved).unwrap();
    assert_eq!(restored.plain_text(), original.plain_text());
    assert_eq!(
        restored.header().unwrap().plain_text(),
        original.header().unwrap().plain_text()
    );
    assert_eq!(
        restored.footer().unwrap().plain_text(),
        original.footer().unwrap().plain_text()
    );
    assert_eq!(restored.blocks(), original.blocks());
}

#[test]
fn cdata_and_escaped_hyperlink_targets_are_preserved() {
    let xml = r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><w:body><w:p><w:hyperlink r:id="link&amp;id"><w:r><w:t><![CDATA[日本語 <text>]]></w:t></w:r></w:hyperlink></w:p></w:body></w:document>"#;
    let rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="link&amp;id" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://example.com/?q=a&amp;lang=ja" TargetMode="External"/></Relationships>"#;
    let loaded = DocxFormat
        .load_from_bytes(&package(&[
            ("word/document.xml", Some(xml.as_bytes())),
            ("word/_rels/document.xml.rels", Some(rels.as_bytes())),
        ]))
        .unwrap();
    assert_eq!(loaded.plain_text(), "日本語 <text>");
    assert_eq!(
        loaded.paragraph(0).unwrap().runs[0].link.as_deref(),
        Some("https://example.com/?q=a&lang=ja")
    );
    let restored = DocxFormat
        .load_from_bytes(&DocxFormat.save_to_bytes(&loaded).unwrap())
        .unwrap();
    assert_eq!(restored.plain_text(), loaded.plain_text());
    assert_eq!(
        restored.paragraph(0).unwrap().runs[0].link,
        loaded.paragraph(0).unwrap().runs[0].link
    );
}

#[test]
fn referenced_headers_are_validated_and_default_reference_is_preferred() {
    let doc = part("word/document.xml").replace(
        r#"<w:headerReference w:type="default" r:id="rId2"/>"#,
        r#"<w:headerReference w:type="first" r:id="firstHeader"/><w:headerReference w:type="default" r:id="rId2"/>"#,
    );
    assert!(doc.contains("firstHeader"));
    let rels = part("word/_rels/document.xml.rels").replace("</Relationships>", r#"<Relationship Id="firstHeader" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/header" Target="first-header.xml"/></Relationships>"#);
    let first = part("word/header1.xml").replace("Header &amp; 日本語", "First only");
    let loaded = DocxFormat
        .load_from_bytes(&package(&[
            ("word/document.xml", Some(doc.as_bytes())),
            ("word/_rels/document.xml.rels", Some(rels.as_bytes())),
            ("word/first-header.xml", Some(first.as_bytes())),
        ]))
        .unwrap();
    assert_eq!(loaded.header().unwrap().plain_text(), "Header & 日本語");
    let error = DocxFormat
        .load_from_bytes(&package(&[
            ("word/document.xml", Some(doc.as_bytes())),
            ("word/_rels/document.xml.rels", Some(rels.as_bytes())),
            ("word/first-header.xml", Some(b"bad first header")),
        ]))
        .unwrap_err();
    assert!(error.to_string().contains("word/first-header.xml"));
}
