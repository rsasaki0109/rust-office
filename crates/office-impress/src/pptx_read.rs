//! Minimal PPTX (OOXML) reader — title/body text per slide.

use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Read};
use std::path::Path;

use quick_xml::events::{BytesStart, Event};
use quick_xml::name::{QName, ResolveResult};
use quick_xml::reader::NsReader;
use zip::ZipArchive;

use crate::model::{Presentation, Slide};
use crate::pptx::PptxError;
use crate::theme::Theme;

const PRESENTATION_NS: &[&str] = &[
    "http://schemas.openxmlformats.org/presentationml/2006/main",
    "http://purl.oclc.org/ooxml/presentationml/main",
];
const DRAWING_NS: &[&str] = &[
    "http://schemas.openxmlformats.org/drawingml/2006/main",
    "http://purl.oclc.org/ooxml/drawingml/main",
];
const REL_NS: &[&str] = &[
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
    "http://purl.oclc.org/ooxml/officeDocument/relationships",
];
const PACKAGE_NS: &[&str] = &["http://schemas.openxmlformats.org/package/2006/relationships"];
const CORE_NS: &[&str] =
    &["http://schemas.openxmlformats.org/package/2006/metadata/core-properties"];
const DC_NS: &[&str] = &["http://purl.org/dc/elements/1.1/"];

/// Load a presentation from a `.pptx` path.
pub fn load_pptx_path(path: &Path) -> Result<Presentation, PptxError> {
    let bytes = std::fs::read(path)?;
    load_pptx_bytes(&bytes)
}

/// Load a presentation from PPTX package bytes. Any listed slide failure rejects
/// the entire import; ZIP directory order and relationship IDs never set slide order.
pub fn load_pptx_bytes(bytes: &[u8]) -> Result<Presentation, PptxError> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))?;
    let manifest_path = "ppt/presentation.xml";
    let manifest = read_zip_string(&mut archive, manifest_path)?;
    let ids = parse_slide_ids(&manifest).map_err(|e| part_error(manifest_path, e))?;
    let rels_path = "ppt/_rels/presentation.xml.rels";
    let rels = match read_optional_zip_string(&mut archive, rels_path)? {
        Some(xml) => parse_relationships(&xml).map_err(|e| part_error(rels_path, e))?,
        None if ids.is_empty() => HashMap::new(),
        None => return Err(parse_error(format!("missing {rels_path}"))),
    };
    let mut slides = Vec::new();
    for id in ids {
        let rel = rels
            .get(&id)
            .ok_or_else(|| parse_error(format!("{rels_path}: missing slide relationship {id}")))?;
        if !REL_NS.iter().any(|ns| rel.kind == format!("{ns}/slide")) || rel.external {
            return Err(parse_error(format!(
                "{rels_path}: {id} is not an internal slide relationship"
            )));
        }
        let path = slide_part_path(&rel.target).map_err(|e| part_error(rels_path, e))?;
        let xml = read_zip_string(&mut archive, &path)?;
        slides.push(parse_slide_xml(&xml).map_err(|e| part_error(&path, e))?);
    }
    // A valid presentation with no listed slides opens as an editable blank deck.
    // Unlisted/orphan slide files are deliberately ignored.
    if slides.is_empty() {
        slides.push(Slide::blank());
    }
    let theme_path = "ppt/theme/theme1.xml";
    let theme = match read_optional_zip_string(&mut archive, theme_path)? {
        Some(xml) => parse_theme_accent(&xml).map_err(|e| part_error(theme_path, e))?,
        None => Theme::light(),
    };
    let title_path = "docProps/core.xml";
    let title = match read_optional_zip_string(&mut archive, title_path)? {
        Some(xml) => extract_dc_title(&xml).map_err(|e| part_error(title_path, e))?,
        None => None,
    }
    .filter(|t| !t.is_empty())
    .unwrap_or_else(|| "Imported presentation".into());

    let mut p = Presentation::new();
    p.title = title;
    p.theme = theme;
    p.slides = slides;
    p.active = 0;
    p.mark_clean();
    Ok(p)
}

fn parse_error(message: impl Into<String>) -> PptxError {
    PptxError::Parse(message.into())
}
fn part_error(path: &str, error: PptxError) -> PptxError {
    match error {
        PptxError::Parse(message) => parse_error(format!("{path}: {message}")),
        other => parse_error(format!("{path}: {other}")),
    }
}

