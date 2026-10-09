//! Namespace-aware next-page section properties.
use super::xml::{self, invalid, REL_NS, WORD_NS};
use crate::FormatError;
use office_core::PageStyle;
use quick_xml::events::Event;

#[derive(Default)]
pub(super) struct Properties {
    pub style: PageStyle,
    pub header: Option<String>,
    pub footer: Option<String>,
}

pub(super) fn read(xml_text: &str) -> Result<Vec<Properties>, FormatError> {
    let mut sections = Vec::new();
    let mut current = None::<Properties>;
    let mut header_default = false;
    let mut footer_default = false;
    xml::read_xml(xml_text, "document", WORD_NS, |event, reader, parents| {
        match event {
            Event::Start(e) | Event::Empty(e)
                if xml::element(reader, e.name(), "sectPr", WORD_NS) =>
            {
                if current.is_some()
                    || parents.iter().any(|p| p == "tbl")
                    || !matches!(parents.last().map(String::as_str), Some("body" | "pPr"))
                {
                    return Err(invalid("Unsupported nested section properties"));
                }
                current = Some(Properties::default());
                header_default = false;
                footer_default = false;
                if matches!(event, Event::Empty(_)) {
                    sections.push(current.take().unwrap());
                }
            }
            Event::Start(e) | Event::Empty(e) if current.is_some() => {
                let properties = current.as_mut().unwrap();
                let number = |key: &str, target: &mut f32| -> Result<(), FormatError> {
                    if let Some(value) = xml::namespaced_attr(e, reader, key, WORD_NS)? {
                        *target = value
                            .parse::<f32>()
                            .map_err(|_| invalid(format!("Invalid section {key}")))?
                            / 20.0;
                    }
                    Ok(())
                };
                if xml::element(reader, e.name(), "pgSz", WORD_NS) {
                    number("w", &mut properties.style.width)?;
                    number("h", &mut properties.style.height)?;
                } else if xml::element(reader, e.name(), "pgMar", WORD_NS) {
                    number("top", &mut properties.style.margin_top)?;
                    number("bottom", &mut properties.style.margin_bottom)?;
                    number("left", &mut properties.style.margin_left)?;
                    number("right", &mut properties.style.margin_right)?;
                } else if xml::element(reader, e.name(), "type", WORD_NS) {
                    if xml::namespaced_attr(e, reader, "val", WORD_NS)?
                        .is_some_and(|v| v != "nextPage")
                    {
                        return Err(invalid("Only next-page DOCX sections are supported"));
                    }
                } else if xml::element(reader, e.name(), "headerReference", WORD_NS)
                    || xml::element(reader, e.name(), "footerReference", WORD_NS)
                {
                    let is_header = xml::element(reader, e.name(), "headerReference", WORD_NS);
                    let is_default = xml::namespaced_attr(e, reader, "type", WORD_NS)?
                        .is_none_or(|v| v == "default");
                    let (target, selected_default) = if is_header {
                        (&mut properties.header, &mut header_default)
                    } else {
                        (&mut properties.footer, &mut footer_default)
                    };
                    if target.is_none() || is_default && !*selected_default {
                        *target = xml::namespaced_attr(e, reader, "id", REL_NS)?;
                        *selected_default = is_default;
                    }
                }
            }
            Event::End(e) if xml::element(reader, e.name(), "sectPr", WORD_NS) => {
                let properties = current
                    .take()
                    .ok_or_else(|| invalid("Missing section properties"))?;
                if !properties.style.is_valid() {
                    return Err(invalid("Invalid section paper size or margins"));
                }
                sections.push(properties);
            }
            _ => {}
        }
        Ok(())
    })?;
    Ok(sections)
}
