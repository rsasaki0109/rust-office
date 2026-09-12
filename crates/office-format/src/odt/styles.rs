//! Parse ODT `styles.xml` for master-page header/footer text.

use office_core::{Document, Paragraph, Run, TextStyle};
use quick_xml::events::Event;
use quick_xml::name::QName;
use quick_xml::reader::Reader;

use crate::FormatError;

/// Apply header/footer paragraphs from `styles.xml` onto `document` (in place).
pub fn apply_styles_xml(document: &mut Document, xml: &str) -> Result<(), FormatError> {
    let (header, footer) = parse_master_header_footer(xml)?;
    if let Some(p) = header {
        document.main_section_mut().header = Some(p);
    }
    if let Some(p) = footer {
        document.main_section_mut().footer = Some(p);
    }
    Ok(())
}

/// Extract the first master-page header and footer paragraphs (plain text runs).
pub fn parse_master_header_footer(
    xml: &str,
) -> Result<(Option<Paragraph>, Option<Paragraph>), FormatError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);

    let mut in_header = false;
    let mut in_footer = false;
    let mut in_p = false;
    let mut current_runs: Vec<Run> = Vec::new();
    let mut header: Option<Paragraph> = None;
    let mut footer: Option<Paragraph> = None;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let local = local_name(e.name());
                match local.as_str() {
                    "header" => {
                        in_header = true;
                        current_runs.clear();
                    }
                    "footer" => {
                        in_footer = true;
                        current_runs.clear();
                    }
                    "p" if in_header || in_footer => {
                        in_p = true;
                        current_runs.clear();
                    }
                    "s" if in_p => {
                        let count = attr_c(&e).unwrap_or(1);
                        push_plain(&mut current_runs, &" ".repeat(count));
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(e)) => {
                let local = local_name(e.name());
                match local.as_str() {
                    "s" if in_p => {
                        let count = attr_c(&e).unwrap_or(1);
                        push_plain(&mut current_runs, &" ".repeat(count));
                    }
                    "p" if in_header || in_footer => {
                        let para = runs_to_para(std::mem::take(&mut current_runs));
                        if in_header && header.is_none() {
                            header = Some(para);
                        } else if in_footer && footer.is_none() {
                            footer = Some(para);
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(t)) => {
                if in_p {
                    let text = t.unescape().unwrap_or_default();
                    if !text.is_empty() {
                        push_plain(&mut current_runs, &text);
                    }
                }
            }
            Ok(Event::End(e)) => {
                let local = local_name(e.name());
                match local.as_str() {
                    "p" if in_p => {
                        let para = runs_to_para(std::mem::take(&mut current_runs));
                        if in_header && header.is_none() {
                            header = Some(para);
                        } else if in_footer && footer.is_none() {
                            footer = Some(para);
                        }
                        in_p = false;
                    }
                    "header" => in_header = false,
                    "footer" => in_footer = false,
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(FormatError::InvalidDocument(format!(
                    "styles.xml error: {e}"
                )));
            }
            _ => {}
        }
        buf.clear();
    }

    Ok((header, footer))
}

fn runs_to_para(runs: Vec<Run>) -> Paragraph {
    if runs.is_empty() {
        Paragraph::empty()
    } else {
        let mut p = Paragraph {
            runs,
            style: Default::default(),
        };
        p.normalize();
        p
    }
}

fn push_plain(runs: &mut Vec<Run>, text: &str) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = runs.last_mut() {
        if last.style == TextStyle::default() && last.link.is_none() {
            last.text.push_str(text);
            return;
        }
    }
    runs.push(Run::new(text, TextStyle::default()));
}

fn local_name(name: QName<'_>) -> String {
    let raw = name.local_name();
    String::from_utf8_lossy(raw.as_ref()).into_owned()
}

fn attr_c(e: &quick_xml::events::BytesStart<'_>) -> Option<usize> {
    for a in e.attributes().flatten() {
        let key = String::from_utf8_lossy(a.key.local_name().as_ref()).into_owned();
        if key == "c" {
            return String::from_utf8_lossy(&a.value).parse().ok();
        }
    }
    None
}
