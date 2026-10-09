use super::*;
use office_core::{Block, DocumentEditor, Image};
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
    editor.insert_table(2, 2).unwrap();
    editor.insert_text("A1").unwrap();
    editor.move_cell(true).unwrap();
    editor.insert_text("B1").unwrap();
    OdtFormat.save_to_bytes(editor.document()).unwrap()
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
fn missing_referenced_image_rejects_the_entire_document() {
    let path = "Pictures/image1.png";
    let error = OdtFormat
        .load_from_bytes(&package(&[(path, None)]))
        .unwrap_err();
    assert!(error.to_string().contains(path), "{error}");
}

#[test]
fn truncated_content_does_not_return_a_partial_document() {
    let xml = part("content.xml").replace("</office:document-content>", "");
    let error = OdtFormat
        .load_from_bytes(&package(&[("content.xml", Some(xml.as_bytes()))]))
        .unwrap_err();
    assert!(error.to_string().contains("content.xml"));
}

#[test]
fn malformed_styles_are_not_silently_ignored() {
    let xml = part("styles.xml").replace("</style:header>", "</style:wrong>");
    let error = OdtFormat
        .load_from_bytes(&package(&[("styles.xml", Some(xml.as_bytes()))]))
        .unwrap_err();
    assert!(error.to_string().contains("styles.xml"));
}

#[test]
fn invalid_content_roots_attributes_entities_and_utf8_name_the_part() {
    let xml = part("content.xml");
    for content in [
        String::new(),
        format!("{xml}<extra/>"),
        xml.replace("document-content", "document-styles"),
        xml.replace("office:text", "office:spreadsheet"),
        xml.replace("<office:text>", "<office:text/><office:text>"),
        xml.replace("Before &amp; 日本語", "Bad &unknown;"),
        xml.replace(
            "xlink:href=\"Pictures/image1.png\"",
            "xlink:href=\"Pictures/image1.png\" xlink:href=\"duplicate\"",
        ),
        xml.replace("xmlns:xlink=", "xmlns:unused="),
        xml.replace("Pictures/image1.png", "Pictures/&unknown;.png"),
    ] {
        let error = OdtFormat
            .load_from_bytes(&package(&[("content.xml", Some(content.as_bytes()))]))
            .unwrap_err();
        assert!(error.to_string().contains("content.xml"), "{error}");
    }
    for path in ["content.xml", "styles.xml"] {
        let error = OdtFormat
            .load_from_bytes(&package(&[(path, Some(b"\xff"))]))
            .unwrap_err();
        assert!(error.to_string().contains(path), "{error}");
    }
}

#[test]
fn absent_styles_are_optional_but_present_invalid_styles_fail() {
    let doc = OdtFormat
        .load_from_bytes(&package(&[("styles.xml", None)]))
        .unwrap();
    assert!(doc.header().is_none());
    assert!(doc.plain_text().contains("Before & 日本語"));
    let styles = part("styles.xml");
    for xml in [
        String::new(),
        styles.replace("</office:document-styles>", ""),
        styles.replace("document-styles", "document-content"),
        styles.replace("Header &amp; 日本語", "&unknown;"),
        format!("{styles}<extra/>"),
    ] {
        let error = OdtFormat
            .load_from_bytes(&package(&[("styles.xml", Some(xml.as_bytes()))]))
            .unwrap_err();
        assert!(error.to_string().contains("styles.xml"), "{error}");
    }
    let error = OdtFormat
        .load_from_bytes(&package(&[("content.xml", None)]))
        .unwrap_err();
    assert!(error.to_string().contains("content.xml"));
}

