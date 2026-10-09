//! DOCX package writer (minimal OOXML).

use std::io::{Seek, Write};

use office_core::{
    extension_for_mime, mime_from_path, Alignment, Block, Document, Image, ImageSource, ListKind,
    NamedParagraphStyle, PageStyle, Paragraph, Table, TextStyle,
};
use zip::write::FileOptions;
use zip::{CompressionMethod, ZipWriter};

use crate::FormatError;

struct MediaFile {
    /// Package path relative to `word/` (e.g. `media/image1.png`).
    target: String,
    /// Full zip path (e.g. `word/media/image1.png`).
    zip_path: String,
    data: Vec<u8>,
    mime: String,
}

enum RelKind {
    Hyperlink(String),
    Image { target: String },
    Header(usize),
    Footer(usize),
}

pub fn write_docx_package<W: Write + Seek>(
    document: &Document,
    writer: W,
) -> Result<(), FormatError> {
    let package = build_package(document)?;

    let mut zip = ZipWriter::new(writer);
    let deflated = FileOptions::default().compression_method(CompressionMethod::Deflated);

    zip.start_file("[Content_Types].xml", deflated)?;
    zip.write_all(package.content_types.as_bytes())?;

    zip.start_file("_rels/.rels", deflated)?;
    zip.write_all(ROOT_RELS.as_bytes())?;

    zip.start_file("docProps/core.xml", deflated)?;
    zip.write_all(core_xml(document).as_bytes())?;

    zip.start_file("docProps/app.xml", deflated)?;
    zip.write_all(APP_XML.as_bytes())?;

    zip.start_file("word/document.xml", deflated)?;
    zip.write_all(package.document_xml.as_bytes())?;

    zip.start_file("word/_rels/document.xml.rels", deflated)?;
    zip.write_all(package.document_rels.as_bytes())?;

    for (index, header) in package.header_xml.iter().enumerate() {
        zip.start_file(format!("word/header{}.xml", index + 1), deflated)?;
        zip.write_all(header.as_bytes())?;
    }
    for (index, footer) in package.footer_xml.iter().enumerate() {
        zip.start_file(format!("word/footer{}.xml", index + 1), deflated)?;
        zip.write_all(footer.as_bytes())?;
    }

    for media in &package.media {
        let Some(media) = media else {
            continue;
        };
        zip.start_file(media.zip_path.as_str(), deflated)?;
        zip.write_all(&media.data)?;
    }

    zip.finish()?;
    Ok(())
}

/// Public helper used by tests / goldens.
pub fn build_document_xml(document: &Document) -> String {
    build_package(document)
        .expect("docx package build")
        .document_xml
}

struct Package {
    document_xml: String,
    document_rels: String,
    content_types: String,
    media: Vec<Option<MediaFile>>,
    header_xml: Vec<String>,
    footer_xml: Vec<String>,
}

