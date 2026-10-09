//! Parse word/document.xml into a Document.

use std::collections::HashMap;

use office_core::{
    apply_named_paragraph_defaults, mime_from_path, Alignment, Block, Document, Image, ListKind,
    ListStyle, NamedParagraphStyle, Paragraph, ParagraphStyle, Run, Section, Table, TableCell,
    TableRow, TextStyle,
};
use quick_xml::events::Event;
use quick_xml::name::QName;
use quick_xml::reader::NsReader;

use super::xml::{self, invalid, part_error, DRAWING_NS, PACKAGE_NS, REL_NS, WORD_NS};
use crate::FormatError;

/// Embedded image bytes keyed by relationship id (`rId1`, …).
pub type MediaMap = HashMap<String, MediaPart>;

/// One image part loaded from the DOCX package.
#[derive(Debug, Clone)]
pub struct MediaPart {
    pub mime: String,
    pub data: Vec<u8>,
}

/// Parse `word/document.xml` with empty media / no header-footer parts.
pub fn parse_document_xml(xml: &str, rels_xml: &str) -> Result<Document, FormatError> {
    parse_document_parts(xml, rels_xml, &MediaMap::new(), None, None)
}

/// Parse document body plus optional header/footer parts and media.
pub fn parse_document_parts(
    xml: &str,
    rels_xml: &str,
    media: &MediaMap,
    header_xml: Option<&str>,
    footer_xml: Option<&str>,
) -> Result<Document, FormatError> {
    let references = document_references(xml)?;
    let relationships = if rels_xml.is_empty() {
        HashMap::new()
    } else {
        parse_typed_rels(rels_xml)?
    };
    let mut hyperlinks = HashMap::new();
    // XML-only callers may deliberately omit package resources. File imports
    // resolve every reference and read the required parts before reaching here.
    if !rels_xml.is_empty() {
        for reference in &references {
            let relationship = resolve_reference(&relationships, reference)?;
            if reference.kind == ReferenceKind::Hyperlink {
                hyperlinks.insert(reference.id.clone(), relationship.target.clone());
            }
        }
    }
    let mut doc = parse_body(xml, &hyperlinks, media)?;
    if let Some(h) = header_xml
        .map(|xml| parse_hf_paragraph(xml, "hdr"))
        .transpose()?
        .flatten()
    {
        doc.main_section_mut().header = Some(h);
    }
    if let Some(f) = footer_xml
        .map(|xml| parse_hf_paragraph(xml, "ftr"))
        .transpose()?
        .flatten()
    {
        doc.main_section_mut().footer = Some(f);
    }
    Ok(doc)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReferenceKind {
    Image,
    Header,
    Footer,
    Hyperlink,
}

impl ReferenceKind {
    fn name(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Header => "header",
            Self::Footer => "footer",
            Self::Hyperlink => "hyperlink",
        }
    }
}

pub(super) struct Reference {
    pub id: String,
    pub kind: ReferenceKind,
}
pub(super) struct Relationship {
    pub target: String,
    kind: String,
    external: bool,
}
pub(super) type DocRels = HashMap<String, Relationship>;

pub(super) fn parse_typed_rels(xml_text: &str) -> Result<DocRels, FormatError> {
    let mut relationships = HashMap::new();
    xml::read_xml(
        xml_text,
        "Relationships",
        PACKAGE_NS,
        |event, reader, parents| {
            if let Event::Start(e) | Event::Empty(e) = event {
                if parents == ["Relationships"]
                    && xml::element(reader, e.name(), "Relationship", PACKAGE_NS)
                {
                    let id = xml::required_attr(e, "Id")?;
                    let external = match xml::attr(e, "TargetMode")?.as_deref() {
                        None | Some("Internal") => false,
                        Some("External") => true,
                        Some(_) => return Err(invalid("invalid relationship TargetMode")),
                    };
                    let relationship = Relationship {
                        target: xml::required_attr(e, "Target")?,
                        kind: xml::required_attr(e, "Type")?,
                        external,
                    };
                    if relationships.insert(id, relationship).is_some() {
                        return Err(invalid("duplicate relationship Id"));
                    }
                }
            }
            Ok(())
        },
    )
    .map_err(|e| part_error("word/_rels/document.xml.rels", e))?;
    Ok(relationships)
}

