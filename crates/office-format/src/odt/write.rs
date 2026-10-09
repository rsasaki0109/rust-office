//! ODT package writer.

use std::collections::HashMap;
use std::io::{Seek, Write};

use office_core::{
    extension_for_mime, mime_from_path, Alignment, Block, Document, Image, ImageSource, ListKind,
    NamedParagraphStyle, Paragraph, Run, Table, TextStyle,
};
use zip::write::FileOptions;
use zip::{CompressionMethod, ZipWriter};

use crate::FormatError;

const MIMETYPE: &str = "application/vnd.oasis.opendocument.text";

#[derive(Debug, Clone)]
pub struct PackageImage {
    pub href: String,
    pub mime: String,
    pub data: Vec<u8>,
}

pub fn write_odt_package<W: Write + Seek>(
    document: &Document,
    writer: W,
) -> Result<(), FormatError> {
    let (content_xml, pictures) = build_content_and_pictures(document)?;

    let mut zip = ZipWriter::new(writer);
    let stored = FileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = FileOptions::default().compression_method(CompressionMethod::Deflated);

    // mimetype must be first and uncompressed (ODF package rules).
    zip.start_file("mimetype", stored)?;
    zip.write_all(MIMETYPE.as_bytes())?;

    zip.start_file("META-INF/manifest.xml", deflated)?;
    zip.write_all(manifest_xml(&pictures).as_bytes())?;

    zip.start_file("content.xml", deflated)?;
    zip.write_all(content_xml.as_bytes())?;

    for pic in &pictures {
        zip.start_file(&pic.href, deflated)?;
        zip.write_all(&pic.data)?;
    }

    zip.start_file("styles.xml", deflated)?;
    zip.write_all(styles_xml(document).as_bytes())?;

    zip.start_file("meta.xml", deflated)?;
    zip.write_all(meta_xml(document).as_bytes())?;

    zip.finish()?;
    Ok(())
}

