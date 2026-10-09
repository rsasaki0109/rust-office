//! Section settings must survive repeated package round trips, not just XML generation.
use office_core::{Alignment, Block, Document, PageStyle, Paragraph, Section, Table};
use office_format::{DocxFormat, OdtFormat};

fn document() -> Document {
    let mut doc = Document::with_text("First 日本語");
    let mut header = Paragraph::from_text("  Title 日本語  ");
    header.runs[0].style.bold = true;
    header.runs[0].style.font_size = 14.0;
    header.style.alignment = Alignment::Center;
    doc.sections[0].header = Some(header);
    doc.sections[0].footer = Some(Paragraph::from_text("{page} / {pages}"));
    doc.sections[0].blocks.push(Block::Table(Table::new(2, 2)));
    let mut landscape = PageStyle {
        width: 792.0,
        height: 612.0,
        margin_top: 42.0,
        margin_bottom: 48.0,
        margin_left: 54.0,
        margin_right: 60.0,
    };
    doc.sections.push(Section {
        blocks: vec![
            Block::Paragraph(Paragraph::from_text("Second")),
            Block::PageBreak,
            Block::Paragraph(Paragraph::from_text("After explicit break")),
        ],
        page_style: landscape.clone(),
        header: Some(Paragraph::from_text("Section two")),
        footer: None,
    });
    landscape.width = 419.53;
    landscape.height = 595.28;
    doc.sections.push(Section {
        blocks: vec![
            Block::Table(Table::new(1, 1)),
            Block::Paragraph(Paragraph::from_text("Third")),
        ],
        page_style: landscape,
        header: None,
        footer: Some(Paragraph::empty()),
    });
    doc
}

fn assert_preserved(expected: &Document, actual: &Document) {
    assert_eq!(actual.sections.len(), expected.sections.len());
    for (index, (before, after)) in expected.sections.iter().zip(&actual.sections).enumerate() {
        for (before, after) in [
            (before.page_style.width, after.page_style.width),
            (before.page_style.height, after.page_style.height),
            (before.page_style.margin_top, after.page_style.margin_top),
            (
                before.page_style.margin_bottom,
                after.page_style.margin_bottom,
            ),
            (before.page_style.margin_left, after.page_style.margin_left),
            (
                before.page_style.margin_right,
                after.page_style.margin_right,
            ),
        ] {
            assert!(
                (before - after).abs() <= 0.03,
                "section {index}: {before} -> {after}"
            );
        }
        assert_eq!(
            before.blocks, after.blocks,
            "section {index} body or break changed"
        );
        assert_eq!(expected.section_header(index), actual.section_header(index));
        assert_eq!(expected.section_footer(index), actual.section_footer(index));
    }
}

#[test]
fn docx_mixed_paper_margins_headers_tables_and_breaks_survive_two_round_trips() {
    let original = document();
    let first = DocxFormat
        .load_from_bytes(&DocxFormat.save_to_bytes(&original).unwrap())
        .unwrap();
    assert_preserved(&original, &first);
    let second = DocxFormat
        .load_from_bytes(&DocxFormat.save_to_bytes(&first).unwrap())
        .unwrap();
    assert_preserved(&first, &second);
}

#[test]
fn odt_mixed_paper_margins_headers_tables_and_breaks_survive_two_round_trips() {
    let original = document();
    let first = OdtFormat
        .load_from_bytes(&OdtFormat.save_to_bytes(&original).unwrap())
        .unwrap();
    assert_preserved(&original, &first);
    let second = OdtFormat
        .load_from_bytes(&OdtFormat.save_to_bytes(&first).unwrap())
        .unwrap();
    assert_preserved(&first, &second);
}

#[test]
fn invalid_geometry_is_rejected_before_package_export() {
    let mut doc = document();
    doc.sections[2].page_style.margin_top = f32::NAN;
    assert!(DocxFormat.save_to_bytes(&doc).is_err());
    assert!(OdtFormat.save_to_bytes(&doc).is_err());
}