fn read_zip_string<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
    path: &str,
) -> Result<String, PptxError> {
    read_optional_zip_string(archive, path)?.ok_or_else(|| parse_error(format!("missing {path}")))
}

fn read_optional_zip_string<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
    path: &str,
) -> Result<Option<String>, PptxError> {
    let mut file = match archive.by_name(path) {
        Ok(file) => file,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(error) => return Err(parse_error(format!("cannot read {path}: {error}"))),
    };
    let mut text = String::new();
    file.read_to_string(&mut text)
        .map_err(|e| parse_error(format!("cannot read {path}: {e}")))?;
    Ok(Some(text))
}

fn namespace_matches(namespace: ResolveResult<'_>, allowed: &[&str]) -> bool {
    matches!(namespace, ResolveResult::Bound(ns) if allowed.iter().any(|s| ns.as_ref() == s.as_bytes()))
}

fn element(reader: &NsReader<&[u8]>, name: QName<'_>, local: &str, namespaces: &[&str]) -> bool {
    let (namespace, found) = reader.resolve_element(name);
    found.as_ref() == local.as_bytes() && namespace_matches(namespace, namespaces)
}

// quick-xml detects mismatched closing tags, but EOF alone does not reject a
// truncated document. Track open elements and require one complete OOXML root.
fn read_xml(
    xml: &str,
    root: &str,
    namespaces: &[&str],
    mut visit: impl FnMut(&Event<'_>, &NsReader<&[u8]>, &[String]) -> Result<(), PptxError>,
) -> Result<(), PptxError> {
    let mut reader = NsReader::from_str(xml);
    reader.config_mut().check_end_names = true;
    let mut parents = Vec::new();
    let mut seen_root = false;
    loop {
        let event = reader
            .read_event()
            .map_err(|e| parse_error(format!("invalid XML: {e}")))?;
        match &event {
            Event::Start(e) | Event::Empty(e) => {
                if matches!(
                    reader.resolve_element(e.name()).0,
                    ResolveResult::Unknown(_)
                ) {
                    return Err(parse_error("undeclared XML element prefix"));
                }
                if parents.is_empty() {
                    if seen_root || !element(&reader, e.name(), root, namespaces) {
                        return Err(parse_error(format!("expected a single {root} root")));
                    }
                    seen_root = true;
                }
                for attribute in e.attributes() {
                    let attribute = attribute
                        .map_err(|e| parse_error(format!("invalid XML attribute: {e}")))?;
                    if matches!(
                        reader.resolve_attribute(attribute.key).0,
                        ResolveResult::Unknown(_)
                    ) {
                        return Err(parse_error("undeclared XML attribute prefix"));
                    }
                    attribute
                        .unescape_value()
                        .map_err(|e| parse_error(format!("invalid XML attribute value: {e}")))?;
                }
            }
            Event::Text(text) => {
                let text = text
                    .unescape()
                    .map_err(|e| parse_error(format!("invalid XML text: {e}")))?;
                if parents.is_empty() && !text.trim().is_empty() {
                    return Err(parse_error("text outside XML root"));
                }
            }
            Event::CData(_) if parents.is_empty() => {
                return Err(parse_error("CDATA outside XML root"))
            }
            Event::DocType(_) => return Err(parse_error("DTD is unsupported in PPTX XML parts")),
            Event::Decl(_) if seen_root => return Err(parse_error("XML declaration after root")),
            Event::Eof => {
                if !seen_root || !parents.is_empty() {
                    return Err(parse_error("empty or truncated XML document"));
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
                    .ok_or_else(|| parse_error("unexpected XML closing tag"))?;
            }
            _ => {}
        }
    }
}

fn attr(e: &BytesStart<'_>, key: &str) -> Result<Option<String>, PptxError> {
    let attr = e
        .try_get_attribute(key)
        .map_err(|e| parse_error(format!("invalid XML attribute: {e}")))?;
    attr.map(|a| {
        a.unescape_value()
            .map(|s| s.into_owned())
            .map_err(|e| parse_error(format!("invalid XML attribute value: {e}")))
    })
    .transpose()
}

fn required_attr(e: &BytesStart<'_>, key: &str) -> Result<String, PptxError> {
    attr(e, key)?
        .filter(|s| !s.is_empty())
        .ok_or_else(|| parse_error(format!("missing {key} attribute")))
}

fn relationship_id(e: &BytesStart<'_>, reader: &NsReader<&[u8]>) -> Result<String, PptxError> {
    let mut id = None;
    for attribute in e.attributes() {
        let attribute =
            attribute.map_err(|e| parse_error(format!("invalid slide attribute: {e}")))?;
        let (namespace, local) = reader.resolve_attribute(attribute.key);
        if local.as_ref() == b"id" && namespace_matches(namespace, REL_NS) {
            if id.is_some() {
                return Err(parse_error("duplicate slide relationship attribute"));
            }
            id = Some(
                attribute
                    .unescape_value()
                    .map_err(|e| parse_error(e.to_string()))?
                    .into_owned(),
            );
        }
    }
    id.filter(|s| !s.is_empty())
        .ok_or_else(|| parse_error("missing slide relationship id"))
}

fn parse_slide_ids(xml: &str) -> Result<Vec<String>, PptxError> {
    let mut ids = Vec::new();
    let mut slide_ids = HashSet::new();
    let mut relation_ids = HashSet::new();
    let mut seen_list = false;
    let mut in_list = false;
    read_xml(
        xml,
        "presentation",
        PRESENTATION_NS,
        |event, reader, parents| {
            if let Event::Start(e) | Event::Empty(e) = event {
                if element(reader, e.name(), "sldIdLst", PRESENTATION_NS)
                    && parents == ["presentation"]
                {
                    if seen_list {
                        return Err(parse_error("duplicate slide list"));
                    }
                    seen_list = true;
                    in_list = matches!(event, Event::Start(_));
                }
                if in_list
                    && element(reader, e.name(), "sldId", PRESENTATION_NS)
                    && parents == ["presentation", "sldIdLst"]
                {
                    let slide_id = required_attr(e, "id")?
                        .parse::<u32>()
                        .map_err(|_| parse_error("invalid numeric slide id"))?;
                    let id = relationship_id(e, reader)?;
                    if !slide_ids.insert(slide_id) || !relation_ids.insert(id.clone()) {
                        return Err(parse_error(
                            "duplicate slide id or relationship in slide list",
                        ));
                    }
                    ids.push(id);
                }
            }
            if let Event::End(e) = event {
                if parents == ["presentation", "sldIdLst"]
                    && element(reader, e.name(), "sldIdLst", PRESENTATION_NS)
                {
                    in_list = false;
                }
            }
            Ok(())
        },
    )?;
    Ok(ids)
}

struct Relationship {
    kind: String,
    target: String,
    external: bool,
}

fn parse_relationships(xml: &str) -> Result<HashMap<String, Relationship>, PptxError> {
    let mut relationships = HashMap::new();
    read_xml(
        xml,
        "Relationships",
        PACKAGE_NS,
        |event, reader, parents| {
            if let Event::Start(e) | Event::Empty(e) = event {
                if parents == ["Relationships"]
                    && element(reader, e.name(), "Relationship", PACKAGE_NS)
                {
                    let id = required_attr(e, "Id")?;
                    let relationship = Relationship {
                        kind: required_attr(e, "Type")?,
                        target: required_attr(e, "Target")?,
                        external: match attr(e, "TargetMode")?.as_deref() {
                            None | Some("Internal") => false,
                            Some("External") => true,
                            Some(_) => return Err(parse_error("invalid relationship TargetMode")),
                        },
                    };
                    if relationships.insert(id, relationship).is_some() {
                        return Err(parse_error("duplicate relationship Id"));
                    }
                }
            }
            Ok(())
        },
    )?;
    Ok(relationships)
}

// Resolve OPC part URIs relative to ppt/presentation.xml, including absolute
// package targets and dot segments. This is ZIP lookup only, never filesystem I/O.
fn slide_part_path(target: &str) -> Result<String, PptxError> {
    if target.contains(['\\', '?', '#', ':']) {
        return Err(parse_error(format!("invalid slide target {target:?}")));
    }
    let mut decoded = Vec::new();
    let bytes = target.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes
                .get(i + 1..i + 3)
                .ok_or_else(|| parse_error("invalid percent-encoded slide target"))?;
            let hex = std::str::from_utf8(hex)
                .map_err(|_| parse_error("invalid percent-encoded slide target"))?;
            let byte = u8::from_str_radix(hex, 16)
                .map_err(|_| parse_error("invalid percent-encoded slide target"))?;
            if matches!(byte, b'/' | b'\\' | 0) {
                return Err(parse_error("invalid encoded path separator"));
            }
            decoded.push(byte);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    let decoded =
        String::from_utf8(decoded).map_err(|_| parse_error("slide target is not UTF-8"))?;
    let mut parts = if decoded.starts_with('/') {
        Vec::new()
    } else {
        vec!["ppt"]
    };
    for part in decoded.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err(parse_error("slide target leaves package root"));
                }
            }
            part => parts.push(part),
        }
    }
    if parts.is_empty() {
        return Err(parse_error("empty slide target"));
    }
    Ok(parts.join("/"))
}