pub(super) fn document_references(text: &str) -> Result<Vec<Reference>, FormatError> {
    let mut references = Vec::new();
    let mut seen_body = false;
    let mut in_body = false;
    xml::read_xml(text, "document", WORD_NS, |event, reader, parents| {
        match event {
            Event::Start(e) | Event::Empty(e) => {
                if parents == ["document"] && xml::element(reader, e.name(), "body", WORD_NS) {
                    if seen_body {
                        return Err(invalid("duplicate document body"));
                    }
                    seen_body = true;
                    in_body = matches!(event, Event::Start(_));
                }
                if !in_body {
                    return Ok(());
                }
                let kind = if xml::element(reader, e.name(), "headerReference", WORD_NS) {
                    Some(ReferenceKind::Header)
                } else if xml::element(reader, e.name(), "footerReference", WORD_NS) {
                    Some(ReferenceKind::Footer)
                } else if xml::element(reader, e.name(), "hyperlink", WORD_NS) {
                    Some(ReferenceKind::Hyperlink)
                } else if xml::element(reader, e.name(), "blip", DRAWING_NS) {
                    Some(ReferenceKind::Image)
                } else {
                    None
                };
                if let Some(kind) = kind {
                    let key = if kind == ReferenceKind::Image {
                        "embed"
                    } else {
                        "id"
                    };
                    let id = xml::namespaced_attr(e, reader, key, REL_NS)?;
                    if kind == ReferenceKind::Image
                        && id.is_none()
                        && xml::namespaced_attr(e, reader, "link", REL_NS)?.is_some()
                    {
                        return Err(invalid("external linked images are unsupported"));
                    }
                    match id {
                        Some(id) if !id.is_empty() => references.push(Reference { id, kind }),
                        None if matches!(kind, ReferenceKind::Image | ReferenceKind::Hyperlink) => {
                        }
                        _ => {
                            return Err(invalid(format!("missing {} relationship id", kind.name())))
                        }
                    }
                }
            }
            Event::End(e)
                if parents == ["document", "body"]
                    && xml::element(reader, e.name(), "body", WORD_NS) =>
            {
                in_body = false
            }
            _ => {}
        }
        Ok(())
    })
    .map_err(|e| part_error("word/document.xml", e))?;
    if !seen_body {
        return Err(invalid("DOCX word/document.xml: missing document body"));
    }
    Ok(references)
}

pub(super) fn resolve_reference<'a>(
    relationships: &'a DocRels,
    reference: &Reference,
) -> Result<&'a Relationship, FormatError> {
    let relationship = relationships.get(&reference.id).ok_or_else(|| {
        invalid(format!(
            "DOCX word/_rels/document.xml.rels: missing {} relationship {}",
            reference.kind.name(),
            reference.id
        ))
    })?;
    if !REL_NS
        .iter()
        .any(|ns| relationship.kind == format!("{ns}/{}", reference.kind.name()))
    {
        return Err(invalid(format!(
            "DOCX word/_rels/document.xml.rels: {} is not a {} relationship",
            reference.id,
            reference.kind.name()
        )));
    }
    if reference.kind != ReferenceKind::Hyperlink && relationship.external {
        return Err(invalid(format!(
            "DOCX word/_rels/document.xml.rels: external {} {} is unsupported",
            reference.kind.name(),
            reference.id
        )));
    }
    Ok(relationship)
}

