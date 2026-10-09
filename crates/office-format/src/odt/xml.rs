//! ODT part validation and namespace-aware package image references.

use crate::FormatError;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::{QName, ResolveResult};
use quick_xml::reader::NsReader;

pub(super) const OFFICE_NS: &[&str] = &["urn:oasis:names:tc:opendocument:xmlns:office:1.0"];
pub(super) const DRAW_NS: &[&str] = &["urn:oasis:names:tc:opendocument:xmlns:drawing:1.0"];
pub(super) const XLINK_NS: &[&str] = &["http://www.w3.org/1999/xlink"];
const TEXT_NS: &[&str] = &["urn:oasis:names:tc:opendocument:xmlns:text:1.0"];
// Bound the expansion of compact text:s runs before allocating repeated spaces.
const MAX_EXPANDED_SPACES: usize = 1_000_000;

pub(super) fn invalid(message: impl Into<String>) -> FormatError {
    FormatError::InvalidDocument(message.into())
}
pub(super) fn part_error(path: &str, error: FormatError) -> FormatError {
    match error {
        FormatError::InvalidDocument(message) => invalid(format!("ODT {path}: {message}")),
        other => invalid(format!("ODT {path}: {other}")),
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
// truncated document. Track open elements and require one complete ODF root.
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
    let mut expanded_spaces = 0usize;
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
                if element(&reader, e.name(), "s", TEXT_NS) {
                    expanded_spaces = expanded_spaces.saturating_add(space_count(e, &reader)?);
                    if expanded_spaces > MAX_EXPANDED_SPACES {
                        return Err(invalid(
                            "text:s expansion exceeds 1,000,000 spaces per XML part",
                        ));
                    }
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
            Event::DocType(_) => return Err(invalid("DTD is unsupported in ODT XML parts")),
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

pub(super) fn space_count(
    e: &BytesStart<'_>,
    reader: &NsReader<&[u8]>,
) -> Result<usize, FormatError> {
    let count = namespaced_attr(e, reader, "c", TEXT_NS)?;
    match count {
        None => Ok(1),
        Some(value) => value
            .parse::<usize>()
            .ok()
            .filter(|count| (1..=MAX_EXPANDED_SPACES).contains(count))
            .ok_or_else(|| invalid("text:s count must be between 1 and 1,000,000")),
    }
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

/// Resolve a target URI relative to content.xml for ZIP lookup.
pub(super) fn image_part_path(target: &str) -> Result<String, FormatError> {
    if target.starts_with("//") {
        return Err(invalid("external image targets are unsupported"));
    }
    if target.contains(['\\', '?', '#', ':']) {
        return Err(invalid(format!("invalid ODT part target {target:?}")));
    }
    let mut decoded = Vec::new();
    let bytes = target.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes
                .get(i + 1..i + 3)
                .ok_or_else(|| invalid("invalid percent-encoded ODT part target"))?;
            let hex = std::str::from_utf8(hex)
                .map_err(|_| invalid("invalid percent-encoded ODT part target"))?;
            let byte = u8::from_str_radix(hex, 16)
                .map_err(|_| invalid("invalid percent-encoded ODT part target"))?;
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
        String::from_utf8(decoded).map_err(|_| invalid("ODT part target is not UTF-8"))?;
    let mut parts = Vec::new();
    for part in decoded.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err(invalid("ODT part target leaves package root"));
                }
            }
            part => parts.push(part),
        }
    }
    if parts.is_empty() {
        return Err(invalid("empty ODT part target"));
    }
    Ok(parts.join("/"))
}

pub(super) fn content_images(text: &str) -> Result<Vec<String>, FormatError> {
    let mut images = Vec::new();
    let mut seen_body = false;
    let mut seen_text = false;
    let mut in_text = false;
    read_xml(text, "document-content", OFFICE_NS, |event, reader, parents| {
        match event {
            Event::Start(e) | Event::Empty(e) => {
                if parents == ["document-content"] && element(reader, e.name(), "body", OFFICE_NS) {
                    if seen_body { return Err(invalid("duplicate office:body")); }
                    seen_body = true;
                }
                if parents == ["document-content", "body"] && element(reader, e.name(), "text", OFFICE_NS) {
                    if seen_text { return Err(invalid("duplicate office:text")); }
                    seen_text = true;
                    in_text = matches!(event, Event::Start(_));
                }
                if in_text && element(reader, e.name(), "image", DRAW_NS) {
                    let href = namespaced_attr(e, reader, "href", XLINK_NS)?
                        .filter(|href| !href.is_empty())
                        .ok_or_else(|| invalid("draw:image requires a package xlink:href; inline binary images are unsupported"))?;
                    image_part_path(&href)?;
                    images.push(href);
                }
            }
            Event::End(e) if parents == ["document-content", "body", "text"] && element(reader, e.name(), "text", OFFICE_NS) => in_text = false,
            _ => {}
        }
        Ok(())
    }).map_err(|e| part_error("content.xml", e))?;
    if !seen_body || !seen_text {
        return Err(invalid("ODT content.xml: requires office:body/office:text"));
    }
    Ok(images)
}

/// Keep inherited namespace bindings when a subtree is parsed independently.
pub(super) fn with_namespaces(
    element: &BytesStart<'_>,
    reader: &NsReader<&[u8]>,
) -> Result<BytesStart<'static>, FormatError> {
    use quick_xml::name::PrefixDeclaration;
    let mut element = element.clone().into_owned();
    for (prefix, namespace) in reader.prefixes() {
        let key = match prefix {
            PrefixDeclaration::Default => "xmlns".to_owned(),
            PrefixDeclaration::Named(name) => format!(
                "xmlns:{}",
                std::str::from_utf8(name).map_err(|e| invalid(e.to_string()))?
            ),
        };
        if element
            .try_get_attribute(key.as_str())
            .map_err(|e| invalid(e.to_string()))?
            .is_none()
        {
            element.push_attribute((
                key.as_str(),
                std::str::from_utf8(namespace.as_ref()).map_err(|e| invalid(e.to_string()))?,
            ));
        }
    }
    Ok(element)
}

pub(super) fn office_wrapper(
    reader: &NsReader<&[u8]>,
    local: &str,
) -> Result<BytesStart<'static>, FormatError> {
    let mut index = 0;
    loop {
        let prefix = format!("rooffice{index}");
        let key = format!("xmlns:{prefix}");
        let mut element = with_namespaces(&BytesStart::new(format!("{prefix}:{local}")), reader)?;
        if element
            .try_get_attribute(key.as_str())
            .map_err(|e| invalid(e.to_string()))?
            .is_none()
        {
            element.push_attribute((key.as_str(), OFFICE_NS[0]));
            return Ok(element);
        }
        index += 1;
    }
}
