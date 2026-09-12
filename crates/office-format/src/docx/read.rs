//! Parse word/document.xml into a Document.

use std::collections::HashMap;

use office_core::{
    apply_named_paragraph_defaults, mime_from_path, Alignment, Block, Document, Image, ListKind,
    ListStyle, NamedParagraphStyle, Paragraph, ParagraphStyle, Run, Table, TableCell, TableRow,
    TextStyle,
};
use quick_xml::events::Event;
use quick_xml::name::QName;
use quick_xml::reader::Reader;

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
    let hyperlinks = parse_typed_rels(rels_xml).hyperlinks;
    let mut doc = parse_body(xml, &hyperlinks, media)?;
    if let Some(h) = header_xml.and_then(parse_hf_paragraph) {
        doc.main_section_mut().header = Some(h);
    }
    if let Some(f) = footer_xml.and_then(parse_hf_paragraph) {
        doc.main_section_mut().footer = Some(f);
    }
    Ok(doc)
}

/// Relationship targets discovered in `document.xml.rels`.
#[derive(Debug, Default)]
pub struct DocRels {
    pub hyperlinks: HashMap<String, String>,
    pub images: HashMap<String, String>,
    pub header: Option<String>,
    pub footer: Option<String>,
}

pub fn parse_typed_rels(rels_xml: &str) -> DocRels {
    let mut out = DocRels::default();
    if rels_xml.is_empty() {
        return out;
    }
    let mut reader = Reader::from_str(rels_xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(e)) | Ok(Event::Start(e)) => {
                if local_name(e.name()) == "Relationship" {
                    let id = attr(&e, "Id");
                    let ty = attr(&e, "Type").unwrap_or_default();
                    let target = attr(&e, "Target");
                    if let (Some(id), Some(target)) = (id, target) {
                        if ty.contains("/hyperlink") {
                            out.hyperlinks.insert(id, target);
                        } else if ty.contains("/image") {
                            out.images.insert(id, target);
                        } else if ty.contains("/header") {
                            out.header = Some(target);
                        } else if ty.contains("/footer") {
                            out.footer = Some(target);
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    out
}

fn parse_body(
    xml: &str,
    hyperlinks: &HashMap<String, String>,
    media: &MediaMap,
) -> Result<Document, FormatError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);

    let mut blocks: Vec<Block> = Vec::new();
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

    let flush_run = |runs: &mut Vec<Run>,
                     text: &mut String,
                     style: &TextStyle,
                     link: Option<&str>| {
        if text.is_empty() {
            return;
        }
        runs.push(Run::new(std::mem::take(text), style.clone()).with_link(link.map(str::to_string)));
    };

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let local = local_name(e.name());
                match local.as_str() {
                    "body" => in_body = true,
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
                        para_runs.clear();
                        page_break_pending = false;
                        pending_image = None;
                        pending_extent = None;
                    }
                    "pStyle" if in_p => {
                        if let Some(val) = attr(&e, "val") {
                            p_named = parse_docx_pstyle(&val);
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
                        current_link = attr(&e, "id").and_then(|id| hyperlinks.get(&id).cloned());
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
                        if let Some(id) = attr(&e, "embed") {
                            pending_image = Some(id);
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(e)) => {
                let local = local_name(e.name());
                match local.as_str() {
                    "pStyle" if in_p => {
                        if let Some(val) = attr(&e, "val") {
                            p_named = parse_docx_pstyle(&val);
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
                        if let Some(id) = attr(&e, "embed") {
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
                    let text = t.unescape().unwrap_or_default();
                    run_buf.push_str(&text);
                }
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
                                blocks.push(Block::Paragraph(para));
                            }
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

    if blocks.is_empty() {
        blocks.push(Block::Paragraph(Paragraph::empty()));
    }
    let mut doc = Document::new();
    doc.sections[0].blocks = blocks;
    Ok(doc)
}

fn parse_hf_paragraph(xml: &str) -> Option<Paragraph> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut in_t = false;
    let mut text = String::new();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                if local_name(e.name()) == "t" {
                    in_t = true;
                }
            }
            Ok(Event::Empty(e)) => {
                if local_name(e.name()) == "t" {
                    // empty text node
                }
            }
            Ok(Event::Text(t)) if in_t => {
                text.push_str(&t.unescape().unwrap_or_default());
            }
            Ok(Event::End(e)) => {
                if local_name(e.name()) == "t" {
                    in_t = false;
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    if text.is_empty() {
        None
    } else {
        Some(Paragraph::from_text(text))
    }
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
            return Some(String::from_utf8_lossy(&a.value).into_owned());
        }
    }
    None
}

/// Resolve a relationship Target to a zip path under the package.
pub fn word_part_path(target: &str) -> String {
    let t = target.replace('\\', "/");
    if t.starts_with("word/") || t.starts_with("/word/") {
        t.trim_start_matches('/').to_string()
    } else if let Some(rest) = t.strip_prefix('/') {
        format!("word/{rest}")
    } else {
        format!("word/{t}")
    }
}

pub fn mime_for_media_path(path: &str) -> String {
    mime_from_path(path).to_string()
}