/// Build content.xml and the binary picture parts for the package.
pub fn build_content_and_pictures(
    document: &Document,
) -> Result<(String, Vec<PackageImage>), FormatError> {
    if document.sections.is_empty() || document.sections.iter().any(|s| !s.page_style.is_valid()) {
        return Err(FormatError::InvalidDocument(
            "Invalid section paper size or margins".into(),
        ));
    }
    let mut pictures = Vec::new();
    let mut image_hrefs: Vec<Option<String>> = Vec::new();

    for section in &document.sections {
        for block in &section.blocks {
            if let Block::Image(image) = block {
                match resolve_image_bytes(image) {
                    Ok((mime, data)) => {
                        let idx = pictures.len() + 1;
                        let ext = extension_for_mime(&mime);
                        let href = format!("Pictures/image{idx}.{ext}");
                        image_hrefs.push(Some(href.clone()));
                        pictures.push(PackageImage { href, mime, data });
                    }
                    Err(_) => {
                        image_hrefs.push(None);
                    }
                }
            }
        }
    }

    Ok((build_content_xml_with_images(document, &image_hrefs), pictures))
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

/// Backward-compatible content builder (images without package hrefs → placeholders).
pub fn build_content_xml(document: &Document) -> String {
    let hrefs: Vec<Option<String>> = document
        .sections
        .iter()
        .flat_map(|s| s.blocks.iter())
        .filter(|b| matches!(b, Block::Image(_)))
        .map(|_| None)
        .collect();
    build_content_xml_with_images(document, &hrefs)
}

fn build_content_xml_with_images(document: &Document, image_hrefs: &[Option<String>]) -> String {
    let mut text_styles: HashMap<TextStyleKey, String> = HashMap::new();
    let mut para_styles: HashMap<ParaStyleKey, String> = HashMap::new();
    let mut auto = String::new();

    let blocks: Vec<_> = document
        .sections
        .iter()
        .flat_map(|s| s.blocks.iter())
        .collect();

    for block in &blocks {
        match block {
            Block::Paragraph(para) => {
                ensure_para_style(&mut para_styles, &mut auto, para.style.alignment);
                for run in &para.runs {
                    ensure_text_style(&mut text_styles, &mut auto, &run.style);
                }
            }
            Block::Table(table) => {
                for row in &table.rows {
                    for cell in &row.cells {
                        for para in &cell.paragraphs {
                            ensure_para_style(&mut para_styles, &mut auto, para.style.alignment);
                            for run in &para.runs {
                                ensure_text_style(&mut text_styles, &mut auto, &run.style);
                            }
                        }
                    }
                }
            }
            Block::Image(_) => {}
            Block::PageBreak => {}
        }
    }

    ensure_list_styles(&mut auto);
    ensure_pagebreak_style(&mut auto);

    let mut body = String::new();
    let mut image_i = 0usize;
    for (section_index, section) in document.sections.iter().enumerate() {
        let blocks = &section.blocks;
        let start = body.len();
        let mut i = 0usize;
        while i < blocks.len() {
            match &blocks[i] {
                Block::Paragraph(para) if para.style.list.is_some() => {
                    let kind = para.style.list.unwrap().kind;
                    let level = para.style.list.unwrap().level;
                    i = append_nested_list(
                        &mut body,
                        blocks,
                        i,
                        level,
                        kind,
                        &para_styles,
                        &text_styles,
                    );
                }
                Block::Paragraph(para) => {
                    append_paragraph(&mut body, para, &para_styles, &text_styles);
                    i += 1;
                }
                Block::Table(table) => {
                    append_table(&mut body, table, &para_styles, &text_styles);
                    i += 1;
                }
                Block::Image(image) => {
                    let href = image_hrefs.get(image_i).and_then(|h| h.as_ref());
                    image_i += 1;
                    if let Some(href) = href {
                        append_image_frame(&mut body, image, href);
                    } else {
                        let label = format!("[Image: {}]", image.display_name());
                        let placeholder = Paragraph::from_text(label);
                        append_paragraph(&mut body, &placeholder, &para_styles, &text_styles);
                    }
                    i += 1;
                }
                Block::PageBreak => {
                    body.push_str("<text:p text:style-name=\"P_pagebreak\"/>");
                    i += 1;
                }
            }
        }

        let marker = !matches!(blocks.first(), Some(Block::Paragraph(_)));
        let alias = format!(
            "RustOfficeSection{}{}",
            if marker { "Marker" } else { "Start" },
            section_index + 1
        );
        let parent = if let Some(Block::Paragraph(p)) = blocks.first() {
            odt_paragraph_style_name(p, &para_styles)
        } else {
            "Standard".into()
        };
        let alignment = if let Some(Block::Paragraph(p)) = blocks.first() {
            p.style.alignment
        } else {
            Alignment::Left
        };
        let alignment = match alignment {
            Alignment::Center => "center",
            Alignment::Right => "end",
            Alignment::Justify => "justify",
            Alignment::Left => "start",
        };
        let break_before = if section_index > 0 {
            " fo:break-before=\"page\""
        } else {
            ""
        };
        auto.push_str(&format!("<style:style style:name=\"{alias}\" style:family=\"paragraph\" style:parent-style-name=\"{parent}\" style:master-page-name=\"{}\"><style:paragraph-properties fo:text-align=\"{alignment}\"{break_before}/></style:style>", master_name(section_index)));
        if marker {
            body.insert_str(start, &format!("<text:p text:style-name=\"{alias}\"/>"));
        } else {
            let paragraph_start = start + body[start..].find("<text:p ").expect("paragraph");
            let at = paragraph_start
                + body[paragraph_start..]
                    .find("text:style-name=\"")
                    .expect("paragraph style")
                + "text:style-name=\"".len();
            let end = at + body[at..].find('"').unwrap();
            body.replace_range(at..end, &alias);
        }
    }

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0" xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" office:version="1.3">
<office:automatic-styles>
{auto}</office:automatic-styles>
<office:body>
<office:text>
{body}</office:text>
</office:body>
</office:document-content>
"#
    )
}

fn append_nested_list(
    body: &mut String,
    blocks: &[Block],
    start: usize,
    level: u8,
    kind: ListKind,
    para_styles: &HashMap<ParaStyleKey, String>,
    text_styles: &HashMap<TextStyleKey, String>,
) -> usize {
    let list_style = match kind {
        ListKind::Bullet => "L_bullet",
        ListKind::Numbered => "L_number",
    };
    body.push_str(&format!(
        "<text:list text:style-name=\"{}\">",
        list_style
    ));
    let mut i = start;
    while i < blocks.len() {
        let Block::Paragraph(p) = &blocks[i] else {
            break;
        };
        let Some(ls) = p.style.list else {
            break;
        };
        if ls.kind != kind || ls.level < level {
            break;
        }
        if ls.level > level {
            i = append_nested_list(body, blocks, i, level + 1, kind, para_styles, text_styles);
            continue;
        }
        body.push_str("<text:list-item>");
        append_paragraph(body, p, para_styles, text_styles);
        i += 1;
        if i < blocks.len() {
            if let Block::Paragraph(child) = &blocks[i] {
                if let Some(cls) = child.style.list {
                    if cls.kind == kind && cls.level > level {
                        i = append_nested_list(
                            body,
                            blocks,
                            i,
                            level + 1,
                            kind,
                            para_styles,
                            text_styles,
                        );
                    }
                }
            }
        }
        body.push_str("</text:list-item>");
    }
    body.push_str("</text:list>");
    i
}