fn build_package(document: &Document) -> Result<Package, FormatError> {
    let media = collect_media(document)?;
    let mut rels: Vec<RelKind> = Vec::new();
    let mut body = String::new();
    let mut image_i = 0usize;
    let mut header_xml = Vec::new();
    let mut footer_xml = Vec::new();
    if document.sections.is_empty() || document.sections.iter().any(|s| !s.page_style.is_valid()) {
        return Err(FormatError::InvalidDocument(
            "Invalid section page geometry".into(),
        ));
    }

    for (sec_i, section) in document.sections.iter().enumerate() {
        for block in &section.blocks {
            match block {
                Block::Paragraph(para) => append_paragraph(&mut body, para, &mut rels),
                Block::Table(table) => append_table(&mut body, table, &mut rels),
                Block::Image(image) => {
                    if let Some(Some(file)) = media.get(image_i) {
                        rels.push(RelKind::Image {
                            target: file.target.clone(),
                        });
                        let rid = rels.len();
                        append_image_paragraph(&mut body, image, rid);
                    } else {
                        let label = format!("[Image: {}]", image.display_name());
                        append_paragraph(&mut body, &Paragraph::from_text(label), &mut rels);
                    }
                    image_i += 1;
                }
                Block::PageBreak => {
                    body.push_str(r#"<w:p><w:r><w:br w:type="page"/></w:r></w:p>"#);
                }
            }
        }
        let blank = Paragraph::empty();
        let header = document
            .section_header(sec_i)
            .or_else(|| (sec_i > 0).then_some(&blank));
        let footer = document
            .section_footer(sec_i)
            .or_else(|| (sec_i > 0).then_some(&blank));
        let header_rid = header.map(|paragraph| {
            header_xml.push(hf_part_xml("hdr", paragraph));
            rels.push(RelKind::Header(header_xml.len()));
            rels.len()
        });
        let footer_rid = footer.map(|paragraph| {
            footer_xml.push(hf_part_xml("ftr", paragraph));
            rels.push(RelKind::Footer(footer_xml.len()));
            rels.len()
        });
        let properties = sect_pr(&section.page_style, header_rid, footer_rid);
        if sec_i + 1 == document.sections.len() {
            body.push_str(&properties);
        } else if matches!(section.blocks.last(), Some(Block::Paragraph(_))) {
            let at = body.rfind("</w:pPr>").expect("paragraph properties");
            body.insert_str(at, &properties);
        } else {
            body.push_str(&format!("<w:p><w:pPr><w:pStyle w:val=\"RustOfficeSectionBreak\"/>{properties}</w:pPr></w:p>"));
        }
    }

    let document_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture">
<w:body>
{body}</w:body>
</w:document>
"#
    );

    let document_rels = build_rels_xml(&rels);
    let content_types = build_content_types(&media, header_xml.len(), footer_xml.len());

    Ok(Package {
        document_xml,
        document_rels,
        content_types,
        media,
        header_xml,
        footer_xml,
    })
}

fn collect_media(document: &Document) -> Result<Vec<Option<MediaFile>>, FormatError> {
    let mut out = Vec::new();
    let mut idx = 0usize;
    for section in &document.sections {
        for block in &section.blocks {
            let Block::Image(image) = block else {
                continue;
            };
            idx += 1;
            match resolve_image_bytes(image) {
                Ok((mime, data)) => {
                    let ext = extension_for_mime(&mime);
                    let target = format!("media/image{idx}.{ext}");
                    let zip_path = format!("word/{target}");
                    out.push(Some(MediaFile {
                        target,
                        zip_path,
                        data,
                        mime,
                    }));
                }
                Err(_) => out.push(None),
            }
        }
    }
    Ok(out)
}

fn resolve_image_bytes(image: &Image) -> Result<(String, Vec<u8>), String> {
    match &image.source {
        ImageSource::Embedded { mime, data } => Ok((mime.clone(), data.clone())),
        ImageSource::Path { path } => {
            let data = std::fs::read(path).map_err(|e| format!("read {path}: {e}"))?;
            Ok((mime_from_path(path).to_string(), data))
        }
    }
}

fn build_rels_xml(rels: &[RelKind]) -> String {
    let mut out = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
"#,
    );
    for (i, rel) in rels.iter().enumerate() {
        let id = i + 1;
        match rel {
            RelKind::Hyperlink(url) => {
                out.push_str(&format!(
                    r#"<Relationship Id="rId{id}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="{}" TargetMode="External"/>
"#,
                    escape_xml(url)
                ));
            }
            RelKind::Image { target } => {
                out.push_str(&format!(
                    r#"<Relationship Id="rId{id}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="{}"/>
"#,
                    escape_xml(target)
                ));
            }
            RelKind::Header(index) => {
                out.push_str(&format!(
                    r#"<Relationship Id="rId{id}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/header" Target="header{index}.xml"/>
"#
                ));
            }
            RelKind::Footer(index) => {
                out.push_str(&format!(
                    r#"<Relationship Id="rId{id}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/footer" Target="footer{index}.xml"/>
"#
                ));
            }
        }
    }
    out.push_str("</Relationships>\n");
    out
}