fn parse_slide_xml(xml: &str) -> Result<Slide, PptxError> {
    let mut shapes = Vec::new();
    let mut in_sp = false;
    let mut has_text = false;
    let mut in_t = false;
    let mut paragraphs = Vec::new();
    let mut para = String::new();
    read_xml(xml, "sld", PRESENTATION_NS, |event, reader, _| {
        match event {
            Event::Start(e) => {
                if element(reader, e.name(), "sp", PRESENTATION_NS) {
                    in_sp = true;
                    has_text = false;
                    paragraphs.clear();
                    para.clear();
                } else if in_sp && element(reader, e.name(), "txBody", PRESENTATION_NS) {
                    has_text = true;
                } else if in_sp && element(reader, e.name(), "t", DRAWING_NS) {
                    in_t = true;
                } else if in_sp && element(reader, e.name(), "p", DRAWING_NS) {
                    para.clear();
                } else if in_sp && element(reader, e.name(), "br", DRAWING_NS) {
                    para.push('\n');
                }
            }
            Event::Empty(e) => {
                if in_sp && element(reader, e.name(), "br", DRAWING_NS) {
                    para.push('\n');
                } else if in_sp && element(reader, e.name(), "p", DRAWING_NS) {
                    paragraphs.push(String::new());
                } else if in_sp && element(reader, e.name(), "txBody", PRESENTATION_NS) {
                    has_text = true;
                }
            }
            Event::Text(text) if in_t => {
                para.push_str(&text.unescape().map_err(|e| parse_error(e.to_string()))?)
            }
            Event::CData(text) if in_t => para.push_str(
                std::str::from_utf8(text.as_ref()).map_err(|e| parse_error(e.to_string()))?,
            ),
            Event::End(e) => {
                if element(reader, e.name(), "t", DRAWING_NS) {
                    in_t = false;
                } else if in_sp && element(reader, e.name(), "p", DRAWING_NS) {
                    paragraphs.push(std::mem::take(&mut para));
                } else if in_sp && element(reader, e.name(), "sp", PRESENTATION_NS) {
                    if has_text {
                        shapes.push(paragraphs.join("\n"));
                    }
                    in_sp = false;
                }
            }
            _ => {}
        }
        Ok(())
    })?;
    let title = shapes.first().cloned().unwrap_or_default();
    let body = shapes.get(1..).unwrap_or_default().join("\n\n");
    Ok(Slide::title_and_body(title, body))
}