fn append_image_frame(body: &mut String, image: &Image, href: &str) {
    let w_cm = points_to_cm(image.width_pt);
    let h_cm = points_to_cm(image.height_pt);
    let name = escape_xml(image.display_name());
    let href = escape_xml(href);
    body.push_str(&format!(
        r#"<text:p><draw:frame draw:name="{name}" text:anchor-type="paragraph" svg:width="{w_cm}cm" svg:height="{h_cm}cm"><draw:image xlink:href="{href}" xlink:type="simple" xlink:show="embed" xlink:actuate="onLoad"/></draw:frame></text:p>"#
    ));
}

fn points_to_cm(pt: f32) -> f32 {
    pt * 2.54 / 72.0
}

fn append_paragraph(
    body: &mut String,
    para: &Paragraph,
    para_styles: &HashMap<ParaStyleKey, String>,
    text_styles: &HashMap<TextStyleKey, String>,
) {
    let p_name = odt_paragraph_style_name(para, para_styles);
    body.push_str(&format!(
        "<text:p text:style-name=\"{}\">",
        escape_xml(&p_name)
    ));
    for run in &para.runs {
        append_run(body, run, text_styles);
    }
    body.push_str("</text:p>");
}

fn odt_paragraph_style_name(
    para: &Paragraph,
    para_styles: &HashMap<ParaStyleKey, String>,
) -> String {
    match para.style.named {
        NamedParagraphStyle::Heading1 => "Heading1".into(),
        NamedParagraphStyle::Heading2 => "Heading2".into(),
        NamedParagraphStyle::Heading3 => "Heading3".into(),
        NamedParagraphStyle::Normal => para_styles
            .get(&ParaStyleKey {
                alignment: para.style.alignment,
            })
            .cloned()
            .unwrap_or_else(|| "Standard".into()),
    }
}

fn append_table(
    body: &mut String,
    table: &Table,
    para_styles: &HashMap<ParaStyleKey, String>,
    text_styles: &HashMap<TextStyleKey, String>,
) {
    body.push_str("<table:table>");
    for row in &table.rows {
        body.push_str("<table:table-row>");
        for cell in &row.cells {
            body.push_str("<table:table-cell office:value-type=\"string\">");
            if cell.paragraphs.is_empty() {
                body.push_str("<text:p/>");
            } else {
                for para in &cell.paragraphs {
                    append_paragraph(body, para, para_styles, text_styles);
                }
            }
            body.push_str("</table:table-cell>");
        }
        body.push_str("</table:table-row>");
    }
    body.push_str("</table:table>");
}

fn append_run(body: &mut String, run: &Run, text_styles: &HashMap<TextStyleKey, String>) {
    let key = TextStyleKey::from_style(&run.style);
    let needs_span =
        key.bold || key.italic || key.underline || (key.font_size_centi - 1200).abs() > 1;
    let text = escape_run_text(&run.text);
    let inner = if needs_span {
        let name = text_styles
            .get(&key)
            .map(String::as_str)
            .unwrap_or("T_plain");
        format!(
            "<text:span text:style-name=\"{}\">{}</text:span>",
            escape_xml(name),
            text
        )
    } else {
        text
    };
    if let Some(url) = &run.link {
        body.push_str(&format!(
            "<text:a xlink:href=\"{}\" xlink:type=\"simple\">{}</text:a>",
            escape_xml(url),
            inner
        ));
    } else {
        body.push_str(&inner);
    }
}


fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