#[test]
fn referenced_images_resolve_namespace_aliases_and_package_uris() {
    let original = OdtFormat.load_from_bytes(&sample()).unwrap();
    for href in [
        "/Pictures/image1.png",
        "./Pictures/image1.png",
        "Pictures/../Pictures/image1.png",
        "Pictures/%69mage1.png",
    ] {
        let xml = part("content.xml")
            .replace("Pictures/image1.png", href)
            .replace("xmlns:office=", "xmlns:odf=")
            .replace("<office:", "<odf:")
            .replace("</office:", "</odf:")
            .replace(" office:", " odf:")
            .replace("xmlns:draw=", "xmlns:drawing=")
            .replace("draw:", "drawing:")
            .replace("xmlns:xlink=", "xmlns:link=")
            .replace("xlink:", "link:");
        let restored = OdtFormat
            .load_from_bytes(&package(&[("content.xml", Some(xml.as_bytes()))]))
            .unwrap();
        assert_eq!(restored.blocks(), original.blocks(), "{href}");
    }
    let xml = part("content.xml").replace("Pictures/image1.png", "assets/a%26b.png");
    let png = match original
        .blocks()
        .iter()
        .find(|block| matches!(block, Block::Image(_)))
        .unwrap()
    {
        Block::Image(image) => match &image.source {
            office_core::ImageSource::Embedded { data, .. } => data,
            _ => panic!("expected embedded image"),
        },
        _ => unreachable!(),
    };
    let restored = OdtFormat
        .load_from_bytes(&package(&[
            ("content.xml", Some(xml.as_bytes())),
            ("Pictures/image1.png", None),
            ("assets/a&b.png", Some(png)),
        ]))
        .unwrap();
    assert_eq!(restored.blocks(), original.blocks());
}

#[test]
fn invalid_external_missing_or_duplicate_image_hrefs_are_errors() {
    for href in [
        "",
        "../image.png",
        "https://example.com/image.png",
        "//host/image.png",
        "%GG.png",
        "Pictures/a%2Fb.png",
        "Pictures/a%00.png",
        "Pictures/image1.png#part",
        "C:\\image.png",
    ] {
        let xml = part("content.xml").replace("Pictures/image1.png", href);
        let error = OdtFormat
            .load_from_bytes(&package(&[("content.xml", Some(xml.as_bytes()))]))
            .unwrap_err();
        assert!(error.to_string().contains("content.xml"), "{href}: {error}");
    }
    for replacement in ["", "href=\"Pictures/image1.png\"", "xmlns:other=\"http://www.w3.org/1999/xlink\" xlink:href=\"Pictures/image1.png\" other:href=\"Pictures/image1.png\""] {
        let xml = part("content.xml").replace("xlink:href=\"Pictures/image1.png\"", replacement);
        assert!(OdtFormat.load_from_bytes(&package(&[("content.xml", Some(xml.as_bytes()))])).is_err());
    }
}

fn corrupt_crc(bytes: &mut [u8], path: &str) {
    let mut offset = bytes
        .windows(4)
        .position(|window| window == b"PK\x01\x02")
        .unwrap();
    while bytes.get(offset..offset + 4) == Some(b"PK\x01\x02") {
        let length = |at: usize| {
            usize::from(u16::from_le_bytes([
                bytes[offset + at],
                bytes[offset + at + 1],
            ]))
        };
        let name_len = length(28);
        let next = offset + 46 + name_len + length(30) + length(32);
        if &bytes[offset + 46..offset + 46 + name_len] == path.as_bytes() {
            bytes[offset + 16] ^= 1;
            return;
        }
        offset = next;
    }
    panic!("ZIP part not found: {path}");
}

#[test]
fn zip_read_errors_reject_referenced_parts_but_unused_pictures_are_ignored() {
    for path in ["content.xml", "styles.xml", "Pictures/image1.png"] {
        let mut bytes = package(&[]);
        corrupt_crc(&mut bytes, path);
        let error = OdtFormat.load_from_bytes(&bytes).unwrap_err();
        assert!(error.to_string().contains(path), "{error}");
    }
    let mut bytes = package(&[("Pictures/unused.png", Some(b"unused"))]);
    corrupt_crc(&mut bytes, "Pictures/unused.png");
    let doc = OdtFormat.load_from_bytes(&bytes).unwrap();
    assert_eq!(
        doc.blocks(),
        OdtFormat.load_from_bytes(&sample()).unwrap().blocks()
    );
}