fn parse_theme_accent(xml: &str) -> Result<Theme, PptxError> {
    let mut in_accent1 = false;
    let mut in_lt2 = false;
    let mut theme = Theme::light();
    theme.name = "Imported".into();
    read_xml(xml, "theme", DRAWING_NS, |event, reader, _| {
        match event {
            Event::Start(e) | Event::Empty(e) => {
                if element(reader, e.name(), "accent1", DRAWING_NS) {
                    in_accent1 = matches!(event, Event::Start(_));
                } else if element(reader, e.name(), "lt2", DRAWING_NS) {
                    in_lt2 = matches!(event, Event::Start(_));
                } else if element(reader, e.name(), "srgbClr", DRAWING_NS) {
                    if let Some(rgb) = attr(e, "val")?.and_then(|s| parse_hex_rgb(&s)) {
                        if in_accent1 {
                            theme.accent = rgb;
                        } else if in_lt2 {
                            theme.background = rgb;
                        }
                    }
                }
            }
            Event::End(e) => {
                if element(reader, e.name(), "accent1", DRAWING_NS) {
                    in_accent1 = false;
                } else if element(reader, e.name(), "lt2", DRAWING_NS) {
                    in_lt2 = false;
                }
            }
            _ => {}
        }
        Ok(())
    })?;
    Ok(theme)
}