fn escape_run_text(s: &str) -> String {
    let mut out = String::new();
    let mut space_run = 0usize;
    let flush_spaces = |n: usize, out: &mut String| {
        if n == 1 {
            out.push(' ');
        } else if n > 1 {
            out.push_str(&format!("<text:s text:c=\"{n}\"/>"));
        }
    };
    for c in s.chars() {
        if c == ' ' {
            space_run += 1;
            continue;
        }
        flush_spaces(space_run, &mut out);
        space_run = 0;
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\t' => out.push_str("<text:tab/>"),
            '\n' => out.push_str("<text:line-break/>"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    flush_spaces(space_run, &mut out);
    out
}

fn fo_align(alignment: Alignment) -> &'static str {
    match alignment {
        Alignment::Left => "start",
        Alignment::Center => "center",
        Alignment::Right => "end",
        Alignment::Justify => "justify",
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TextStyleKey {
    bold: bool,
    italic: bool,
    underline: bool,
    font_size_centi: i32,
}

impl TextStyleKey {
    fn from_style(style: &TextStyle) -> Self {
        Self {
            bold: style.bold,
            italic: style.italic,
            underline: style.underline,
            font_size_centi: (style.font_size * 100.0).round() as i32,
        }
    }

    fn font_size(&self) -> f32 {
        self.font_size_centi as f32 / 100.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ParaStyleKey {
    alignment: Alignment,
}

fn ensure_list_styles(auto: &mut String) {
    if auto.contains("L_bullet") {
        return;
    }
    let mut bullet_levels = String::new();
    let mut number_levels = String::new();
    const BULLETS: &[&str] = &["•", "○", "■", "–"];
    for level in 1..=9 {
        let space = 0.25 * level as f32;
        let bullet = BULLETS[(level - 1) % BULLETS.len()];
        bullet_levels.push_str(&format!(
            r#"<text:list-level-style-bullet text:level="{level}" text:bullet-char="{bullet}"><style:list-level-properties text:space-before="{space}in" text:min-label-width="0.25in"/></text:list-level-style-bullet>"#
        ));
        number_levels.push_str(&format!(
            r#"<text:list-level-style-number text:level="{level}" style:num-format="1" style:num-suffix="."><style:list-level-properties text:space-before="{space}in" text:min-label-width="0.25in"/></text:list-level-style-number>"#
        ));
    }
    auto.push_str(&format!(
        r#"<text:list-style style:name="L_bullet">{bullet_levels}</text:list-style>
<text:list-style style:name="L_number">{number_levels}</text:list-style>
"#
    ));
}

fn ensure_pagebreak_style(auto: &mut String) {
    if auto.contains("P_pagebreak") {
        return;
    }
    auto.push_str(
        r#"<style:style style:name="P_pagebreak" style:family="paragraph"><style:paragraph-properties fo:break-before="page"/></style:style>
"#,
    );
}

fn ensure_text_style(
    map: &mut HashMap<TextStyleKey, String>,
    auto: &mut String,
    style: &TextStyle,
) {
    let key = TextStyleKey::from_style(style);
    if map.contains_key(&key) {
        return;
    }
    let name = format!("T{}", map.len() + 1);
    let mut props = String::new();
    if key.bold {
        props.push_str(r#" fo:font-weight="bold" style:font-weight-asian="bold""#);
    }
    if key.italic {
        props.push_str(r#" fo:font-style="italic" style:font-style-asian="italic""#);
    }
    if key.underline {
        props.push_str(
            r#" style:text-underline-style="solid" style:text-underline-type="single""#,
        );
    }
    let size = key.font_size();
    if (size - 12.0).abs() > 0.01 {
        props.push_str(&format!(
            r#" fo:font-size="{size}pt" style:font-size-asian="{size}pt""#
        ));
    }
    auto.push_str(&format!(
        r#"<style:style style:name="{name}" style:family="text"><style:text-properties{props}/></style:style>
"#
    ));
    map.insert(key, name);
}

fn ensure_para_style(
    map: &mut HashMap<ParaStyleKey, String>,
    auto: &mut String,
    alignment: Alignment,
) {
    let key = ParaStyleKey { alignment };
    if map.contains_key(&key) {
        return;
    }
    let name = format!("P{}", map.len() + 1);
    let align = fo_align(alignment);
    auto.push_str(&format!(
        r#"<style:style style:name="{name}" style:family="paragraph"><style:paragraph-properties fo:text-align="{align}"/></style:style>
"#
    ));
    map.insert(key, name);
}

fn manifest_xml(pictures: &[PackageImage]) -> String {
    let mut entries = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.3">
 <manifest:file-entry manifest:full-path="/" manifest:version="1.3" manifest:media-type="application/vnd.oasis.opendocument.text"/>
 <manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/>
 <manifest:file-entry manifest:full-path="styles.xml" manifest:media-type="text/xml"/>
 <manifest:file-entry manifest:full-path="meta.xml" manifest:media-type="text/xml"/>
"#,
    );
    if !pictures.is_empty() {
        entries.push_str(
            r#" <manifest:file-entry manifest:full-path="Pictures/" manifest:media-type="application/vnd.oasis.opendocument.image"/>
"#,
        );
    }
    for pic in pictures {
        entries.push_str(&format!(
            r#" <manifest:file-entry manifest:full-path="{}" manifest:media-type="{}"/>
"#,
            escape_xml(&pic.href),
            escape_xml(&pic.mime)
        ));
    }
    entries.push_str("</manifest:manifest>\n");
    entries
}

fn master_name(index: usize) -> String {
    if index == 0 {
        "Standard".into()
    } else {
        format!("RustOfficeMaster{}", index + 1)
    }
}

fn styles_xml(document: &Document) -> String {
    let mut automatic = String::new();
    let mut masters = String::new();
    let mut text_styles = HashMap::new();
    let mut para_styles = HashMap::new();
    for (index, section) in document.sections.iter().enumerate() {
        let page = &section.page_style;
        let layout = format!("Mpm{}", index + 1);
        let orientation = if page.width > page.height {
            "landscape"
        } else {
            "portrait"
        };
        automatic.push_str(&format!("<style:page-layout style:name=\"{layout}\"><style:page-layout-properties fo:page-width=\"{}pt\" fo:page-height=\"{}pt\" fo:margin-top=\"{}pt\" fo:margin-bottom=\"{}pt\" fo:margin-left=\"{}pt\" fo:margin-right=\"{}pt\" style:print-orientation=\"{orientation}\"/></style:page-layout>", page.width, page.height, page.margin_top, page.margin_bottom, page.margin_left, page.margin_right));
        masters.push_str(&format!(
            "<style:master-page style:name=\"{}\" style:page-layout-name=\"{layout}\">",
            master_name(index)
        ));
        for (tag, paragraph) in [
            ("header", document.section_header(index)),
            ("footer", document.section_footer(index)),
        ] {
            if let Some(paragraph) = paragraph {
                ensure_para_style(&mut para_styles, &mut automatic, paragraph.style.alignment);
                for run in &paragraph.runs {
                    ensure_text_style(&mut text_styles, &mut automatic, &run.style);
                }
                masters.push_str(&format!("<style:{tag}>"));
                append_paragraph(&mut masters, paragraph, &para_styles, &text_styles);
                masters.push_str(&format!("</style:{tag}>"));
            }
        }
        masters.push_str("</style:master-page>");
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-styles xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" office:version="1.3">
 <office:styles>
  <style:style style:name="Standard" style:family="paragraph" style:class="text"/>
  <style:style style:name="Heading1" style:family="paragraph" style:parent-style-name="Standard" style:class="text" style:default-outline-level="1"><style:paragraph-properties fo:margin-bottom="0.35cm"/><style:text-properties fo:font-size="20pt" fo:font-weight="bold"/></style:style>
  <style:style style:name="Heading2" style:family="paragraph" style:parent-style-name="Standard" style:class="text" style:default-outline-level="2"><style:paragraph-properties fo:margin-bottom="0.25cm"/><style:text-properties fo:font-size="16pt" fo:font-weight="bold"/></style:style>
  <style:style style:name="Heading3" style:family="paragraph" style:parent-style-name="Standard" style:class="text" style:default-outline-level="3"><style:paragraph-properties fo:margin-bottom="0.2cm"/><style:text-properties fo:font-size="14pt" fo:font-weight="bold"/></style:style>
 </office:styles>
 <office:automatic-styles>{automatic}</office:automatic-styles>
 <office:master-styles>{masters}</office:master-styles>
</office:document-styles>
"#
    )
}

fn meta_xml(document: &Document) -> String {
    let title = if document.title.is_empty() {
        "rust-office document"
    } else {
        document.title.as_str()
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-meta xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:meta="urn:oasis:names:tc:opendocument:xmlns:meta:1.0" office:version="1.3">
 <office:meta>
  <dc:title>{}</dc:title>
  <meta:generator>rust-office</meta:generator>
 </office:meta>
</office:document-meta>
"#,
        escape_xml(title)
    )
}