fn parse_body(
    xml: &str,
    hyperlinks: &HashMap<String, String>,
    media: &MediaMap,
) -> Result<Document, FormatError> {
    let mut reader = NsReader::from_str(xml);
    reader.config_mut().trim_text(false);

    let mut blocks: Vec<Block> = Vec::new();
    let mut sections = Vec::new();
    let mut properties = super::sections::read(xml)?.into_iter();
    let mut section_end = false;
    let mut section_marker = false;
    let finish_section =
        |blocks: &mut Vec<Block>,
         sections: &mut Vec<Section>,
         properties: &mut std::vec::IntoIter<super::sections::Properties>| {
            let props = properties.next().unwrap_or_default();
            let mut section = Section {
                blocks: std::mem::take(blocks),
                page_style: props.style,
                header: None,
                footer: None,
            };
            section.ensure_paragraph();
            sections.push(section);
        };
    let mut in_body = false;
    let mut in_p = false;
    let mut in_r = false;
    let mut in_t = false;
    let mut in_tbl = false;
    let mut in_tr = false;
    let mut in_tc = false;
    let mut in_hyperlink = false;
    let mut current_link: Option<String> = None;
    let mut page_break_pending = false;
    let mut pending_image: Option<String> = None;
    let mut pending_extent: Option<(f32, f32)> = None;

    let mut p_align = Alignment::Left;
    let mut p_indent_level: Option<u8> = None;
    let mut p_named = NamedParagraphStyle::Normal;
    let mut run_style = TextStyle::default();
    let mut run_buf = String::new();
    let mut para_runs: Vec<Run> = Vec::new();
    let mut table_rows: Vec<TableRow> = Vec::new();
    let mut row_cells: Vec<TableCell> = Vec::new();
    let mut cell_paragraphs: Vec<Paragraph> = Vec::new();
    let mut buf = Vec::new();

    let flush_run =
        |runs: &mut Vec<Run>, text: &mut String, style: &TextStyle, link: Option<&str>| {
            if text.is_empty() {
                return;
            }
            runs.push(
                Run::new(std::mem::take(text), style.clone()).with_link(link.map(str::to_string)),
            );
        };

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let local = local_name(e.name());
                match local.as_str() {
                    "body" => in_body = true,
                    "sectPr" if in_body && !in_tc => section_end = true,
                    "tbl" if in_body => {
                        in_tbl = true;
                        table_rows.clear();
                    }
                    "tr" if in_tbl => {
                        in_tr = true;
                        row_cells.clear();
                    }
                    "tc" if in_tr => {
                        in_tc = true;
                        cell_paragraphs.clear();
                    }
                    "p" if in_body => {
                        in_p = true;
                        p_align = Alignment::Left;
                        p_indent_level = None;
                        p_named = NamedParagraphStyle::Normal;
                        section_marker = false;
                        para_runs.clear();
                        page_break_pending = false;
                        pending_image = None;
                        pending_extent = None;
                    }
                    "pStyle" if in_p => {
                        if let Some(val) = attr(&e, "val") {
                            p_named = parse_docx_pstyle(&val);
                            section_marker = val == "RustOfficeSectionBreak";
                        }
                    }
                    "jc" if in_p => {
                        if let Some(val) = attr(&e, "val") {
                            p_align = parse_jc(&val);
                        }
                    }
                    "ind" if in_p => {
                        if let Some(left) = attr(&e, "left").and_then(|s| s.parse::<i32>().ok()) {
                            let level = ((left / 720).saturating_sub(1)).clamp(0, 8) as u8;
                            p_indent_level = Some(level);
                        }
                    }
                    "hyperlink" if in_p => {
                        in_hyperlink = true;
                        current_link = xml::namespaced_attr(&e, &reader, "id", REL_NS)?
                            .and_then(|id| hyperlinks.get(&id).cloned());
                    }
                    "r" if in_p => {
                        in_r = true;
                        run_style = TextStyle::default();
                        run_buf.clear();
                    }
                    "b" if in_r => run_style.bold = true,
                    "i" if in_r => run_style.italic = true,
                    "u" if in_r => run_style.underline = true,
                    "sz" if in_r => {
                        if let Some(v) = attr(&e, "val").and_then(|s| s.parse::<f32>().ok()) {
                            run_style.font_size = v / 2.0;
                        }
                    }
                    "t" if in_r => in_t = true,
                    "br" if in_r => {
                        if attr(&e, "type").as_deref() == Some("page") {
                            page_break_pending = true;
                        } else {
                            run_buf.push('\n');
                        }
                    }
                    "extent" if in_p => {
                        let cx = attr(&e, "cx").and_then(|s| s.parse::<f64>().ok());
                        let cy = attr(&e, "cy").and_then(|s| s.parse::<f64>().ok());
                        if let (Some(cx), Some(cy)) = (cx, cy) {
                            pending_extent = Some((emu_to_pt(cx), emu_to_pt(cy)));
                        }
                    }
                    "blip" if in_p => {
                        if let Some(id) = xml::namespaced_attr(&e, &reader, "embed", REL_NS)? {
                            pending_image = Some(id);
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(e)) => {
                let local = local_name(e.name());
                match local.as_str() {
                    "sectPr" if in_body && !in_tc => section_end = true,
                    "pStyle" if in_p => {
                        if let Some(val) = attr(&e, "val") {
                            p_named = parse_docx_pstyle(&val);
                            section_marker = val == "RustOfficeSectionBreak";
                        }
                    }
                    "jc" if in_p => {
                        if let Some(val) = attr(&e, "val") {
                            p_align = parse_jc(&val);
                        }
                    }
                    "ind" if in_p => {
                        if let Some(left) = attr(&e, "left").and_then(|s| s.parse::<i32>().ok()) {
                            let level = ((left / 720).saturating_sub(1)).clamp(0, 8) as u8;
                            p_indent_level = Some(level);
                        }
                    }
                    "b" if in_r => run_style.bold = true,
                    "i" if in_r => run_style.italic = true,
                    "u" if in_r => run_style.underline = true,
                    "sz" if in_r => {
                        if let Some(v) = attr(&e, "val").and_then(|s| s.parse::<f32>().ok()) {
                            run_style.font_size = v / 2.0;
                        }
                    }
                    "br" if in_r => {
                        if attr(&e, "type").as_deref() == Some("page") {
                            page_break_pending = true;
                        } else {
                            run_buf.push('\n');
                        }
                    }
                    "t" if in_r => {}
                    "extent" if in_p => {
                        let cx = attr(&e, "cx").and_then(|s| s.parse::<f64>().ok());
                        let cy = attr(&e, "cy").and_then(|s| s.parse::<f64>().ok());
                        if let (Some(cx), Some(cy)) = (cx, cy) {
                            pending_extent = Some((emu_to_pt(cx), emu_to_pt(cy)));
                        }
                    }
                    "blip" if in_p => {
                        if let Some(id) = xml::namespaced_attr(&e, &reader, "embed", REL_NS)? {
                            pending_image = Some(id);
                        }
                    }
                    "p" if in_body && !in_tbl => {
                        blocks.push(Block::Paragraph(Paragraph::empty()));
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(t)) => {
                if in_t {
                    let text = t
                        .unescape()
                        .map_err(|e| part_error("word/document.xml", invalid(e.to_string())))?;
                    run_buf.push_str(&text);
                }
            }
            Ok(Event::CData(text)) if in_t => {
                run_buf.push_str(
                    std::str::from_utf8(text.as_ref())
                        .map_err(|e| part_error("word/document.xml", invalid(e.to_string())))?,
                );
            }
            Ok(Event::End(e)) => {
                let local = local_name(e.name());
                match local.as_str() {
                    "t" => in_t = false,
                    "r" if in_r => {
                        flush_run(
                            &mut para_runs,
                            &mut run_buf,
                            &run_style,
                            if in_hyperlink {
                                current_link.as_deref()
                            } else {
                                None
                            },
                        );
                        in_r = false;
                    }
                    "hyperlink" => {
                        in_hyperlink = false;
                        current_link = None;
                    }
                    "p" if in_p => {
                        if let Some(rid) = pending_image.take() {
                            if !in_tc {
                                if let Some(part) = media.get(&rid) {
                                    let (w, h) = pending_extent.unwrap_or((120.0, 80.0));
                                    let img = Image::from_embedded(
                                        part.mime.clone(),
                                        part.data.clone(),
                                        "image",
                                        w,
                                        h,
                                    );
                                    blocks.push(Block::Image(img));
                                }
                            }
                            para_runs.clear();
                        } else if page_break_pending && para_runs.is_empty() {
                            if !in_tc {
                                blocks.push(Block::PageBreak);
                            }
                        } else {
                            let mut para =
                                finish_paragraph(para_runs, p_align, p_indent_level, p_named);
                            para_runs = Vec::new();
                            if in_tc {
                                cell_paragraphs.push(para);
                            } else {
                                detect_list_prefix(&mut para);
                                if !(section_marker && section_end && para.is_empty()) {
                                    blocks.push(Block::Paragraph(para));
                                }
                            }
                        }
                        if section_end && !in_tc {
                            finish_section(&mut blocks, &mut sections, &mut properties);
                            section_end = false;
                        }
                        in_p = false;
                        page_break_pending = false;
                        pending_extent = None;
                    }
                    "tc" if in_tc => {
                        if cell_paragraphs.is_empty() {
                            cell_paragraphs.push(Paragraph::empty());
                        }
                        row_cells.push(TableCell {
                            paragraphs: std::mem::take(&mut cell_paragraphs),
                        });
                        in_tc = false;
                    }
                    "tr" if in_tr => {
                        table_rows.push(TableRow {
                            cells: std::mem::take(&mut row_cells),
                        });
                        in_tr = false;
                    }
                    "tbl" if in_tbl => {
                        if !table_rows.is_empty() {
                            blocks.push(Block::Table(Table {
                                rows: std::mem::take(&mut table_rows),
                            }));
                        }
                        in_tbl = false;
                    }
                    "body" => in_body = false,
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(FormatError::InvalidDocument(format!("DOCX XML error: {e}")));
            }
            _ => {}
        }
        buf.clear();
    }

    if !blocks.is_empty() || sections.is_empty() || section_end {
        finish_section(&mut blocks, &mut sections, &mut properties);
    }
    let mut doc = Document::new();
    doc.sections = sections;
    Ok(doc)
}

pub(super) fn parse_hf_paragraph(text: &str, root: &str) -> Result<Option<Paragraph>, FormatError> {
    use quick_xml::{
        events::{BytesEnd, BytesStart},
        Writer,
    };
    let mut writer = Writer::new(Vec::new());
    let mut prefix = String::new();
    xml::read_xml(text, root, WORD_NS, |event, _reader, parents| {
        match event {
            Event::Start(e) if parents.is_empty() => {
                let mut index = 0;
                loop {
                    prefix = format!("rodoc{index}");
                    if e.try_get_attribute(format!("xmlns:{prefix}").as_str())
                        .map_err(|e| invalid(e.to_string()))?
                        .is_none()
                    {
                        break;
                    }
                    index += 1;
                }
                let mut start = e.clone().into_owned();
                let name = format!("{prefix}:document");
                start.set_name(name.as_bytes());
                let namespace = format!("xmlns:{prefix}");
                start.push_attribute((namespace.as_str(), WORD_NS[0]));
                writer.write_event(Event::Start(start))?;
                writer.write_event(Event::Start(BytesStart::new(format!("{prefix}:body"))))?;
            }
            Event::End(_) if parents.len() == 1 => {
                writer.write_event(Event::End(BytesEnd::new(format!("{prefix}:body"))))?;
                writer.write_event(Event::End(BytesEnd::new(format!("{prefix}:document"))))?;
            }
            Event::Empty(_) if parents.is_empty() => {}
            Event::Decl(_) => {}
            _ => {
                writer.write_event(event.clone())?;
            }
        }
        Ok(())
    })?;
    if prefix.is_empty() {
        return Ok(Some(Paragraph::empty()));
    }
    let content = String::from_utf8(writer.into_inner()).map_err(|e| invalid(e.to_string()))?;
    let document = parse_body(&content, &HashMap::new(), &MediaMap::new())?;
    let mut paragraphs = document
        .sections
        .into_iter()
        .flat_map(|s| s.blocks)
        .filter_map(|b| {
            if let Block::Paragraph(p) = b {
                Some(p)
            } else {
                None
            }
        });
    let mut paragraph = paragraphs.next().unwrap_or_else(Paragraph::empty);
    for next in paragraphs {
        paragraph.runs.push(Run::new("\n", TextStyle::default()));
        paragraph.runs.extend(next.runs);
    }
    Ok(Some(paragraph))
}

fn parse_jc(val: &str) -> Alignment {
    match val {
        "center" => Alignment::Center,
        "right" | "end" => Alignment::Right,
        "both" => Alignment::Justify,
        _ => Alignment::Left,
    }
}

fn emu_to_pt(emu: f64) -> f32 {
    (emu / 12700.0) as f32
}

fn finish_paragraph(
    runs: Vec<Run>,
    alignment: Alignment,
    indent_level: Option<u8>,
    named: NamedParagraphStyle,
) -> Paragraph {
    let mut para = if runs.is_empty() {
        Paragraph::empty()
    } else {
        Paragraph {
            runs,
            style: ParagraphStyle {
                alignment,
                list: indent_level.map(|level| ListStyle {
                    kind: ListKind::Bullet,
                    level,
                }),
                ..ParagraphStyle::default()
            },
        }
    };
    if named != NamedParagraphStyle::Normal {
        apply_named_paragraph_defaults(&mut para.style, named);
    }
    para.style.alignment = alignment;
    if let Some(level) = indent_level {
        if para.style.list.is_none() {
            para.style.list = Some(ListStyle {
                kind: ListKind::Bullet,
                level,
            });
        }
    }
    para.normalize();
    para
}

fn parse_docx_pstyle(val: &str) -> NamedParagraphStyle {
    match val {
        "Heading1" | "heading 1" | "Title" => NamedParagraphStyle::Heading1,
        "Heading2" | "heading 2" => NamedParagraphStyle::Heading2,
        "Heading3" | "heading 3" => NamedParagraphStyle::Heading3,
        _ => NamedParagraphStyle::Normal,
    }
}

fn detect_list_prefix(para: &mut Paragraph) {
    let text = para.plain_text();
    let (kind, prefix_len) = if text.starts_with("• ") {
        (ListKind::Bullet, 2usize)
    } else if text.starts_with("1. ") {
        (ListKind::Numbered, 3usize)
    } else {
        return;
    };
    let level = para.style.list.map(|l| l.level).unwrap_or(0);
    let mut remaining = prefix_len;
    let mut new_runs = Vec::new();
    for run in &para.runs {
        let chars: Vec<char> = run.text.chars().collect();
        if remaining >= chars.len() {
            remaining -= chars.len();
            continue;
        }
        let rest: String = chars[remaining..].iter().collect();
        remaining = 0;
        if !rest.is_empty() {
            new_runs.push(Run::new(rest, run.style.clone()).with_link(run.link.clone()));
        }
    }
    para.runs = new_runs;
    para.style.list = Some(ListStyle { kind, level });
    para.normalize();
}

fn local_name(name: QName<'_>) -> String {
    let raw = name.local_name();
    String::from_utf8_lossy(raw.as_ref()).into_owned()
}

fn attr(e: &quick_xml::events::BytesStart<'_>, key: &str) -> Option<String> {
    for a in e.attributes().flatten() {
        let k = String::from_utf8_lossy(a.key.as_ref()).into_owned();
        let local = k.rsplit(':').next().unwrap_or(&k);
        if local == key || k == key {
            return a.unescape_value().ok().map(|value| value.into_owned());
        }
    }
    None
}

/// Resolve a target URI relative to word/document.xml for ZIP lookup.
pub(super) fn word_part_path(target: &str) -> Result<String, FormatError> {
    if target.contains(['\\', '?', '#', ':']) {
        return Err(invalid(format!("invalid DOCX part target {target:?}")));
    }
    let mut decoded = Vec::new();
    let bytes = target.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes
                .get(i + 1..i + 3)
                .ok_or_else(|| invalid("invalid percent-encoded DOCX part target"))?;
            let hex = std::str::from_utf8(hex)
                .map_err(|_| invalid("invalid percent-encoded DOCX part target"))?;
            let byte = u8::from_str_radix(hex, 16)
                .map_err(|_| invalid("invalid percent-encoded DOCX part target"))?;
            if matches!(byte, b'/' | b'\\' | 0) {
                return Err(invalid("invalid encoded path separator"));
            }
            decoded.push(byte);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    let decoded =
        String::from_utf8(decoded).map_err(|_| invalid("DOCX part target is not UTF-8"))?;
    let mut parts = if decoded.starts_with('/') {
        Vec::new()
    } else {
        vec!["word"]
    };
    for part in decoded.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err(invalid("DOCX part target leaves package root"));
                }
            }
            part => parts.push(part),
        }
    }
    if parts.is_empty() {
        return Err(invalid("empty DOCX part target"));
    }
    Ok(parts.join("/"))
}

pub fn mime_for_media_path(path: &str) -> String {
    mime_from_path(path).to_string()
}
