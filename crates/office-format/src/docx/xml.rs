//! DOCX XML validation and namespace-aware relationship attributes.

use crate::FormatError;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::{QName, ResolveResult};
use quick_xml::reader::NsReader;

pub(super) const WORD_NS: &[&str] = &[
    "http://schemas.openxmlformats.org/wordprocessingml/2006/main",
    "http://purl.oclc.org/ooxml/wordprocessingml/main",
];
pub(super) const DRAWING_NS: &[&str] = &[
    "http://schemas.openxmlformats.org/drawingml/2006/main",
    "http://purl.oclc.org/ooxml/drawingml/main",
];
pub(super) const REL_NS: &[&str] = &[
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
    "http://purl.oclc.org/ooxml/officeDocument/relationships",
];
pub(super) const PACKAGE_NS: &[&str] =
    &["http://schemas.openxmlformats.org/package/2006/relationships"];

pub(super) fn invalid(message: impl Into<String>) -> FormatError {
    FormatError::InvalidDocument(message.into())
}
pub(super) fn part_error(path: &str, error: FormatError) -> FormatError {
    match error {
        FormatError::InvalidDocument(message) => invalid(format!("DOCX {path}: {message}")),
        other => invalid(format!("DOCX {path}: {other}")),
    }
}
fn namespace_matches(namespace: ResolveResult<'_>, allowed: &[&str]) -> bool {
    matches!(namespace, ResolveResult::Bound(ns) if allowed.iter().any(|s| ns.as_ref() == s.as_bytes()))
}

pub(super) fn element(
    reader: &NsReader<&[u8]>,
    name: QName<'_>,
    local: &str,
    namespaces: &[&str],
) -> bool {
    let (namespace, found) = reader.resolve_element(name);
    found.as_ref() == local.as_bytes() && namespace_matches(namespace, namespaces)
}

// quick-xml detects mismatched closing tags, but EOF alone does not reject a
// truncated document. Track open elements and require one complete OOXML root.
pub(super) fn read_xml(
    xml: &str,
    root: &str,
    namespaces: &[&str],
    mut visit: impl FnMut(&Event<'_>, &NsReader<&[u8]>, &[String]) -> Result<(), FormatError>,
) -> Result<(), FormatError> {
    let mut reader = NsReader::from_str(xml);
    reader.config_mut().check_end_names = true;
    let mut parents = Vec::new();
    let mut seen_root = false;
    loop {
        let event = reader
            .read_event()
            .map_err(|e| invalid(format!("invalid XML: {e}")))?;
        match &event {
            Event::Start(e) | Event::Empty(e) => {
                if matches!(
                    reader.resolve_element(e.name()).0,
                    ResolveResult::Unknown(_)
                ) {
                    return Err(invalid("undeclared XML element prefix"));
                }
                if parents.is_empty() {
                    if seen_root || !element(&reader, e.name(), root, namespaces) {
                        return Err(invalid(format!("expected a single {root} root")));
                    }
                    seen_root = true;
                }
                for attribute in e.attributes() {
                    let attribute =
                        attribute.map_err(|e| invalid(format!("invalid XML attribute: {e}")))?;
                    if matches!(
                        reader.resolve_attribute(attribute.key).0,
                        ResolveResult::Unknown(_)
                    ) {
                        return Err(invalid("undeclared XML attribute prefix"));
                    }
                    attribute
                        .unescape_value()
                        .map_err(|e| invalid(format!("invalid XML attribute value: {e}")))?;
                }
            }
            Event::Text(text) => {
                let text = text
                    .unescape()
                    .map_err(|e| invalid(format!("invalid XML text: {e}")))?;
                if parents.is_empty() && !text.trim().is_empty() {
                    return Err(invalid("text outside XML root"));
                }
            }
            Event::CData(_) if parents.is_empty() => return Err(invalid("CDATA outside XML root")),
            Event::DocType(_) => return Err(invalid("DTD is unsupported in DOCX XML parts")),
            Event::Decl(_) if seen_root => return Err(invalid("XML declaration after root")),
            Event::Eof => {
                if !seen_root || !parents.is_empty() {
                    return Err(invalid("empty or truncated XML document"));
                }
                return Ok(());
            }
            _ => {}
        }
        visit(&event, &reader, &parents)?;
        match event {
            Event::Start(e) => parents.push(local_name(e.name())),
            Event::End(_) => {
                parents
                    .pop()
                    .ok_or_else(|| invalid("unexpected XML closing tag"))?;
            }
            _ => {}
        }
    }
}

pub(super) fn attr(e: &BytesStart<'_>, key: &str) -> Result<Option<String>, FormatError> {
    let attr = e
        .try_get_attribute(key)
        .map_err(|e| invalid(format!("invalid XML attribute: {e}")))?;
    attr.map(|a| {
        a.unescape_value()
            .map(|s| s.into_owned())
            .map_err(|e| invalid(format!("invalid XML attribute value: {e}")))
    })
    .transpose()
}

pub(super) fn required_attr(e: &BytesStart<'_>, key: &str) -> Result<String, FormatError> {
    attr(e, key)?
        .filter(|s| !s.is_empty())
        .ok_or_else(|| invalid(format!("missing {key} attribute")))
}

fn local_name(name: QName<'_>) -> String {
    String::from_utf8_lossy(name.local_name().as_ref()).into_owned()
}

pub(super) fn namespaced_attr(
    e: &BytesStart<'_>,
    reader: &NsReader<&[u8]>,
    key: &str,
    namespaces: &[&str],
) -> Result<Option<String>, FormatError> {
    let mut value = None;
    for attr in e.attributes() {
        let attr = attr.map_err(|e| invalid(e.to_string()))?;
        let (namespace, local) = reader.resolve_attribute(attr.key);
        if local.as_ref() == key.as_bytes() && namespace_matches(namespace, namespaces) {
            if value.is_some() {
                return Err(invalid(format!("duplicate {key} attribute")));
            }
            value = Some(
                attr.unescape_value()
                    .map_err(|e| invalid(e.to_string()))?
                    .into_owned(),
            );
        }
    }
    Ok(value)
}