#[test]
fn cdata_escaped_links_and_header_footer_text_survive_round_trip() {
    let content = part("content.xml").replace("Before &amp; 日本語", "<text:a xlink:href=\"https://example.com/?q=a&amp;lang=ja\"><![CDATA[Before & 日本語]]></text:a>");
    let styles = part("styles.xml")
        .replace("Header &amp; 日本語", "<![CDATA[Header & 日本語]]><text:s text:c=\"&#50;\"/><text:tab/><text:line-break/>Next")
        .replace("Footer {page}", "<![CDATA[Footer {page}]]>")
        .replace("xmlns:office=", "xmlns:odf=").replace("<office:", "<odf:")
            .replace("</office:", "</odf:")
            .replace(" office:", " odf:");
    let original = OdtFormat
        .load_from_bytes(&package(&[
            ("content.xml", Some(content.as_bytes())),
            ("styles.xml", Some(styles.as_bytes())),
        ]))
        .unwrap();
    assert_eq!(
        original.header().unwrap().plain_text(),
        "Header & 日本語  \t\nNext"
    );
    assert_eq!(original.footer().unwrap().plain_text(), "Footer {page}");
    assert_eq!(
        original.paragraph(0).unwrap().runs[0].link.as_deref(),
        Some("https://example.com/?q=a&lang=ja")
    );
    let mut current = original.clone();
    for _ in 0..2 {
        let restored = OdtFormat
            .load_from_bytes(&OdtFormat.save_to_bytes(&current).unwrap())
            .unwrap();
        assert_eq!(restored.blocks(), original.blocks());
        assert_eq!(restored.header(), original.header());
        assert_eq!(restored.footer(), original.footer());
        current = restored;
    }
}

#[test]
fn xml_only_helpers_can_omit_images_and_failed_styles_do_not_mutate_document() {
    let parsed = parse_content_xml(&part("content.xml")).unwrap();
    assert!(parsed.plain_text().contains("Before & 日本語"));
    assert!(parsed.plain_text().contains("A1"));
    let mut doc = OdtFormat.load_from_bytes(&sample()).unwrap();
    let original = doc.clone();
    let styles = part("styles.xml").replace("</office:document-styles>", "");
    assert!(apply_styles_xml(&mut doc, &styles).is_err());
    assert_eq!(doc, original);
}

#[test]
fn valid_empty_body_opens_and_space_expansion_is_bounded() {
    let minimal = r#"<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"><office:body><office:text/></office:body></office:document-content>"#;
    assert_eq!(
        OdtFormat
            .load_from_bytes(&package(&[
                ("content.xml", Some(minimal.as_bytes())),
                ("styles.xml", None)
            ]))
            .unwrap()
            .plain_text(),
        ""
    );
    for path in ["content.xml", "styles.xml"] {
        for count in ["0", "bad", "18446744073709551615", "1000001"] {
            let xml = part(path)
                .replace(
                    "Before &amp; 日本語",
                    &format!("<text:s text:c=\"{count}\"/>"),
                )
                .replace(
                    "Header &amp; 日本語",
                    &format!("<text:s text:c=\"{count}\"/>"),
                );
            let error = OdtFormat
                .load_from_bytes(&package(&[(path, Some(xml.as_bytes()))]))
                .unwrap_err();
            assert!(error.to_string().contains(path), "{error}");
        }
        let xml = part(path)
            .replace(
                "Before &amp; 日本語",
                "<text:s text:c=\"600000\"/><text:s text:c=\"600000\"/>",
            )
            .replace(
                "Header &amp; 日本語",
                "<text:s text:c=\"600000\"/><text:s text:c=\"600000\"/>",
            );
        assert!(OdtFormat
            .load_from_bytes(&package(&[(path, Some(xml.as_bytes()))]))
            .is_err());
    }
}