fn build_content_types(media: &[Option<MediaFile>], headers: usize, footers: usize) -> String {
    let mut defaults = String::from(
        r#"  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
"#,
    );
    let mut seen_ext = std::collections::HashSet::new();
    for m in media.iter().flatten() {
        let ext = extension_for_mime(&m.mime);
        if seen_ext.insert(ext) {
            let ct = match ext {
                "jpg" => "image/jpeg",
                "gif" => "image/gif",
                "bmp" => "image/bmp",
                "webp" => "image/webp",
                _ => "image/png",
            };
            defaults.push_str(&format!(
                r#"  <Default Extension="{ext}" ContentType="{ct}"/>
"#
            ));
        }
    }

    let mut overrides = String::from(
        r#"  <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
  <Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/>
  <Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/>
"#,
    );
    for index in 1..=headers {
        overrides.push_str(&format!(r#"  <Override PartName="/word/header{index}.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml"/>
"#));
    }
    for index in 1..=footers {
        overrides.push_str(&format!(r#"  <Override PartName="/word/footer{index}.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml"/>
"#));
    }

    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
{defaults}{overrides}</Types>
"#
    )
}

fn sect_pr(page: &PageStyle, header_rid: Option<usize>, footer_rid: Option<usize>) -> String {
    // OOXML uses twentieths of a point (dxa): 1 pt = 20 dxa.
    let w = (page.width * 20.0).round() as i32;
    let h = (page.height * 20.0).round() as i32;
    let top = (page.margin_top * 20.0).round() as i32;
    let bottom = (page.margin_bottom * 20.0).round() as i32;
    let left = (page.margin_left * 20.0).round() as i32;
    let right = (page.margin_right * 20.0).round() as i32;
    let mut refs = String::new();
    if let Some(id) = header_rid {
        refs.push_str(&format!(
            r#"<w:headerReference w:type="default" r:id="rId{id}"/>"#
        ));
    }
    if let Some(id) = footer_rid {
        refs.push_str(&format!(
            r#"<w:footerReference w:type="default" r:id="rId{id}"/>"#
        ));
    }
    let orientation = if page.width > page.height {
        "landscape"
    } else {
        "portrait"
    };
    format!(
        r#"<w:sectPr>{refs}<w:type w:val="nextPage"/><w:pgSz w:w="{w}" w:h="{h}" w:orient="{orientation}"/><w:pgMar w:top="{top}" w:right="{right}" w:bottom="{bottom}" w:left="{left}" w:header="720" w:footer="720"/></w:sectPr>"#
    )
}

fn hf_part_xml(root: &str, para: &Paragraph) -> String {
    // root is "hdr" or "ftr"
    let mut inner = String::new();
    append_paragraph(&mut inner, para, &mut Vec::new());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:{root} xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
{inner}</w:{root}>
"#
    )
}

fn append_image_paragraph(body: &mut String, image: &Image, rid: usize) {
    let cx = pt_to_emu(image.width_pt);
    let cy = pt_to_emu(image.height_pt);
    let name = escape_xml(image.display_name());
    let doc_pr_id = rid; // unique-enough for MVP
    body.push_str(&format!(
        r#"<w:p><w:r><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="{cx}" cy="{cy}"/><wp:docPr id="{doc_pr_id}" name="{name}"/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic><pic:nvPicPr><pic:cNvPr id="0" name="{name}"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="rId{rid}"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#
    ));
}

fn pt_to_emu(pt: f32) -> i64 {
    (pt as f64 * 12700.0).round() as i64
}

fn append_paragraph(body: &mut String, para: &Paragraph, rels: &mut Vec<RelKind>) {
    body.push_str("<w:p>");
    body.push_str("<w:pPr>");
    if let Some(style_id) = docx_pstyle_id(para.style.named) {
        body.push_str(&format!(r#"<w:pStyle w:val="{style_id}"/>"#));
    }
    match para.style.alignment {
        Alignment::Left => {}
        Alignment::Center => body.push_str(r#"<w:jc w:val="center"/>"#),
        Alignment::Right => body.push_str(r#"<w:jc w:val="right"/>"#),
        Alignment::Justify => body.push_str(r#"<w:jc w:val="both"/>"#),
    }
    if let Some(list) = para.style.list {
        let indent = 720 * (list.level as i32 + 1);
        body.push_str(&format!(r#"<w:ind w:left="{indent}" w:hanging="360"/>"#));
    }
    body.push_str("</w:pPr>");

    if let Some(list) = para.style.list {
        let marker = match list.kind {
            ListKind::Bullet => "• ".to_string(),
            ListKind::Numbered => "1. ".to_string(),
        };
        append_run_text(body, &marker, &TextStyle::default(), None, rels);
    }

    if para.runs.is_empty() {
        body.push_str("<w:r><w:t></w:t></w:r>");
    } else {
        for run in &para.runs {
            append_run_text(body, &run.text, &run.style, run.link.as_deref(), rels);
        }
    }
    body.push_str("</w:p>");
}

fn docx_pstyle_id(named: NamedParagraphStyle) -> Option<&'static str> {
    match named {
        NamedParagraphStyle::Normal => None,
        NamedParagraphStyle::Heading1 => Some("Heading1"),
        NamedParagraphStyle::Heading2 => Some("Heading2"),
        NamedParagraphStyle::Heading3 => Some("Heading3"),
    }
}

fn append_run_text(
    body: &mut String,
    text: &str,
    style: &TextStyle,
    link: Option<&str>,
    rels: &mut Vec<RelKind>,
) {
    let mut rpr = String::new();
    if style.bold {
        rpr.push_str("<w:b/>");
    }
    if style.italic {
        rpr.push_str("<w:i/>");
    }
    if style.underline {
        rpr.push_str(r#"<w:u w:val="single"/>"#);
    }
    let half_points = (style.font_size * 2.0).round() as i32;
    if (style.font_size - 12.0).abs() > 0.01 {
        rpr.push_str(&format!(
            r#"<w:sz w:val="{half_points}"/><w:szCs w:val="{half_points}"/>"#
        ));
    }
    if link.is_some() {
        rpr.push_str(r#"<w:color w:val="0563C1"/><w:u w:val="single"/>"#);
    }

    let t = escape_xml(text);
    let xml_space = if text.starts_with(' ') || text.ends_with(' ') {
        r#" xml:space="preserve""#
    } else {
        ""
    };
    let run_xml = format!(
        "<w:r>{}<w:t{xml_space}>{t}</w:t></w:r>",
        if rpr.is_empty() {
            String::new()
        } else {
            format!("<w:rPr>{rpr}</w:rPr>")
        }
    );

    if let Some(url) = link {
        rels.push(RelKind::Hyperlink(url.to_string()));
        let id = rels.len();
        body.push_str(&format!(
            r#"<w:hyperlink r:id="rId{id}">{run_xml}</w:hyperlink>"#
        ));
    } else {
        body.push_str(&run_xml);
    }
}

fn append_table(body: &mut String, table: &Table, rels: &mut Vec<RelKind>) {
    body.push_str("<w:tbl>");
    body.push_str(
        r#"<w:tblPr><w:tblW w:w="0" w:type="auto"/><w:tblBorders>
<w:top w:val="single" w:sz="4" w:space="0" w:color="000000"/>
<w:left w:val="single" w:sz="4" w:space="0" w:color="000000"/>
<w:bottom w:val="single" w:sz="4" w:space="0" w:color="000000"/>
<w:right w:val="single" w:sz="4" w:space="0" w:color="000000"/>
<w:insideH w:val="single" w:sz="4" w:space="0" w:color="000000"/>
<w:insideV w:val="single" w:sz="4" w:space="0" w:color="000000"/>
</w:tblBorders></w:tblPr>"#,
    );
    for row in &table.rows {
        body.push_str("<w:tr>");
        for cell in &row.cells {
            body.push_str(r#"<w:tc><w:tcPr><w:tcW w:w="2000" w:type="dxa"/></w:tcPr>"#);
            if cell.paragraphs.is_empty() {
                body.push_str("<w:p/>");
            } else {
                for para in &cell.paragraphs {
                    append_paragraph(body, para, rels);
                }
            }
            body.push_str("</w:tc>");
        }
        body.push_str("</w:tr>");
    }
    body.push_str("</w:tbl>");
}

fn core_xml(document: &Document) -> String {
    let title = if document.title.is_empty() {
        "rust-office document"
    } else {
        document.title.as_str()
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:dcmitype="http://purl.org/dc/dcmitype/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
  <dc:title>{}</dc:title>
  <dc:creator>rust-office</dc:creator>
</cp:coreProperties>
"#,
        escape_xml(title)
    )
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

const ROOT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
  <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/>
  <Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties" Target="docProps/app.xml"/>
</Relationships>
"#;

const APP_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties">
  <Application>rust-office</Application>
</Properties>
"#;
