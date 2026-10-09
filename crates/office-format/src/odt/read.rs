//! ODT content.xml reader (paragraphs, tables, styled spans).

use std::collections::HashMap;

use office_core::{
    apply_named_paragraph_defaults, mime_from_path, Alignment, Block, Document, Image, ListKind,
    ListStyle, NamedParagraphStyle, Paragraph, ParagraphStyle, Run, Table, TableCell, TableRow,
    TextStyle,
};
use quick_xml::events::Event;
use quick_xml::name::QName;
use quick_xml::reader::NsReader;

use super::xml::{self, content_images, image_part_path, part_error, XLINK_NS};

use crate::FormatError;

#[derive(Debug, Default, Clone)]
struct TextProps {
    bold: bool,
    italic: bool,
    underline: bool,
    font_size: Option<f32>,
}

#[derive(Debug, Default, Clone)]
struct ParaProps {
    alignment: Alignment,
    break_before: bool,
}

/// Parse `content.xml` into a [`Document`] (no package pictures).
pub fn parse_content_xml(xml: &str) -> Result<Document, FormatError> {
    parse_content_xml_with_pictures(xml, &HashMap::new())
}

/// Parse `content.xml`, resolving `draw:image` hrefs via `pictures` (keys like `Pictures/a.png`).
pub fn parse_content_xml_with_pictures(
    xml: &str,
    pictures: &HashMap<String, Vec<u8>>,
) -> Result<Document, FormatError> {
    // XML-only callers may omit images. The package loader reads all actual
    // references before calling this parser.
    content_images(xml)?;
    let mut reader = NsReader::from_str(xml);
    reader.config_mut().trim_text(false);

    let mut text_styles: HashMap<String, TextProps> = HashMap::new();
    let mut para_styles: HashMap<String, ParaProps> = HashMap::new();

    let mut blocks: Vec<Block> = Vec::new();
    let mut in_automatic_styles = false;
    let mut current_style_name: Option<String> = None;
    let mut current_style_family: Option<String> = None;

    let mut in_text = false;
    let mut in_paragraph = false;
    let mut in_table = false;
    let mut in_row = false;
    let mut in_cell = false;
    let mut in_frame = false;
    let mut frame_width_pt = 240.0_f32;
    let mut frame_height_pt = 160.0_f32;
    let mut frame_name = String::from("image");
    let mut frame_href: Option<String> = None;
    let mut paragraph_has_image = false;
    let mut saw_soft_page_break = false;
    let mut list_stack: Vec<ListKind> = Vec::new();
    let mut current_link: Option<String> = None;
    let mut current_para_style: Option<String> = None;
    let mut current_span_style: Option<String> = None;
    let mut current_runs: Vec<Run> = Vec::new();
    let mut table_rows: Vec<TableRow> = Vec::new();
    let mut row_cells: Vec<TableCell> = Vec::new();
    let mut cell_paragraphs: Vec<Paragraph> = Vec::new();
    let mut buf = Vec::new();

    let push_finished_para = |runs: Vec<Run>,
                              style_name: Option<&str>,
                              para_styles: &HashMap<String, ParaProps>,
                              in_cell: bool,
                              cell_paragraphs: &mut Vec<Paragraph>,
                              blocks: &mut Vec<Block>,
                              list: Option<ListStyle>| {
        let mut para = make_paragraph(runs, style_name, para_styles);
        para.style.list = list;
        if in_cell {
            cell_paragraphs.push(para);
        } else {
            blocks.push(Block::Paragraph(para));
        }
    };

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let local = local_name(e.name());
                match local.as_str() {
                    "automatic-styles" => in_automatic_styles = true,
                    "style" if in_automatic_styles => {
                        current_style_name = attr(&e, "name");
                        current_style_family = attr(&e, "family");
                    }
                    "text-properties" if in_automatic_styles => {
                        if let (Some(name), Some(family)) =
                            (current_style_name.clone(), current_style_family.clone())
                        {
                            if family == "text" {
                                text_styles.insert(name, parse_text_props(&e));
                            }
                        }
                    }
                    "paragraph-properties" if in_automatic_styles => {
                        if let (Some(name), Some(family)) =
                            (current_style_name.clone(), current_style_family.clone())
                        {
                            if family == "paragraph" {
                                para_styles.insert(name, parse_para_props(&e));
                            }
                        }
                    }
                    "text" if !in_automatic_styles => in_text = true,
                    "list" if in_text => {
                        let kind = list_kind_from_style(attr(&e, "style-name").as_deref());
                        list_stack.push(kind);
                    }
                    "a" if in_paragraph => {
                        current_link = xml::namespaced_attr(&e, &reader, "href", XLINK_NS)?;
                    }
                    "table" if in_text => {
                        in_table = true;
                        table_rows.clear();
                    }
                    "table-row" if in_table => {
                        in_row = true;
                        row_cells.clear();
                    }
                    "table-cell" if in_row => {
                        in_cell = true;
                        cell_paragraphs.clear();
                    }
                    "p" if in_text => {
                        in_paragraph = true;
                        current_para_style = attr(&e, "style-name");
                        current_runs.clear();
                        current_span_style = None;
                        paragraph_has_image = false;
                        saw_soft_page_break = false;
                    }
                    "frame" if in_text => {
                        in_frame = true;
                        frame_href = None;
                        frame_name = attr(&e, "name").unwrap_or_else(|| "image".into());
                        if let Some(w) = attr(&e, "width").and_then(|s| parse_length_pt(&s)) {
                            frame_width_pt = w;
                        }
                        if let Some(h) = attr(&e, "height").and_then(|s| parse_length_pt(&s)) {
                            frame_height_pt = h;
                        }
                    }
                    "image" if in_frame => {
                        if let Some(href) = xml::namespaced_attr(&e, &reader, "href", XLINK_NS)? {
                            frame_href = Some(href);
                        }
                    }
                    "span" if in_paragraph => {
                        current_span_style = attr(&e, "style-name");
                    }
                    "s" if in_paragraph => {
                        let count = xml::space_count(&e, &reader)?;
                        push_text(
                            &mut current_runs,
                            &" ".repeat(count),
                            current_span_style.as_deref(),
                            &text_styles,
                            current_link.as_deref(),
                        );
                    }
                    "tab" if in_paragraph => {
                        push_text(
                            &mut current_runs,
                            "\t",
                            current_span_style.as_deref(),
                            &text_styles,
                            current_link.as_deref(),
                        );
                    }
                    "line-break" if in_paragraph => {
                        push_text(
                            &mut current_runs,
                            "\n",
                            current_span_style.as_deref(),
                            &text_styles,
                            current_link.as_deref(),
                        );
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(e)) => {
                let local = local_name(e.name());
                match local.as_str() {
                    "text-properties" if in_automatic_styles => {
                        if let (Some(name), Some(family)) =
                            (current_style_name.clone(), current_style_family.clone())
                        {
                            if family == "text" {
                                text_styles.insert(name, parse_text_props(&e));
                            }
                        }
                    }
                    "paragraph-properties" if in_automatic_styles => {
                        if let (Some(name), Some(family)) =
                            (current_style_name.clone(), current_style_family.clone())
                        {
                            if family == "paragraph" {
                                para_styles.insert(name, parse_para_props(&e));
                            }
                        }
                    }
                    "s" if in_paragraph => {
                        let count = xml::space_count(&e, &reader)?;
                        push_text(
                            &mut current_runs,
                            &" ".repeat(count),
                            current_span_style.as_deref(),
                            &text_styles,
                            current_link.as_deref(),
                        );
                    }
                    "tab" if in_paragraph => {
                        push_text(
                            &mut current_runs,
                            "\t",
                            current_span_style.as_deref(),
                            &text_styles,
                            current_link.as_deref(),
                        );
                    }
                    "line-break" if in_paragraph => {
                        push_text(
                            &mut current_runs,
                            "\n",
                            current_span_style.as_deref(),
                            &text_styles,
                            current_link.as_deref(),
                        );
                    }
                    "soft-page-break" if in_paragraph => {
                        saw_soft_page_break = true;
                    }
                    "image" if in_frame => {
                        if let Some(href) = xml::namespaced_attr(&e, &reader, "href", XLINK_NS)? {
                            frame_href = Some(href);
                        }
                    }
                    "frame" if in_text => {
                        // Self-closing frame: capture and emit immediately.
                        frame_href = None;
                        frame_name = attr(&e, "name").unwrap_or_else(|| "image".into());
                        if let Some(w) = attr(&e, "width").and_then(|s| parse_length_pt(&s)) {
                            frame_width_pt = w;
                        }
                        if let Some(h) = attr(&e, "height").and_then(|s| parse_length_pt(&s)) {
                            frame_height_pt = h;
                        }
                        if let Some(img) = finish_frame_image(
                            &frame_href,
                            &frame_name,
                            frame_width_pt,
                            frame_height_pt,
                            pictures,
                        ) {
                            if !in_cell {
                                blocks.push(Block::Image(img));
                                paragraph_has_image = true;
                            }
                        }
                    }
                    "p" if in_text => {
                        let style_name = attr(&e, "style-name");
                        if !in_cell && is_pagebreak_style(style_name.as_deref(), &para_styles) {
                            blocks.push(Block::PageBreak);
                        } else {
                            push_finished_para(
                                Vec::new(),
                                style_name.as_deref(),
                                &para_styles,
                                in_cell,
                                &mut cell_paragraphs,
                                &mut blocks,
                                list_stack.last().map(|k| ListStyle {
                                    kind: *k,
                                    level: list_stack.len().saturating_sub(1) as u8,
                                }),
                            );
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(t)) => {
                if in_paragraph {
                    let text = t
                        .unescape()
                        .map_err(|e| part_error("content.xml", xml::invalid(e.to_string())))?;
                    if !text.is_empty() {
                        push_text(
                            &mut current_runs,
                            &text,
                            current_span_style.as_deref(),
                            &text_styles,
                            current_link.as_deref(),
                        );
                    }
                }
            }
            Ok(Event::CData(t)) if in_paragraph => {
                let text = std::str::from_utf8(t.as_ref())
                    .map_err(|e| part_error("content.xml", xml::invalid(e.to_string())))?;
                push_text(
                    &mut current_runs,
                    text,
                    current_span_style.as_deref(),
                    &text_styles,
                    current_link.as_deref(),
                );
            }
            Ok(Event::End(e)) => {
                let local = local_name(e.name());
                match local.as_str() {
                    "automatic-styles" => in_automatic_styles = false,
                    "style" => {
                        current_style_name = None;
                        current_style_family = None;
                    }
                    "span" => current_span_style = None,
                    "a" => current_link = None,
                    "list" => {
                        list_stack.pop();
                    }
                    "frame" if in_frame => {
                        if let Some(img) = finish_frame_image(
                            &frame_href,
                            &frame_name,
                            frame_width_pt,
                            frame_height_pt,
                            pictures,
                        ) {
                            if !in_cell {
                                blocks.push(Block::Image(img));
                                paragraph_has_image = true;
                            }
                        }
                        in_frame = false;
                        frame_href = None;
                    }
                    "p" if in_paragraph => {
                        let runs = std::mem::take(&mut current_runs);
                        let only_image = paragraph_has_image && runs.is_empty() && !in_cell;
                        let style_name = current_para_style.as_deref();
                        let pagebreak = !in_cell
                            && (is_pagebreak_style(style_name, &para_styles)
                                || (saw_soft_page_break && runs.is_empty()));
                        if pagebreak && runs.is_empty() && !paragraph_has_image {
                            blocks.push(Block::PageBreak);
                        } else if !only_image {
                            if pagebreak && !runs.is_empty() {
                                blocks.push(Block::PageBreak);
                            }
                            push_finished_para(
                                runs,
                                style_name,
                                &para_styles,
                                in_cell,
                                &mut cell_paragraphs,
                                &mut blocks,
                                list_stack.last().map(|k| ListStyle {
                                    kind: *k,
                                    level: list_stack.len().saturating_sub(1) as u8,
                                }),
                            );
                        }
                        in_paragraph = false;
                        current_para_style = None;
                        paragraph_has_image = false;
                        saw_soft_page_break = false;
                    }
                    "table-cell" if in_cell => {
                        if cell_paragraphs.is_empty() {
                            cell_paragraphs.push(Paragraph::empty());
                        }
                        row_cells.push(TableCell {
                            paragraphs: std::mem::take(&mut cell_paragraphs),
                        });
                        in_cell = false;
                    }
                    "table-row" if in_row => {
                        table_rows.push(TableRow {
                            cells: std::mem::take(&mut row_cells),
                        });
                        in_row = false;
                    }
                    "table" if in_table => {
                        if table_rows.is_empty() {
                            table_rows.push(TableRow {
                                cells: vec![TableCell::empty()],
                            });
                        }
                        blocks.push(Block::Table(Table {
                            rows: std::mem::take(&mut table_rows),
                        }));
                        in_table = false;
                    }
                    "text" => in_text = false,
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(FormatError::InvalidDocument(format!("ODT XML error: {e}")));
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

fn make_paragraph(
    runs: Vec<Run>,
    style_name: Option<&str>,
    para_styles: &HashMap<String, ParaProps>,
) -> Paragraph {
    let mut para = if runs.is_empty() {
        Paragraph::empty()
    } else {
        Paragraph {
            runs,
            style: ParagraphStyle::default(),
        }
    };
    if let Some(name) = style_name {
        if let Some(named) = parse_odt_named_style(name) {
            apply_named_paragraph_defaults(&mut para.style, named);
        }
        if let Some(props) = para_styles.get(name) {
            para.style.alignment = props.alignment;
        }
    }
    para.normalize();
    para
}

fn parse_odt_named_style(name: &str) -> Option<NamedParagraphStyle> {
    match name {
        "Heading1" | "Heading_20_1" | "Heading 1" => Some(NamedParagraphStyle::Heading1),
        "Heading2" | "Heading_20_2" | "Heading 2" => Some(NamedParagraphStyle::Heading2),
        "Heading3" | "Heading_20_3" | "Heading 3" => Some(NamedParagraphStyle::Heading3),
        _ => None,
    }
}

fn push_text(
    runs: &mut Vec<Run>,
    text: &str,
    span_style: Option<&str>,
    text_styles: &HashMap<String, TextProps>,
    link: Option<&str>,
) {
    let mut style = TextStyle::default();
    if let Some(name) = span_style {
        if let Some(props) = text_styles.get(name) {
            style.bold = props.bold;
            style.italic = props.italic;
            style.underline = props.underline;
            if let Some(size) = props.font_size {
                style.font_size = size;
            }
        }
    }
    let link = link.map(str::to_string);
    if let Some(last) = runs.last_mut() {
        if last.style == style && last.link == link {
            last.text.push_str(text);
            return;
        }
    }
    runs.push(Run::new(text, style).with_link(link));
}

fn list_kind_from_style(name: Option<&str>) -> ListKind {
    match name {
        Some(n) if n.to_ascii_lowercase().contains("number") || n.contains("L_number") => {
            ListKind::Numbered
        }
        _ => ListKind::Bullet,
    }
}

fn local_name(name: QName<'_>) -> String {
    let raw = name.local_name();
    String::from_utf8_lossy(raw.as_ref()).into_owned()
}

fn attr(e: &quick_xml::events::BytesStart<'_>, local: &str) -> Option<String> {
    for a in e.attributes().flatten() {
        let key = local_name(a.key);
        if key == local {
            return Some(attr_value(&a));
        }
    }
    None
}

fn attr_value(a: &quick_xml::events::attributes::Attribute<'_>) -> String {
    let raw = String::from_utf8_lossy(&a.value);
    match quick_xml::escape::unescape(raw.as_ref()) {
        Ok(unescaped) => unescaped.into_owned(),
        Err(_) => raw.into_owned(),
    }
}

fn attr_key(a: &quick_xml::events::attributes::Attribute<'_>) -> String {
    local_name(a.key)
}

fn parse_text_props(e: &quick_xml::events::BytesStart<'_>) -> TextProps {
    let mut props = TextProps::default();
    for a in e.attributes().flatten() {
        let key = attr_key(&a);
        let val = attr_value(&a);
        match key.as_str() {
            "font-weight" | "font-weight-asian" => {
                if val == "bold" || val == "700" {
                    props.bold = true;
                }
            }
            "font-style" | "font-style-asian" => {
                if val == "italic" || val == "oblique" {
                    props.italic = true;
                }
            }
            "text-underline-style" => {
                if val != "none" {
                    props.underline = true;
                }
            }
            "font-size" | "font-size-asian" => {
                if let Some(n) = parse_points(&val) {
                    props.font_size = Some(n);
                }
            }
            _ => {}
        }
    }
    props
}

fn parse_para_props(e: &quick_xml::events::BytesStart<'_>) -> ParaProps {
    let mut props = ParaProps::default();
    for a in e.attributes().flatten() {
        let key = attr_key(&a);
        if key == "text-align" {
            let val = attr_value(&a);
            props.alignment = match val.as_str() {
                "center" => Alignment::Center,
                "end" | "right" => Alignment::Right,
                "justify" => Alignment::Justify,
                _ => Alignment::Left,
            };
        } else if key == "break-before" {
            props.break_before = attr_value(&a) == "page";
        }
    }
    props
}

fn is_pagebreak_style(name: Option<&str>, para_styles: &HashMap<String, ParaProps>) -> bool {
    match name {
        Some("P_pagebreak") => true,
        Some(n) => para_styles.get(n).map(|p| p.break_before).unwrap_or(false),
        None => false,
    }
}

fn finish_frame_image(
    href: &Option<String>,
    name: &str,
    width_pt: f32,
    height_pt: f32,
    pictures: &HashMap<String, Vec<u8>>,
) -> Option<Image> {
    let href = href.as_ref()?;
    let path = image_part_path(href).ok()?;
    let data = pictures
        .get(href)
        .or_else(|| pictures.get(&path))
        .cloned()?;
    let mime = mime_from_path(&path).to_string();
    Some(Image::from_embedded(
        mime,
        data,
        name.to_string(),
        width_pt.max(1.0),
        height_pt.max(1.0),
    ))
}

fn parse_length_pt(s: &str) -> Option<f32> {
    let s = s.trim();
    if let Some(num) = s.strip_suffix("cm") {
        let cm: f32 = num.trim().parse().ok()?;
        return Some(cm * 72.0 / 2.54);
    }
    if let Some(num) = s.strip_suffix("mm") {
        let mm: f32 = num.trim().parse().ok()?;
        return Some(mm * 72.0 / 25.4);
    }
    if let Some(num) = s.strip_suffix("in") {
        let inches: f32 = num.trim().parse().ok()?;
        return Some(inches * 72.0);
    }
    if let Some(num) = s.strip_suffix("pt") {
        return num.trim().parse().ok();
    }
    s.parse().ok()
}

fn parse_points(s: &str) -> Option<f32> {
    let s = s.trim();
    if let Some(num) = s.strip_suffix("pt") {
        return num.trim().parse().ok();
    }
    // already a number?
    s.parse().ok()
}