fn rewrite(bytes: &[u8], mut change: impl FnMut(&str, String) -> String) -> Vec<u8> {
    use std::io::{Cursor, Read, Write};
    let mut input = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut output = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for index in 0..input.len() {
        let mut file = input.by_index(index).unwrap();
        let name = file.name().to_owned();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        if name.ends_with(".xml") {
            bytes = change(&name, String::from_utf8(bytes).unwrap()).into_bytes();
        }
        output
            .start_file(name, zip::write::FileOptions::default())
            .unwrap();
        output.write_all(&bytes).unwrap();
    }
    output.finish().unwrap().into_inner()
}

#[test]
fn section_geometry_and_headers_resolve_namespace_aliases() {
    let original = document();
    let alias = |mut xml: String, aliases: &[(&str, &str)]| {
        for (before, after) in aliases {
            xml = xml
                .replace(&format!("xmlns:{before}="), &format!("xmlns:{after}="))
                .replace(&format!("<{before}:"), &format!("<{after}:"))
                .replace(&format!("</{before}:"), &format!("</{after}:"))
                .replace(&format!(" {before}:"), &format!(" {after}:"));
        }
        xml
    };
    let docx = rewrite(&DocxFormat.save_to_bytes(&original).unwrap(), |_, xml| {
        alias(xml, &[("w", "word"), ("r", "rel")])
    });
    assert_preserved(&original, &DocxFormat.load_from_bytes(&docx).unwrap());
    let odt = rewrite(&OdtFormat.save_to_bytes(&original).unwrap(), |_, xml| {
        alias(
            xml,
            &[("office", "o"), ("style", "s"), ("text", "t"), ("fo", "f")],
        )
    });
    assert_preserved(&original, &OdtFormat.load_from_bytes(&odt).unwrap());
}

#[test]
fn docx_omitted_margin_reference_inherits_previous_section_and_unsupported_breaks_fail() {
    let bytes = DocxFormat.save_to_bytes(&document()).unwrap();
    let inherited = rewrite(&bytes, |path, xml| {
        if path == "word/document.xml" {
            xml.replace(r#"<w:headerReference w:type="default" r:id="rId5"/>"#, "")
        } else {
            xml
        }
    });
    let doc = DocxFormat.load_from_bytes(&inherited).unwrap();
    assert_eq!(doc.section_header(2).unwrap().plain_text(), "Section two");
    for replacement in ["continuous", "evenPage", "oddPage"] {
        let bad = rewrite(&bytes, |path, xml| {
            if path == "word/document.xml" {
                xml.replace("w:val=\"nextPage\"", &format!("w:val=\"{replacement}\""))
            } else {
                xml
            }
        });
        assert!(DocxFormat.load_from_bytes(&bad).is_err());
    }
    let bad = rewrite(&bytes, |path, xml| {
        if path == "word/document.xml" {
            xml.replace("w:w=\"15840\"", "w:w=\"10\"")
        } else {
            xml
        }
    });
    assert!(DocxFormat.load_from_bytes(&bad).is_err());
}

#[test]
fn odt_invalid_layout_and_missing_master_references_reject_the_entire_package() {
    let bytes = OdtFormat.save_to_bytes(&document()).unwrap();
    for (part, before, after) in [
        (
            "styles.xml",
            "fo:page-width=\"792pt\"",
            "fo:page-width=\"nanpt\"",
        ),
        (
            "styles.xml",
            "style:page-layout-name=\"Mpm2\"",
            "style:page-layout-name=\"Missing\"",
        ),
        (
            "content.xml",
            "style:master-page-name=\"RustOfficeMaster2\"",
            "style:master-page-name=\"Missing\"",
        ),
    ] {
        let bad = rewrite(&bytes, |path, xml| {
            if path == part {
                xml.replace(before, after)
            } else {
                xml
            }
        });
        assert!(OdtFormat.load_from_bytes(&bad).is_err());
    }
}
