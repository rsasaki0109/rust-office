//! Minimal PPTX (OOXML) reader — title/body text per slide.

use std::io::{Cursor, Read};
use std::path::Path;

use quick_xml::events::Event;
use quick_xml::name::QName;
use quick_xml::reader::Reader;
use zip::ZipArchive;

use crate::model::{Presentation, Slide};
use crate::pptx::PptxError;
use crate::theme::Theme;

/// Load a presentation from a `.pptx` path.
pub fn load_pptx_path(path: &Path) -> Result<Presentation, PptxError> {
    let bytes = std::fs::read(path)?;
    load_pptx_bytes(&bytes)
}

/// Load a presentation from PPTX package bytes.
pub fn load_pptx_bytes(bytes: &[u8]) -> Result<Presentation, PptxError> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))?;

    let rels = read_zip_string(&mut archive, "ppt/_rels/presentation.xml.rels").unwrap_or_default();
    let mut slide_paths = parse_slide_targets(&rels);
    if slide_paths.is_empty() {
        let mut names: Vec<String> = Vec::new();
        for i in 0..archive.len() {
            if let Ok(file) = archive.by_index(i) {
                let name = file.name().to_string();
                if name.starts_with("ppt/slides/slide") && name.ends_with(".xml") {
                    names.push(name);
                }
            }
        }
        names.sort_by_key(|a| human_slide_order(a));
        slide_paths = names;
    }

    let mut slides = Vec::new();
    for path in &slide_paths {
        let xml = read_zip_string(&mut archive, path).unwrap_or_default();
        slides.push(parse_slide_xml(&xml)?);
    }
    if slides.is_empty() {
        slides.push(Slide::blank());
    }

    let theme = read_zip_string(&mut archive, "ppt/theme/theme1.xml")
        .ok()
        .and_then(|xml| parse_theme_accent(&xml))
        .unwrap_or_else(Theme::light);

    let title = read_zip_string(&mut archive, "docProps/core.xml")
        .ok()
        .and_then(|xml| extract_dc_title(&xml))
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

fn human_slide_order(path: &str) -> u32 {
    path.chars()
        .filter(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .unwrap_or(0)
}

fn read_zip_string<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
    path: &str,
) -> Result<String, PptxError> {
    let mut file = archive
        .by_name(path)
        .map_err(|_| PptxError::Parse(format!("missing {path}")))?;
    let mut s = String::new();
    file.read_to_string(&mut s)?;
    Ok(s)
}

fn parse_slide_targets(rels_xml: &str) -> Vec<String> {
    let mut pairs: Vec<(u32, String)> = Vec::new();
    let mut reader = Reader::from_str(rels_xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(e)) | Ok(Event::Start(e)) => {
                if local_name(e.name()) == "Relationship" {
                    let ty = attr(&e, "Type").unwrap_or_default();
                    let target = attr(&e, "Target");
                    let id = attr(&e, "Id").unwrap_or_default();
                    if ty.contains("/slide") && !ty.contains("slideLayout") && !ty.contains("slideMaster")
                    {
                        if let Some(target) = target {
                            let order = id
                                .trim_start_matches("rId")
                                .parse::<u32>()
                                .unwrap_or(pairs.len() as u32);
                            let path = if target.starts_with("ppt/") {
                                target
                            } else if let Some(rest) = target.strip_prefix('/') {
                                rest.to_string()
                            } else {
                                format!("ppt/{target}")
                            };
                            pairs.push((order, path));
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
    pairs.sort_by_key(|(o, _)| *o);
    pairs.into_iter().map(|(_, p)| p).collect()
}

fn parse_slide_xml(xml: &str) -> Result<Slide, PptxError> {
    let shapes = collect_shape_texts(xml);
    let title = shapes.first().cloned().unwrap_or_default();
    let body = if shapes.len() > 1 {
        shapes[1..].join("\n\n")
    } else {
        String::new()
    };
    Ok(Slide::title_and_body(title, body))
}

fn collect_shape_texts(xml: &str) -> Vec<String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut shapes: Vec<String> = Vec::new();
    let mut in_sp = false;
    let mut in_t = false;
    let mut shape_paras: Vec<String> = Vec::new();
    let mut para = String::new();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let local = local_name(e.name());
                match local.as_str() {
                    "sp" => {
                        in_sp = true;
                        shape_paras.clear();
                        para.clear();
                    }
                    "t" if in_sp => in_t = true,
                    "p" if in_sp => {
                        para.clear();
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(e)) => {
                if local_name(e.name()) == "br" && in_sp {
                    para.push('\n');
                }
            }
            Ok(Event::Text(t)) if in_t => {
                para.push_str(&t.unescape().unwrap_or_default());
            }
            Ok(Event::End(e)) => {
                let local = local_name(e.name());
                match local.as_str() {
                    "t" => in_t = false,
                    "p" if in_sp => {
                        shape_paras.push(std::mem::take(&mut para));
                    }
                    "sp" if in_sp => {
                        let text = shape_paras
                            .iter()
                            .map(|s| s.as_str())
                            .filter(|s| !s.is_empty())
                            .collect::<Vec<_>>()
                            .join("\n");
                        if !text.is_empty() {
                            shapes.push(text);
                        }
                        in_sp = false;
                        shape_paras.clear();
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    shapes
}

fn parse_theme_accent(xml: &str) -> Option<Theme> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut in_accent1 = false;
    let mut in_lt2 = false;
    let mut buf = Vec::new();
    let mut accent: Option<[u8; 3]> = None;
    let mut lt2: Option<[u8; 3]> = None;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let local = local_name(e.name());
                if local == "accent1" {
                    in_accent1 = true;
                } else if local == "lt2" {
                    in_lt2 = true;
                } else if local == "srgbClr" {
                    if let Some(val) = attr(&e, "val") {
                        if let Some(rgb) = parse_hex_rgb(&val) {
                            if in_accent1 && accent.is_none() {
                                accent = Some(rgb);
                            } else if in_lt2 && lt2.is_none() {
                                lt2 = Some(rgb);
                            }
                        }
                    }
                }
            }
            Ok(Event::End(e)) => {
                let local = local_name(e.name());
                if local == "accent1" {
                    in_accent1 = false;
                } else if local == "lt2" {
                    in_lt2 = false;
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    let mut theme = Theme::light();
    theme.name = "Imported".into();
    if let Some(a) = accent {
        theme.accent = a;
    }
    if let Some(bg) = lt2 {
        theme.background = bg;
    }
    Some(theme)
}

fn parse_hex_rgb(s: &str) -> Option<[u8; 3]> {
    let s = s.trim();
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some([r, g, b])
}

fn extract_dc_title(xml: &str) -> Option<String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut in_title = false;
    let mut title = String::new();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                if local_name(e.name()) == "title" {
                    in_title = true;
                }
            }
            Ok(Event::Text(t)) if in_title => {
                title.push_str(&t.unescape().unwrap_or_default());
            }
            Ok(Event::End(e)) => {
                if local_name(e.name()) == "title" {
                    break;
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    if title.is_empty() {
        None
    } else {
        Some(title)
    }
}

fn local_name(name: QName<'_>) -> String {
    String::from_utf8_lossy(name.local_name().as_ref()).into_owned()
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