fn parse_hex_rgb(s: &str) -> Option<[u8; 3]> {
    let s = s.trim();
    if s.len() != 6 || !s.is_ascii() {
        return None;
    }
    Some([
        u8::from_str_radix(&s[0..2], 16).ok()?,
        u8::from_str_radix(&s[2..4], 16).ok()?,
        u8::from_str_radix(&s[4..6], 16).ok()?,
    ])
}

fn extract_dc_title(xml: &str) -> Result<Option<String>, PptxError> {
    let mut in_title = false;
    let mut title = String::new();
    read_xml(xml, "coreProperties", CORE_NS, |event, reader, _| {
        match event {
            Event::Start(e) if element(reader, e.name(), "title", DC_NS) => in_title = true,
            Event::End(e) if element(reader, e.name(), "title", DC_NS) => in_title = false,
            Event::Text(text) if in_title => {
                title.push_str(&text.unescape().map_err(|e| parse_error(e.to_string()))?)
            }
            Event::CData(text) if in_title => title.push_str(
                std::str::from_utf8(text.as_ref()).map_err(|e| parse_error(e.to_string()))?,
            ),
            _ => {}
        }
        Ok(())
    })?;
    Ok((!title.is_empty()).then_some(title))
}

fn local_name(name: QName<'_>) -> String {
    String::from_utf8_lossy(name.local_name().as_ref()).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::{write::FileOptions, ZipWriter};

    fn package(changes: &[(&str, Option<&[u8]>)]) -> Vec<u8> {
        let original = crate::write_pptx_bytes(&Presentation::demo()).unwrap();
        let mut archive = ZipArchive::new(Cursor::new(original)).unwrap();
        let names: HashSet<String> = archive.file_names().map(str::to_owned).collect();
        let mut output = ZipWriter::new(Cursor::new(Vec::new()));
        for index in 0..archive.len() {
            let mut part = archive.by_index(index).unwrap();
            let name = part.name().to_owned();
            let mut data = Vec::new();
            part.read_to_end(&mut data).unwrap();
            let replacement = changes.iter().find(|(path, _)| *path == name);
            if let Some((_, None)) = replacement {
                continue;
            }
            output.start_file(&name, FileOptions::default()).unwrap();
            output
                .write_all(replacement.and_then(|(_, bytes)| *bytes).unwrap_or(&data))
                .unwrap();
        }
        for (path, bytes) in changes {
            if let Some(bytes) = bytes.filter(|_| !names.contains(*path)) {
                output.start_file(*path, FileOptions::default()).unwrap();
                output.write_all(bytes).unwrap();
            }
        }
        output.finish().unwrap().into_inner()
    }

    fn original_part(path: &str) -> String {
        let bytes = crate::write_pptx_bytes(&Presentation::demo()).unwrap();
        read_zip_string(&mut ZipArchive::new(Cursor::new(bytes)).unwrap(), path).unwrap()
    }

    #[test]
    fn slide_order_comes_from_presentation_not_relationship_ids() {
        let path = "ppt/presentation.xml";
        let xml = original_part(path).replace(
            r#"<p:sldId id="257" r:id="rId1"/><p:sldId id="258" r:id="rId2"/><p:sldId id="259" r:id="rId3"/>"#,
            r#"<p:sldId id="259" r:id="rId3"/><p:sldId id="257" r:id="rId1"/>"#,
        );
        let loaded = load_pptx_bytes(&package(&[(path, Some(xml.as_bytes()))])).unwrap();
        assert_eq!(loaded.slides.len(), 2);
        assert_eq!(loaded.slides[0].title.text, "Thank you");
        assert_eq!(loaded.slides[1].title.text, "rust-office Impress");
    }

    #[test]
    fn missing_listed_slide_rejects_the_entire_import() {
        let result = load_pptx_bytes(&package(&[("ppt/slides/slide2.xml", None)]));
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("ppt/slides/slide2.xml"));
    }

    #[test]
    fn malformed_slide_rejects_partial_text_instead_of_returning_success() {
        for xml in [
            r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld>"#,
            r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:sp></p:cSld></p:sld>"#,
        ] {
            assert!(
                load_pptx_bytes(&package(&[("ppt/slides/slide2.xml", Some(xml.as_bytes()))]))
                    .is_err()
            );
        }
    }

    #[test]
    fn missing_required_parts_never_fall_back_to_zip_directory_slides() {
        for path in ["ppt/presentation.xml", "ppt/_rels/presentation.xml.rels"] {
            let error = load_pptx_bytes(&package(&[(path, None)])).unwrap_err();
            assert!(error.to_string().contains(path));
        }
    }

    #[test]
    fn relationship_ids_use_namespace_and_unescape_without_numeric_ordering() {
        let manifest = original_part("ppt/presentation.xml")
            .replace("xmlns:r=", "xmlns:link=")
            .replace("r:id=", "link:id=")
            .replace("rId1", "first&amp;slide")
            .replace("rId2", "next");
        let rels = original_part("ppt/_rels/presentation.xml.rels")
            .replace("rId1", "first&amp;slide")
            .replace("rId2", "next");
        let loaded = load_pptx_bytes(&package(&[
            ("ppt/presentation.xml", Some(manifest.as_bytes())),
            ("ppt/_rels/presentation.xml.rels", Some(rels.as_bytes())),
        ]))
        .unwrap();
        assert_eq!(loaded.slides, Presentation::demo().slides);
    }

    #[test]
    fn invalid_slide_relationships_are_reported_with_the_part_name() {
        let path = "ppt/_rels/presentation.xml.rels";
        let original = original_part(path);
        for xml in [
            original.replace("Id=\"rId1\"", "Id=\"unlisted\""),
            original.replace(
                "Target=\"slides/slide1.xml\"",
                "TargetMode=\"External\" Target=\"slides/slide1.xml\"",
            ),
            original.replace("/relationships/slide\"", "/relationships/slideLayout\""),
            original.replace("Target=\"slides/slide1.xml\"", ""),
            original.replace("Id=\"rId2\"", "Id=\"rId1\""),
        ] {
            let error = load_pptx_bytes(&package(&[(path, Some(xml.as_bytes()))])).unwrap_err();
            assert!(error.to_string().contains(path), "{error}");
        }
    }

    #[test]
    fn package_targets_accept_absolute_relative_dot_and_encoded_names() {
        let path = "ppt/_rels/presentation.xml.rels";
        let original = original_part(path);
        for target in [
            "/ppt/slides/slide1.xml",
            "../ppt/slides/./slide1.xml",
            "slides/%73lide1.xml",
        ] {
            let rels = original.replace("slides/slide1.xml", target);
            let loaded = load_pptx_bytes(&package(&[(path, Some(rels.as_bytes()))])).unwrap();
            assert_eq!(loaded.slides, Presentation::demo().slides, "{target}");
        }
        for target in [
            "../../slides/slide1.xml",
            "http://example.com/slide.xml",
            "slides/%XX.xml",
            "slides/slide1.xml#fragment",
            "slides/%2Fslide1.xml",
        ] {
            let rels = original.replace("slides/slide1.xml", target);
            assert!(
                load_pptx_bytes(&package(&[(path, Some(rels.as_bytes()))])).is_err(),
                "{target}"
            );
        }
    }

    #[test]
    fn malformed_xml_attributes_entities_roots_and_trailing_content_fail() {
        let slide_path = "ppt/slides/slide1.xml";
        let slide = original_part(slide_path);
        for xml in [
            String::new(),
            slide.replace("rust-office Impress", "bad &unknown; text"),
            format!("{slide}<extra/>"),
            slide.replace("name=\"Title\"", "name=\"Title\" name=\"duplicate\""),
            slide
                .replace("<p:sld ", "<p:wrong ")
                .replace("</p:sld>", "</p:wrong>"),
            slide.replace(
                "xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\"",
                "xmlns:p=\"urn:wrong\"",
            ),
        ] {
            let error =
                load_pptx_bytes(&package(&[(slide_path, Some(xml.as_bytes()))])).unwrap_err();
            assert!(error.to_string().contains(slide_path));
        }
        let manifest_path = "ppt/presentation.xml";
        let manifest = original_part(manifest_path);
        for xml in [
            manifest.replace("r:id=\"rId1\"", ""),
            manifest.replace("id=\"258\"", "id=\"257\""),
            manifest.replace("r:id=\"rId2\"", "r:id=\"rId1\""),
            manifest.replace("</p:presentation>", ""),
        ] {
            let error =
                load_pptx_bytes(&package(&[(manifest_path, Some(xml.as_bytes()))])).unwrap_err();
            assert!(error.to_string().contains(manifest_path));
        }
    }

    #[test]
    fn optional_parts_default_only_when_absent_and_bad_reads_are_errors() {
        let loaded = load_pptx_bytes(&package(&[("ppt/theme/theme1.xml", None)])).unwrap();
        assert_eq!(loaded.theme, Theme::light());
        assert_eq!(loaded.title, "Imported presentation");
        for (path, bad) in [
            ("ppt/presentation.xml", b"\xff".as_slice()),
            (
                "ppt/_rels/presentation.xml.rels",
                b"<Relationships>".as_slice(),
            ),
            ("ppt/slides/slide2.xml", b"\xff".as_slice()),
            ("ppt/theme/theme1.xml", b"\xff".as_slice()),
            ("ppt/theme/theme1.xml", b"<theme>".as_slice()),
            ("docProps/core.xml", b"<coreProperties>".as_slice()),
        ] {
            let error = load_pptx_bytes(&package(&[(path, Some(bad))])).unwrap_err();
            assert!(error.to_string().contains(path), "{error}");
        }
        let core = r#"<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>日本語 &amp; <![CDATA[<title>]]></dc:title></cp:coreProperties>"#;
        let loaded =
            load_pptx_bytes(&package(&[("docProps/core.xml", Some(core.as_bytes()))])).unwrap();
        assert_eq!(loaded.title, "日本語 & <title>");
        let truncated = core.replace("</cp:coreProperties>", "");
        assert!(load_pptx_bytes(&package(&[(
            "docProps/core.xml",
            Some(truncated.as_bytes())
        )]))
        .is_err());
    }

    #[test]
    fn valid_empty_slide_list_ignores_orphans_and_opens_one_blank_slide() {
        let xml = r#"<p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:x="urn:extension"><p:sldIdLst/><x:sldIdLst><p:sldId id="257" r:id="rId1"/></x:sldIdLst></p:presentation>"#;
        let loaded = load_pptx_bytes(&package(&[
            ("ppt/presentation.xml", Some(xml.as_bytes())),
            ("ppt/_rels/presentation.xml.rels", None),
            ("ppt/slides/slide2.xml", Some(b"broken orphan")),
        ]))
        .unwrap();
        assert_eq!(loaded.slides, vec![Slide::blank()]);
        assert_eq!(loaded.active, 0);
        assert!(!loaded.is_dirty());
    }

    #[test]
    fn blank_titles_blank_paragraphs_and_unicode_text_round_trip() {
        let mut original = Presentation::new();
        original.slides = vec![
            Slide::title_and_body("", "日本語 & <tag>\n\nlast\n"),
            Slide::blank(),
        ];
        let loaded = load_pptx_bytes(&crate::write_pptx_bytes(&original).unwrap()).unwrap();
        assert_eq!(loaded.slides, original.slides);
        let slide = original_part("ppt/slides/slide1.xml")
            .replace("rust-office Impress", "<![CDATA[日本語 <&>]]>");
        let loaded = load_pptx_bytes(&package(&[(
            "ppt/slides/slide1.xml",
            Some(slide.as_bytes()),
        )]))
        .unwrap();
        assert_eq!(loaded.slides[0].title.text, "日本語 <&>");
    }

    #[test]
    fn strict_namespaces_and_non_ascii_theme_colors_do_not_panic() {
        let paths = [
            "ppt/presentation.xml",
            "ppt/_rels/presentation.xml.rels",
            "ppt/slides/slide1.xml",
            "ppt/slides/slide2.xml",
            "ppt/slides/slide3.xml",
            "ppt/theme/theme1.xml",
        ];
        let replacements: Vec<String> = paths
            .iter()
            .map(|path| {
                original_part(path)
                    .replace(PRESENTATION_NS[0], PRESENTATION_NS[1])
                    .replace(DRAWING_NS[0], DRAWING_NS[1])
                    .replace(REL_NS[0], REL_NS[1])
            })
            .collect();
        let changes: Vec<(&str, Option<&[u8]>)> = paths
            .iter()
            .zip(&replacements)
            .map(|(p, xml)| (*p, Some(xml.as_bytes())))
            .collect();
        let loaded = load_pptx_bytes(&package(&changes)).unwrap();
        assert_eq!(loaded.slides, Presentation::demo().slides);
        assert_eq!(parse_hex_rgb("ああ"), None);
        let theme = original_part("ppt/theme/theme1.xml").replace("val=\"008CA0\"", "val=\"ああ\"");
        assert!(theme.contains("ああ"));
        assert!(load_pptx_bytes(&package(&[(
            "ppt/theme/theme1.xml",
            Some(theme.as_bytes())
        )]))
        .is_ok());
    }
}
