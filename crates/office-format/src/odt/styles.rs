//! ODF page layouts, master pages and formatted header/footer paragraphs.
use super::xml::{self, OFFICE_NS};
use crate::FormatError;
use office_core::{Document, PageStyle, Paragraph, Run, TextStyle};
use quick_xml::{
    events::{BytesEnd, BytesStart, Event},
    Writer,
};
use std::collections::HashMap;

const STYLE_NS: &[&str] = &["urn:oasis:names:tc:opendocument:xmlns:style:1.0"];
const FO_NS: &[&str] = &["urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0"];
type Margins = (Option<Paragraph>, Option<Paragraph>);

pub fn apply_styles_xml(document: &mut Document, xml: &str) -> Result<(), FormatError> {
    apply_section_styles(document, xml, &[])
}

pub(super) fn apply_section_styles(
    document: &mut Document,
    source: &str,
    names: &[Option<String>],
) -> Result<(), FormatError> {
    let mut layouts = HashMap::new();
    let mut layout_name = None;
    let mut page = PageStyle::default();
    let mut masters: Vec<(String, Option<String>, Margins)> = Vec::new();
    let mut automatic = Writer::new(Vec::new());
    let mut master = None::<(String, Option<String>, BytesStart<'static>, Writer<Vec<u8>>)>;
    xml::read_xml(
        source,
        "document-styles",
        OFFICE_NS,
        |event, reader, parents| {
            if let Event::Start(e) = event {
                if xml::element(reader, e.name(), "automatic-styles", OFFICE_NS) {
                    automatic.write_event(Event::Start(xml::with_namespaces(e, reader)?))?;
                }
            }
            if parents.iter().any(|p| p == "automatic-styles") {
                automatic.write_event(event.clone())?;
            }
            match event {
                Event::Start(e) if xml::element(reader, e.name(), "page-layout", STYLE_NS) => {
                    layout_name = xml::namespaced_attr(e, reader, "name", STYLE_NS)?;
                    page = PageStyle::default();
                }
                Event::Start(e) | Event::Empty(e)
                    if xml::element(reader, e.name(), "page-layout-properties", STYLE_NS) =>
                {
                    let length = |key: &str, target: &mut f32| -> Result<(), FormatError> {
                        if let Some(value) = xml::namespaced_attr(e, reader, key, FO_NS)? {
                            *target = super::read::parse_length_pt(&value).ok_or_else(|| {
                                xml::invalid(format!("Invalid page layout {key}"))
                            })?;
                        }
                        Ok(())
                    };
                    if let Some(value) = xml::namespaced_attr(e, reader, "margin", FO_NS)? {
                        let value = super::read::parse_length_pt(&value)
                            .ok_or_else(|| xml::invalid("Invalid page margin"))?;
                        page.margin_top = value;
                        page.margin_bottom = value;
                        page.margin_left = value;
                        page.margin_right = value;
                    }
                    length("page-width", &mut page.width)?;
                    length("page-height", &mut page.height)?;
                    length("margin-top", &mut page.margin_top)?;
                    length("margin-bottom", &mut page.margin_bottom)?;
                    length("margin-left", &mut page.margin_left)?;
                    length("margin-right", &mut page.margin_right)?;
                }
                Event::End(e) if xml::element(reader, e.name(), "page-layout", STYLE_NS) => {
                    if !page.is_valid() {
                        return Err(xml::invalid("Invalid ODT paper size or margins"));
                    }
                    let name = layout_name
                        .take()
                        .ok_or_else(|| xml::invalid("Missing page layout name"))?;
                    if layouts.insert(name, page.clone()).is_some() {
                        return Err(xml::invalid("Duplicate page layout"));
                    }
                }
                _ => {}
            }
            if let Event::Start(e) | Event::Empty(e) = event {
                if xml::element(reader, e.name(), "master-page", STYLE_NS) {
                    let name = xml::namespaced_attr(e, reader, "name", STYLE_NS)?
                        .ok_or_else(|| xml::invalid("Missing master page name"))?;
                    let layout = xml::namespaced_attr(e, reader, "page-layout-name", STYLE_NS)?;
                    let root = xml::office_wrapper(reader, "document-styles")?;
                    let mut writer = Writer::new(Vec::new());
                    writer.write_event(Event::Start(root.clone()))?;
                    writer.get_mut().extend_from_slice(automatic.get_ref());
                    master = Some((name, layout, root, writer));
                }
            }
            if let Some((_, _, _, writer)) = &mut master {
                writer.write_event(event.clone())?;
            }
            let finish = match event {
                Event::End(e) => xml::element(reader, e.name(), "master-page", STYLE_NS),
                Event::Empty(e) => xml::element(reader, e.name(), "master-page", STYLE_NS),
                _ => false,
            };
            if finish {
                let (name, layout, root, mut writer) = master.take().unwrap();
                writer.write_event(Event::End(BytesEnd::new(
                    std::str::from_utf8(root.name().as_ref())
                        .map_err(|e| xml::invalid(e.to_string()))?,
                )))?;
                let text = String::from_utf8(writer.into_inner())
                    .map_err(|e| xml::invalid(e.to_string()))?;
                masters.push((name, layout, parse_master_header_footer(&text)?));
            }
            Ok(())
        },
    )
    .map_err(|e| xml::part_error("styles.xml", e))?;
    let mut seen = std::collections::HashSet::new();
    for (name, _, _) in &masters {
        if !seen.insert(name) {
            return Err(xml::invalid("Duplicate master page"));
        }
    }
    let mut updates = Vec::new();
    for (index, section) in document.sections.iter().enumerate() {
        let mut page_style = section.page_style.clone();
        let mut new_header = section.header.clone();
        let mut new_footer = section.footer.clone();
        let requested = names.get(index).and_then(Option::as_deref);
        let master = if let Some(name) = requested {
            Some(
                masters
                    .iter()
                    .find(|(n, _, _)| n == name)
                    .ok_or_else(|| xml::invalid(format!("Missing master page {name}")))?,
            )
        } else {
            masters.first()
        };
        if let Some((_, layout, (header, footer))) = master {
            if let Some(name) = layout {
                // Older rust-office files referenced Mpm1 without defining it.
                if let Some(page) = layouts.get(name) {
                    page_style = page.clone();
                } else if !(layouts.is_empty() && name == "Mpm1") {
                    return Err(xml::invalid(format!("Missing page layout {name}")));
                }
            }
            new_header = header
                .clone()
                .or_else(|| (index > 0).then(Paragraph::empty));
            new_footer = footer
                .clone()
                .or_else(|| (index > 0).then(Paragraph::empty));
        }
        updates.push((page_style, new_header, new_footer));
    }
    for (section, (page, header, footer)) in document.sections.iter_mut().zip(updates) {
        section.page_style = page;
        section.header = header;
        section.footer = footer;
    }
    Ok(())
}

/// Parse the first master page's margin text using the regular styled paragraph reader.
pub fn parse_master_header_footer(source: &str) -> Result<Margins, FormatError> {
    let mut automatic = Writer::new(Vec::new());
    let mut header = None::<(BytesStart<'static>, Writer<Vec<u8>>)>;
    let mut footer = None::<(BytesStart<'static>, Writer<Vec<u8>>)>;
    let mut active = None;
    xml::read_xml(
        source,
        "document-styles",
        OFFICE_NS,
        |event, reader, parents| {
            if let Event::Start(e) = event {
                if xml::element(reader, e.name(), "automatic-styles", OFFICE_NS) {
                    automatic.write_event(Event::Start(xml::with_namespaces(e, reader)?))?;
                }
            }
            if parents.iter().any(|p| p == "automatic-styles") {
                automatic.write_event(event.clone())?;
            }
            if let Event::Start(e) | Event::Empty(e) = event {
                let is_header = xml::element(reader, e.name(), "header", STYLE_NS);
                let is_footer = xml::element(reader, e.name(), "footer", STYLE_NS);
                if is_header || is_footer {
                    let target = if is_header { &mut header } else { &mut footer };
                    if target.is_none() {
                        *target = Some((
                            xml::office_wrapper(reader, "document-content")?,
                            Writer::new(Vec::new()),
                        ));
                        if matches!(event, Event::Start(_)) {
                            active = Some(is_header);
                        }
                    }
                    return Ok(());
                }
            }
            if let Some(is_header) = active {
                let ends_margin = match event {
                    Event::End(e) => xml::element(
                        reader,
                        e.name(),
                        if is_header { "header" } else { "footer" },
                        STYLE_NS,
                    ),
                    _ => false,
                };
                if ends_margin {
                    active = None;
                } else {
                    let target = if is_header { &mut header } else { &mut footer };
                    target.as_mut().unwrap().1.write_event(event.clone())?;
                }
            }
            Ok(())
        },
    )
    .map_err(|e| xml::part_error("styles.xml", e))?;
    let parse = |part: Option<(BytesStart<'static>, Writer<Vec<u8>>)>| -> Result<Option<Paragraph>, FormatError> {
        let Some((root, contents)) = part else { return Ok(None); };
        let name = std::str::from_utf8(root.name().as_ref()).map_err(|e| xml::invalid(e.to_string()))?.to_owned();
        let prefix = name.split(':').next().unwrap();
        let mut writer = Writer::new(Vec::new());
        writer.write_event(Event::Start(root))?;
        writer.get_mut().extend_from_slice(automatic.get_ref());
        for tag in ["body", "text"] { writer.write_event(Event::Start(BytesStart::new(format!("{prefix}:{tag}"))))?; }
        writer.get_mut().extend_from_slice(contents.get_ref());
        for tag in ["text", "body"] { writer.write_event(Event::End(BytesEnd::new(format!("{prefix}:{tag}"))))?; }
        writer.write_event(Event::End(BytesEnd::new(name)))?;
        let content = String::from_utf8(writer.into_inner()).map_err(|e| xml::invalid(e.to_string()))?;
        let document = super::read::parse_content_xml(&content)?;
        let mut paragraphs = document.sections.into_iter().flat_map(|s| s.blocks).filter_map(|b| {
            if let office_core::Block::Paragraph(p) = b { Some(p) } else { None }
        });
        let mut result = paragraphs.next().unwrap_or_else(Paragraph::empty);
        for paragraph in paragraphs { result.runs.push(Run::new("\n", TextStyle::default())); result.runs.extend(paragraph.runs); }
        Ok(Some(result))
    };
    Ok((parse(header)?, parse(footer)?))
}
