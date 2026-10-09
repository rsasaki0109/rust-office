//! Editable OOXML for the rust-office basic slide profile.
//! No native JSON is embedded: geometry, text and images come from OOXML parts.
use std::{
    collections::HashMap,
    io::{Cursor, Read, Seek},
    sync::Arc,
};

use quick_xml::{events::Event, name::ResolveResult};
use zip::ZipArchive;

use crate::pptx_read::{read_xml, DRAWING_NS, PRESENTATION_NS, REL_NS};
use crate::{Bounds, ObjectKind, PptxError, ShapeKind, Slide, SlideObject, TextBox, Theme};

const PROFILE: &str = "rust-office:basic-v1";
const IMAGE_LIMIT: u64 = 8 * 1024 * 1024;
const P: u8 = 1;
const A: u8 = 2;
const R: u8 = 3;

fn error(s: impl Into<String>) -> PptxError {
    PptxError::Parse(s.into())
}
pub(super) fn escaped(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
        .replace('\r', "&#13;")
}
pub(super) fn valid_text(s: &str) -> Result<(), PptxError> {
    if s.chars().all(
        |c| matches!(c as u32, 9 | 10 | 13 | 0x20..=0xD7FF | 0xE000..=0xFFFD | 0x10000..=0x10FFFF),
    ) {
        Ok(())
    } else {
        Err(error("Text contains a character that XML cannot represent"))
    }
}
fn rgb(c: [u8; 3]) -> String {
    format!("{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}
fn emu(v: f32, extent: i64) -> i64 {
    (v as f64 * extent as f64).round() as i64
}
// One point is 12,700 EMUs; the basic profile uses 960 × 540 pt.
const WIDTH: i64 = 12_192_000;
const HEIGHT: i64 = 6_858_000;
fn xfrm(b: Bounds) -> String {
    format!(
        r#"<a:xfrm><a:off x="{}" y="{}"/><a:ext cx="{}" cy="{}"/></a:xfrm>"#,
        emu(b.x, WIDTH),
        emu(b.y, HEIGHT),
        emu(b.w, WIDTH),
        emu(b.h, HEIGHT)
    )
}
fn textbox_bounds(t: &TextBox) -> Bounds {
    Bounds {
        x: t.x,
        y: t.y,
        w: t.w,
        h: t.h,
    }
}
fn paragraphs(text: &str, font: f32, color: [u8; 3], bold: bool) -> String {
    text.split('\n').map(|line| format!(r#"<a:p><a:r><a:rPr lang="en-US" sz="{}"{}><a:solidFill><a:srgbClr val="{}"/></a:solidFill></a:rPr><a:t xml:space="preserve">{}</a:t></a:r></a:p>"#,
        (font as f64*100.0).round() as u32,if bold {" b=\"1\""} else {""},rgb(color),escaped(line))).collect()
}
fn text_xml(
    id: usize,
    name: &str,
    bounds: Bounds,
    text: &str,
    font: f32,
    color: [u8; 3],
    bold: bool,
) -> String {
    format!(
        r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="{name}"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr>{}<a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:noFill/><a:ln><a:noFill/></a:ln></p:spPr><p:txBody><a:bodyPr lIns="0" tIns="0" rIns="0" bIns="0" wrap="square" anchor="t"/><a:lstStyle/>{}</p:txBody></p:sp>"#,
        xfrm(bounds),
        paragraphs(text, font, color, bold)
    )
}

pub(super) struct Media {
    pub path: String,
    pub data: Vec<u8>,
}
pub(super) struct SlideParts {
    pub xml: String,
    pub rels: String,
    pub media: Vec<Media>,
}

pub(super) fn slide_parts(
    slide: &Slide,
    theme: &Theme,
    index: usize,
    budget: &mut crate::pptx_limits::OutputBudget,
) -> Result<SlideParts, PptxError> {
    for t in [&slide.title, &slide.body] {
        if !textbox_bounds(t).is_valid() {
            return Err(error("Text box position or size is outside the slide"));
        }
        valid_text(&t.text)?;
    }
    for font in [theme.title_font_pt, theme.body_font_pt] {
        if !font.is_finite() || !(1.0..=200.0).contains(&font) {
            return Err(error("Invalid theme font size"));
        }
    }
    let mut objects = text_xml(
        2,
        "Title",
        textbox_bounds(&slide.title),
        &slide.title.text,
        theme.title_font_pt,
        theme.title_color,
        true,
    );
    objects.push_str(&text_xml(
        3,
        "Body",
        textbox_bounds(&slide.body),
        &slide.body.text,
        theme.body_font_pt,
        theme.body_color,
        false,
    ));
    let mut media = Vec::new();
    let mut rels = String::from(
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" Target="../slideLayouts/slideLayout1.xml"/>"#,
    );
    for (i, object) in slide.objects.iter().enumerate() {
        object.validate().map_err(error)?;
        let id = i + 4;
        match &object.kind {
            ObjectKind::Text {
                text,
                font_pt,
                color,
            } => {
                valid_text(text)?;
                objects.push_str(&text_xml(
                    id,
                    "rust-office:text",
                    object.bounds,
                    text,
                    *font_pt,
                    *color,
                    false,
                ));
            }
            ObjectKind::Shape { shape, fill } => {
                let kind = match shape {
                    ShapeKind::Rectangle => "rect",
                    ShapeKind::Ellipse => "ellipse",
                };
                objects.push_str(&format!(r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="rust-office:{kind}"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr>{}<a:prstGeom prst="{kind}"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="{}"/></a:solidFill><a:ln><a:noFill/></a:ln></p:spPr></p:sp>"#,xfrm(object.bounds),rgb(*fill)));
            }
            ObjectKind::Image { data } => {
                let (extension, bytes) =
                    match image::guess_format(data).map_err(|e| error(e.to_string()))? {
                        image::ImageFormat::Png => ("png", data.as_ref().clone()),
                        image::ImageFormat::Jpeg => ("jpeg", data.as_ref().clone()),
                        _ => {
                            let decoded = crate::decode_image(data).map_err(error)?;
                            let mut output = Cursor::new(Vec::new());
                            decoded
                                .write_to(&mut output, image::ImageFormat::Png)
                                .map_err(|e| error(e.to_string()))?;
                            ("png", output.into_inner())
                        }
                    };
                if bytes.len() as u64 > IMAGE_LIMIT {
                    return Err(error("Converted PNG exceeds the 8 MiB image limit"));
                }
                budget.add(bytes.len())?;
                let name = format!("slide{index}-object{id}.{extension}");
                let rid = format!("rId{}", media.len() + 2);
                rels.push_str(&format!(r#"<Relationship Id="{rid}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/{name}"/>"#));
                objects.push_str(&format!(r#"<p:pic><p:nvPicPr><p:cNvPr id="{id}" name="rust-office:image"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="{rid}"/><a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr>{}<a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr></p:pic>"#,xfrm(object.bounds)));
                media.push(Media {
                    path: format!("ppt/media/{name}"),
                    data: bytes,
                });
            }
        }
    }
    valid_text(&slide.notes)?;
    let notes_id = media.len() + 2;
    rels.push_str(&format!(r#"<Relationship Id="rId{notes_id}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesSlide" Target="../notesSlides/notesSlide{index}.xml"/>"#));
    let note_shape = text_xml(
        2,
        "Notes",
        Bounds::default(),
        &slide.notes,
        12.0,
        [0, 0, 0],
        false,
    )
    .replace(
        "<p:nvPr/>",
        r#"<p:nvPr><p:ph type="body" idx="1"/></p:nvPr>"#,
    );
    let notes = format!(
        r#"<p:notes xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><p:cSld name="{PROFILE}"><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>{note_shape}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:notes>"#
    );
    let notes_size = notes.len();
    media.push(Media {
        path: format!("ppt/notesSlides/notesSlide{index}.xml"),
        data: notes.into_bytes(),
    });
    let notes_rels = format!(
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="../slides/slide{index}.xml"/></Relationships>"#
    );
    media.push(Media {
        path: format!("ppt/notesSlides/_rels/notesSlide{index}.xml.rels"),
        data: notes_rels.into_bytes(),
    });
    rels.push_str("</Relationships>");
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld name="{PROFILE}"><p:bg><p:bgPr><a:solidFill><a:srgbClr val="{}"/></a:solidFill><a:effectLst/></p:bgPr></p:bg><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr>{objects}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>"#,
        rgb(theme.background)
    );
    if xml.len() > 8 * 1024 * 1024 || notes_size > 8 * 1024 * 1024 || rels.len() > 8 * 1024 * 1024 {
        return Err(error("PPTX XML output exceeds the 8 MiB part limit"));
    }
    budget.add(xml.len())?;
    budget.add(rels.len())?;
    for file in media
        .iter()
        .filter(|file| file.path.ends_with(".xml") || file.path.ends_with(".rels"))
    {
        budget.add(file.data.len())?;
    }
    Ok(SlideParts { xml, rels, media })
}

#[derive(Debug)]
struct Node {
    ns: u8,
    name: String,
    attrs: HashMap<(u8, String), String>,
    children: Vec<Node>,
    text: String,
}
fn ns_kind(ns: ResolveResult<'_>) -> u8 {
    match ns {
        ResolveResult::Unbound => 0,
        ResolveResult::Bound(ns) if ns.as_ref() == b"http://www.w3.org/XML/1998/namespace" => 5,
        ResolveResult::Bound(ns) if PRESENTATION_NS.iter().any(|s| ns.as_ref() == s.as_bytes()) => {
            P
        }
        ResolveResult::Bound(ns) if DRAWING_NS.iter().any(|s| ns.as_ref() == s.as_bytes()) => A,
        ResolveResult::Bound(ns) if REL_NS.iter().any(|s| ns.as_ref() == s.as_bytes()) => R,
        _ => 4,
    }
}
impl Node {
    fn is(&self, ns: u8, name: &str) -> bool {
        self.ns == ns && self.name == name
    }
    fn attribute(&self, ns: u8, key: &str) -> Option<&str> {
        self.attrs.get(&(ns, key.to_owned())).map(String::as_str)
    }
    fn value(&self, key: &str) -> Result<&str, PptxError> {
        self.attribute(0, key)
            .ok_or_else(|| error(format!("missing {key} in {}", self.name)))
    }
    fn find(&self, ns: u8, name: &str) -> Result<&Node, PptxError> {
        let mut matching = self.children.iter().filter(|n| n.is(ns, name));
        let node = matching
            .next()
            .ok_or_else(|| error(format!("missing {name} in {}", self.name)))?;
        if matching.next().is_some() {
            return Err(error(format!("duplicate {name} in {}", self.name)));
        }
        Ok(node)
    }
    fn allowed_attrs(&self, names: &[(u8, &str)]) -> Result<(), PptxError> {
        for (ns, key) in self.attrs.keys() {
            if !names.iter().any(|(n, k)| *n == *ns && *k == key) {
                return Err(error(format!("Unsupported {} attribute {key}", self.name)));
            }
        }
        Ok(())
    }
    fn allowed(&self, names: &[(u8, &str)]) -> Result<(), PptxError> {
        for child in &self.children {
            if !names.iter().any(|(ns, name)| child.is(*ns, name)) {
                return Err(error(format!(
                    "unsupported slide object or property {}: importing it would lose content",
                    child.name
                )));
            }
        }
        Ok(())
    }
}
fn xml_tree(xml: &str, root_name: &str) -> Result<Node, PptxError> {
    let mut stack: Vec<Node> = Vec::new();
    let mut root = None;
    let mut count = 0usize;
    read_xml(xml, root_name, PRESENTATION_NS, |event, reader, _| {
        match event {
            Event::Start(e) | Event::Empty(e) => {
                count += 1;
                if stack.len() >= 128 || count > 100_000 {
                    return Err(error("Slide XML exceeds structural limits"));
                }
                let mut attrs = HashMap::new();
                for attr in e.attributes() {
                    let attr = attr.map_err(|e| error(e.to_string()))?;
                    let (ns, name) = reader.resolve_attribute(attr.key);
                    // Namespace declarations are not semantic slide attributes.
                    if attr.key.as_ref() == b"xmlns" || attr.key.as_ref().starts_with(b"xmlns:") {
                        continue;
                    }
                    attrs.insert(
                        (
                            ns_kind(ns),
                            String::from_utf8_lossy(name.as_ref()).into_owned(),
                        ),
                        attr.unescape_value()
                            .map_err(|e| error(e.to_string()))?
                            .into_owned(),
                    );
                }
                let (ns, name) = reader.resolve_element(e.name());
                let node = Node {
                    ns: ns_kind(ns),
                    name: String::from_utf8_lossy(name.as_ref()).into_owned(),
                    attrs,
                    children: Vec::new(),
                    text: String::new(),
                };
                if matches!(event, Event::Start(_)) {
                    stack.push(node);
                } else if let Some(parent) = stack.last_mut() {
                    parent.children.push(node);
                } else {
                    root = Some(node);
                }
            }
            Event::End(_) => {
                let node = stack
                    .pop()
                    .ok_or_else(|| error("Unexpected end of slide"))?;
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(node);
                } else {
                    root = Some(node);
                }
            }
            Event::Text(t) => {
                if let Some(node) = stack.last_mut() {
                    node.text
                        .push_str(&t.unescape().map_err(|e| error(e.to_string()))?);
                }
            }
            Event::CData(t) => {
                if let Some(node) = stack.last_mut() {
                    node.text.push_str(
                        std::str::from_utf8(t.as_ref()).map_err(|e| error(e.to_string()))?,
                    );
                }
            }
            _ => {}
        }
        Ok(())
    })?;
    root.ok_or_else(|| error("Missing slide root"))
}
fn number(node: &Node, key: &str) -> Result<i64, PptxError> {
    node.value(key)?
        .parse()
        .map_err(|_| error(format!("invalid {key} in {}", node.name)))
}
fn bounds(node: &Node) -> Result<Bounds, PptxError> {
    let transform = node.find(A, "xfrm")?;
    if transform
        .attrs
        .keys()
        .any(|(_, k)| matches!(k.as_str(), "rot" | "flipH" | "flipV"))
    {
        return Err(error("Rotated/flipped objects are unsupported"));
    }
    transform.allowed_attrs(&[])?;
    transform.allowed(&[(A, "off"), (A, "ext")])?;
    let off = transform.find(A, "off")?;
    let ext = transform.find(A, "ext")?;
    off.allowed_attrs(&[(0, "x"), (0, "y")])?;
    ext.allowed_attrs(&[(0, "cx"), (0, "cy")])?;
    let b = Bounds {
        x: number(off, "x")? as f32 / WIDTH as f32,
        y: number(off, "y")? as f32 / HEIGHT as f32,
        w: number(ext, "cx")? as f32 / WIDTH as f32,
        h: number(ext, "cy")? as f32 / HEIGHT as f32,
    };
    if !b.is_valid() {
        return Err(error("Object position or size is outside the slide"));
    }
    Ok(b)
}
fn color(node: &Node) -> Result<[u8; 3], PptxError> {
    node.allowed(&[(A, "srgbClr")])?;
    let c = node.find(A, "srgbClr")?;
    if !c.children.is_empty() {
        return Err(error("Color effects are unsupported"));
    }
    c.allowed_attrs(&[(0, "val")])?;
    let value = c.value("val")?;
    if value.len() != 6 || !value.is_ascii() {
        return Err(error("Invalid RGB color"));
    }
    Ok([
        u8::from_str_radix(&value[0..2], 16).map_err(|_| error("Invalid RGB color"))?,
        u8::from_str_radix(&value[2..4], 16).map_err(|_| error("Invalid RGB color"))?,
        u8::from_str_radix(&value[4..6], 16).map_err(|_| error("Invalid RGB color"))?,
    ])
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct TextStyle {
    pub font: f32,
    pub color: [u8; 3],
}
fn read_text(node: &Node, bold: bool) -> Result<(String, TextStyle), PptxError> {
    node.allowed(&[(A, "bodyPr"), (A, "lstStyle"), (A, "p")])?;
    let body = node.find(A, "bodyPr")?;
    body.allowed_attrs(&[
        (0, "lIns"),
        (0, "tIns"),
        (0, "rIns"),
        (0, "bIns"),
        (0, "wrap"),
        (0, "anchor"),
    ])?;
    for inset in ["lIns", "tIns", "rIns", "bIns"] {
        if body.attribute(0, inset).is_some_and(|v| v != "0") {
            return Err(error("Nonzero text insets are unsupported"));
        }
    }
    if body.attribute(0, "wrap").is_some_and(|v| v != "square") {
        return Err(error("Unsupported text wrapping"));
    }
    if !node.find(A, "lstStyle")?.children.is_empty() {
        return Err(error("Text list styles are unsupported"));
    }
    if !body.children.is_empty() {
        return Err(error("Text autofit or effects are unsupported"));
    }
    if body.attribute(0, "vert").is_some_and(|v| v != "horz")
        || body.attribute(0, "anchor").is_some_and(|v| v != "t")
    {
        return Err(error("Vertical or centered text is unsupported"));
    }
    let mut lines = Vec::new();
    let mut style = None;
    for para in node.children.iter().filter(|n| n.is(A, "p")) {
        para.allowed_attrs(&[])?;
        para.allowed(&[(A, "r")])?;
        let mut line = String::new();
        for run in &para.children {
            run.allowed_attrs(&[])?;
            run.allowed(&[(A, "rPr"), (A, "t")])?;
            let props = run.find(A, "rPr")?;
            props.allowed_attrs(&[(0, "lang"), (0, "sz"), (0, "b")])?;
            props.allowed(&[(A, "solidFill")])?;
            for (_, key) in props.attrs.keys() {
                if !matches!(key.as_str(), "lang" | "sz" | "b") {
                    return Err(error(format!("Unsupported text style {key}")));
                }
            }
            if props
                .attribute(0, "b")
                .is_some_and(|v| v != if bold { "1" } else { "0" })
            {
                return Err(error("Mixed bold text is unsupported"));
            }
            let font = number(props, "sz")? as f32 / 100.0;
            if !(1.0..=200.0).contains(&font) {
                return Err(error("Invalid text size"));
            }
            let current = TextStyle {
                font,
                color: color(props.find(A, "solidFill")?)?,
            };
            if style.is_some_and(|s| s != current) {
                return Err(error("Mixed text sizes/colors are unsupported"));
            }
            style = Some(current);
            let text = run.find(A, "t")?;
            text.allowed_attrs(&[(5, "space")])?;
            if !text.children.is_empty() {
                return Err(error("Nested text content is unsupported"));
            }
            line.push_str(&text.text);
        }
        lines.push(line);
    }
    let joined = lines.join("\n");
    valid_text(&joined)?;
    Ok((joined, style.ok_or_else(|| error("Missing text style"))?))
}
fn shape_properties(node: &Node, kind: &str, filled: bool) -> Result<Bounds, PptxError> {
    node.allowed(&[
        (A, "xfrm"),
        (A, "prstGeom"),
        (A, "solidFill"),
        (A, "noFill"),
        (A, "ln"),
    ])?;
    node.allowed_attrs(&[])?;
    let geometry = node.find(A, "prstGeom")?;
    geometry.allowed_attrs(&[(0, "prst")])?;
    if geometry.value("prst")? != kind {
        return Err(error("Unsupported shape geometry"));
    }
    geometry.allowed(&[(A, "avLst")])?;
    if !geometry.find(A, "avLst")?.children.is_empty() {
        return Err(error("Adjusted geometry is unsupported"));
    }
    if let Ok(line) = node.find(A, "ln") {
        line.allowed(&[(A, "noFill")])?;
        line.find(A, "noFill")?;
    }
    if !filled {
        node.find(A, "noFill")?;
        if node.children.iter().any(|n| n.is(A, "solidFill")) {
            return Err(error("Text box fills are unsupported"));
        }
    }
    bounds(node)
}

pub(super) struct ImportedSlide {
    pub slide: Slide,
    pub title: TextStyle,
    pub body: TextStyle,
    pub background: [u8; 3],
}
/// Only the explicit basic profile uses this reader; all other decks keep the
/// existing title/body reader and its unsupported-object guard.
pub(super) fn read_slide<Rd: Read + Seek>(
    xml: &str,
    path: &str,
    archive: &mut ZipArchive<Rd>,
    images: &mut HashMap<String, Arc<Vec<u8>>>,
) -> Result<Option<ImportedSlide>, PptxError> {
    let root = xml_tree(xml, "sld")?;
    let common = match root.find(P, "cSld") {
        Ok(n) => n,
        Err(_) => return Ok(None),
    };
    if common.attribute(0, "name") != Some(PROFILE) {
        return Ok(None);
    }
    root.allowed_attrs(&[])?;
    root.allowed(&[(P, "cSld"), (P, "clrMapOvr")])?;
    common.allowed_attrs(&[(0, "name")])?;
    common.allowed(&[(P, "bg"), (P, "spTree")])?;
    let bg = common.find(P, "bg")?;
    bg.allowed(&[(P, "bgPr")])?;
    let bgpr = bg.find(P, "bgPr")?;
    bgpr.allowed(&[(A, "solidFill"), (A, "effectLst")])?;
    if !bgpr.find(A, "effectLst")?.children.is_empty() {
        return Err(error("Background effects are unsupported"));
    }
    let background = color(bgpr.find(A, "solidFill")?)?;
    let tree = common.find(P, "spTree")?;
    tree.allowed(&[(P, "nvGrpSpPr"), (P, "grpSpPr"), (P, "sp"), (P, "pic")])?;
    // The root group carries no transformation in the basic profile.
    let group = tree.find(P, "grpSpPr")?;
    group.allowed(&[(A, "xfrm")])?;
    let transform = group.find(A, "xfrm")?;
    transform.allowed_attrs(&[])?;
    transform.allowed(&[(A, "off"), (A, "ext"), (A, "chOff"), (A, "chExt")])?;
    for (name, keys) in [
        ("off", ["x", "y"]),
        ("ext", ["cx", "cy"]),
        ("chOff", ["x", "y"]),
        ("chExt", ["cx", "cy"]),
    ] {
        let node = transform.find(A, name)?;
        for key in keys {
            if number(node, key)? != 0 {
                return Err(error("Transformed root groups are unsupported"));
            }
        }
    }
    let parent = path
        .rsplit_once('/')
        .ok_or_else(|| error("Invalid slide path"))?;
    let relpath = format!("{}/_rels/{}.rels", parent.0, parent.1);
    let rels = match crate::pptx_read::read_optional_zip_string(archive, &relpath)? {
        Some(xml) => crate::pptx_read::parse_relationships(&xml)
            .map_err(|e| error(format!("{relpath}: {e}")))?,
        None => HashMap::new(),
    };
    let mut slide = Slide::blank();
    let mut title = None;
    let mut body = None;
    let mut ids = std::collections::HashSet::new();
    for (ordinal, object) in tree
        .children
        .iter()
        .filter(|n| n.is(P, "sp") || n.is(P, "pic"))
        .enumerate()
    {
        let picture = object.is(P, "pic");
        object.allowed_attrs(&[])?;
        object.allowed(if picture {
            &[(P, "nvPicPr"), (P, "blipFill"), (P, "spPr")]
        } else {
            &[(P, "nvSpPr"), (P, "spPr"), (P, "txBody")]
        })?;
        let nv = object.find(P, if picture { "nvPicPr" } else { "nvSpPr" })?;
        nv.allowed(&[(P, "cNvPr"), (P, "cNvSpPr"), (P, "cNvPicPr"), (P, "nvPr")])?;
        if !nv.find(P, "nvPr")?.children.is_empty() {
            return Err(error("Object placeholders/extensions are unsupported"));
        }
        let props = nv.find(P, "cNvPr")?;
        props.allowed_attrs(&[(0, "id"), (0, "name")])?;
        if !props.children.is_empty() {
            return Err(error("Hyperlinks/effects are unsupported"));
        }
        let id = number(props, "id")?;
        if id <= 1 || !ids.insert(id) {
            return Err(error("Duplicate or invalid object id"));
        }
        let name = props.value("name")?;
        if (ordinal == 0 && name != "Title")
            || (ordinal == 1 && name != "Body")
            || (ordinal > 1 && matches!(name, "Title" | "Body"))
        {
            return Err(error(
                "Title and body must precede added objects in drawing order",
            ));
        }
        let sp = object.find(P, "spPr")?;
        if picture {
            sp.allowed(&[(A, "xfrm"), (A, "prstGeom")])?;
        }
        let bounds = shape_properties(
            sp,
            if name == "rust-office:ellipse" {
                "ellipse"
            } else {
                "rect"
            },
            matches!(name, "rust-office:rect" | "rust-office:ellipse") || picture,
        )?;
        let kind = match (picture, name) {
            (false, "Title" | "Body" | "rust-office:text") => {
                let (text, style) = read_text(object.find(P, "txBody")?, name == "Title")?;
                if name == "Title" || name == "Body" {
                    let textbox = TextBox {
                        text,
                        x: bounds.x,
                        y: bounds.y,
                        w: bounds.w,
                        h: bounds.h,
                    };
                    if name == "Title" {
                        if title.replace(style).is_some() {
                            return Err(error("Duplicate title"));
                        }
                        slide.title = textbox;
                    } else {
                        if body.replace(style).is_some() {
                            return Err(error("Duplicate body"));
                        }
                        slide.body = textbox;
                    }
                    continue;
                }
                ObjectKind::Text {
                    text,
                    font_pt: style.font,
                    color: style.color,
                }
            }
            (false, "rust-office:rect" | "rust-office:ellipse") => {
                if object.children.iter().any(|n| n.is(P, "txBody")) {
                    return Err(error("Text inside filled shapes is unsupported"));
                }
                ObjectKind::Shape {
                    shape: if name == "rust-office:rect" {
                        ShapeKind::Rectangle
                    } else {
                        ShapeKind::Ellipse
                    },
                    fill: color(sp.find(A, "solidFill")?)?,
                }
            }
            (true, "rust-office:image") => {
                let fill = object.find(P, "blipFill")?;
                fill.allowed(&[(A, "blip"), (A, "stretch")])?;
                let stretch = fill.find(A, "stretch")?;
                stretch.allowed(&[(A, "fillRect")])?;
                if !stretch.find(A, "fillRect")?.attrs.is_empty() {
                    return Err(error("Image crop is unsupported"));
                }
                let blip = fill.find(A, "blip")?;
                blip.allowed_attrs(&[(R, "embed")])?;
                if !blip.children.is_empty() || blip.attribute(R, "link").is_some() {
                    return Err(error("Linked images or image effects are unsupported"));
                }
                let rid = blip
                    .attribute(R, "embed")
                    .ok_or_else(|| error("Missing image relationship"))?;
                let rel = rels
                    .get(rid)
                    .ok_or_else(|| error(format!("{relpath}: missing image relationship {rid}")))?;
                if rel.external || !REL_NS.iter().any(|ns| rel.kind == format!("{ns}/image")) {
                    return Err(error("Image must use an internal image relationship"));
                }
                let target = crate::pptx_read::part_path(&rel.target, parent.0)?;
                let data = if let Some(data) = images.get(&target) {
                    data.clone()
                } else {
                    let file = archive
                        .by_name(&target)
                        .map_err(|e| error(format!("cannot read {target}: {e}")))?;
                    if file.size() > IMAGE_LIMIT {
                        return Err(error(format!("{target}: image exceeds 8 MiB limit")));
                    }
                    let mut bytes = Vec::new();
                    file.take(IMAGE_LIMIT + 1)
                        .read_to_end(&mut bytes)
                        .map_err(|e| error(format!("cannot read {target}: {e}")))?;
                    crate::decode_image(&bytes).map_err(|e| error(format!("{target}: {e}")))?;
                    let data = Arc::new(bytes);
                    images.insert(target, data.clone());
                    data
                };
                ObjectKind::Image { data }
            }
            _ => {
                return Err(error(format!(
                    "unsupported slide object {name}: importing it would lose content"
                )))
            }
        };
        slide.objects.push(SlideObject { bounds, kind });
    }
    // Read plain speaker notes from standard OOXML notesSlide relationships.
    let mut notes = rels
        .values()
        .filter(|r| REL_NS.iter().any(|ns| r.kind == format!("{ns}/notesSlide")));
    if let Some(rel) = notes.next() {
        if rel.external || notes.next().is_some() {
            return Err(error("Invalid notes relationship"));
        }
        let target = crate::pptx_read::part_path(&rel.target, parent.0)?;
        let xml = crate::pptx_read::read_zip_string(archive, &target)?;
        slide.notes = read_notes(&xml).map_err(|e| error(format!("{target}: {e}")))?;
    }
    Ok(Some(ImportedSlide {
        slide,
        title: title.ok_or_else(|| error("Missing title"))?,
        body: body.ok_or_else(|| error("Missing body"))?,
        background,
    }))
}

fn read_notes(xml: &str) -> Result<String, PptxError> {
    let notes = xml_tree(xml, "notes")?;
    notes.allowed(&[(P, "cSld"), (P, "clrMapOvr")])?;
    let common = notes.find(P, "cSld")?;
    common.allowed(&[(P, "spTree")])?;
    let tree = common.find(P, "spTree")?;
    tree.allowed(&[(P, "nvGrpSpPr"), (P, "grpSpPr"), (P, "sp")])?;
    let shape = tree.find(P, "sp")?;
    shape.allowed(&[(P, "nvSpPr"), (P, "spPr"), (P, "txBody")])?;
    read_text(shape.find(P, "txBody")?, false).map(|(text, _)| text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{load_pptx_bytes, write_pptx_bytes, write_pptx_path, Presentation};
    use image::GenericImageView;
    use std::io::Write;

    fn picture(format: image::ImageFormat) -> Vec<u8> {
        let pixels = image::RgbaImage::from_fn(4, 3, |x, y| {
            image::Rgba([x as u8 * 50, y as u8 * 80, 30, if x == 0 { 0 } else { 255 }])
        });
        let image = image::DynamicImage::ImageRgba8(pixels);
        let mut bytes = Cursor::new(Vec::new());
        let image = if format == image::ImageFormat::Jpeg {
            image::DynamicImage::ImageRgb8(image.to_rgb8())
        } else {
            image
        };
        image.write_to(&mut bytes, format).unwrap();
        bytes.into_inner()
    }
    fn deck() -> Presentation {
        let mut p = Presentation::demo();
        p.title = "日本語 & <deck>".into();
        p.slides[0].title.text = "\n日本語 & <title>\n".into();
        p.slides[0].body.text = "before\n\nafter\n".into();
        p.slides[0].title.x = 0.137;
        p.slides[0].body.w = 0.721;
        p.slides[0].notes = "日本語 notes & <tag>\n\nlast\n".into();
        p.slides[0].objects = vec![
            SlideObject {
                bounds: Bounds {
                    x: 0.137,
                    y: 0.213,
                    w: 0.414,
                    h: 0.178,
                },
                kind: ObjectKind::Text {
                    text: "  日本語 & <text>\n\nlast\n".into(),
                    font_pt: 23.75,
                    color: [13, 45, 90],
                },
            },
            SlideObject::shape(ShapeKind::Rectangle),
            SlideObject::shape(ShapeKind::Ellipse),
            SlideObject {
                bounds: Bounds::default(),
                kind: ObjectKind::Image {
                    data: Arc::new(picture(image::ImageFormat::Png)),
                },
            },
            SlideObject {
                bounds: Bounds::default(),
                kind: ObjectKind::Image {
                    data: Arc::new(picture(image::ImageFormat::Jpeg)),
                },
            },
        ];
        p.slides[2].objects = p.slides[0].objects.clone();
        p.slides.reverse();
        p.active = 2;
        p.mark_dirty();
        p
    }
    fn approx(a: Bounds, b: Bounds) {
        for (x, y) in [(a.x, b.x), (a.y, b.y), (a.w, b.w), (a.h, b.h)] {
            assert!((x - y).abs() < 0.000001, "{a:?} != {b:?}");
        }
    }
    fn compare(a: &Presentation, b: &Presentation) {
        assert_eq!(a.title, b.title);
        assert_eq!(a.theme, b.theme);
        assert_eq!(a.slides.len(), b.slides.len());
        for (a, b) in a.slides.iter().zip(&b.slides) {
            assert_eq!(a.title.text, b.title.text);
            assert_eq!(a.body.text, b.body.text);
            assert_eq!(a.notes, b.notes);
            approx(textbox_bounds(&a.title), textbox_bounds(&b.title));
            approx(textbox_bounds(&a.body), textbox_bounds(&b.body));
            assert_eq!(a.objects.len(), b.objects.len());
            for (a, b) in a.objects.iter().zip(&b.objects) {
                approx(a.bounds, b.bounds);
                assert_eq!(a.kind, b.kind);
            }
        }
        assert_eq!(b.active, 0);
        assert!(!b.is_dirty());
    }
    fn rewrite(bytes: &[u8], changes: &[(&str, Option<&[u8]>)]) -> Vec<u8> {
        let mut input = ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut output = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for i in 0..input.len() {
            let mut file = input.by_index(i).unwrap();
            let name = file.name().to_owned();
            let mut data = Vec::new();
            file.read_to_end(&mut data).unwrap();
            let replacement = changes.iter().find(|(n, _)| *n == name);
            if matches!(replacement, Some((_, None))) {
                continue;
            }
            output
                .start_file(
                    name,
                    zip::write::FileOptions::default()
                        .compression_method(zip::CompressionMethod::Deflated),
                )
                .unwrap();
            output
                .write_all(replacement.and_then(|(_, d)| *d).unwrap_or(&data))
                .unwrap();
        }
        output.finish().unwrap().into_inner()
    }
    fn part(bytes: &[u8], name: &str) -> String {
        crate::pptx_read::read_zip_string(&mut ZipArchive::new(Cursor::new(bytes)).unwrap(), name)
            .unwrap()
    }

    #[test]
    fn basic_objects_geometry_notes_unicode_and_themes_round_trip() {
        for theme in Theme::builtins() {
            let mut p = deck();
            p.theme = theme;
            p.theme.title_font_pt = 31.25;
            p.theme.body_font_pt = 18.75;
            let bytes = write_pptx_bytes(&p).unwrap();
            let loaded = load_pptx_bytes(&bytes).unwrap();
            compare(&p, &loaded);
            compare(
                &loaded,
                &load_pptx_bytes(&write_pptx_bytes(&loaded).unwrap()).unwrap(),
            );
        }
    }
    #[test]
    fn gif_bmp_webp_are_converted_to_embedded_png_without_pixel_loss() {
        for format in [
            image::ImageFormat::Bmp,
            image::ImageFormat::WebP,
            image::ImageFormat::Gif,
        ] {
            let data = picture(format);
            let pixels = crate::decode_image(&data).unwrap().to_rgba8();
            let mut p = Presentation::new();
            p.slides[0].objects.push(SlideObject {
                bounds: Bounds::default(),
                kind: ObjectKind::Image {
                    data: Arc::new(data),
                },
            });
            let loaded = load_pptx_bytes(&write_pptx_bytes(&p).unwrap()).unwrap();
            let ObjectKind::Image { data } = &loaded.slides[0].objects[0].kind else {
                panic!()
            };
            assert_eq!(image::guess_format(data).unwrap(), image::ImageFormat::Png);
            assert_eq!(crate::decode_image(data).unwrap().to_rgba8(), pixels);
        }
    }
    #[test]
    fn invalid_objects_text_and_theme_do_not_replace_existing_output() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("old.pptx");
        std::fs::write(&output, b"original").unwrap();
        let mut cases = Vec::new();
        let mut p = deck();
        p.slides[0].objects[0].bounds.w = -1.0;
        cases.push(p);
        let mut p = deck();
        p.slides[0].objects[0].kind = ObjectKind::Text {
            text: "bad\0text".into(),
            font_pt: 24.0,
            color: [0; 3],
        };
        cases.push(p);
        let mut p = deck();
        p.slides[0].objects[0].kind = ObjectKind::Image {
            data: Arc::new(b"corrupt image".to_vec()),
        };
        cases.push(p);
        let mut p = deck();
        p.slides[0].title.x = f32::NAN;
        cases.push(p);
        let mut p = deck();
        p.theme.title_font_pt = 201.0;
        cases.push(p);
        let mut p = deck();
        p.theme.name = "bad\0theme".into();
        cases.push(p);
        let mut p = deck();
        p.slides[0].notes = "bad\0notes".into();
        cases.push(p);
        for p in cases {
            assert!(write_pptx_path(&p, &output).is_err());
            assert_eq!(std::fs::read(&output).unwrap(), b"original");
        }
    }
    #[test]
    fn missing_corrupt_oversized_or_external_images_reject_whole_deck() {
        let bytes = write_pptx_bytes(&deck()).unwrap();
        let asset = "ppt/media/slide1-object7.png";
        for data in [None, Some(b"corrupt".as_slice())] {
            let error = load_pptx_bytes(&rewrite(&bytes, &[(asset, data)]))
                .unwrap_err()
                .to_string();
            assert!(error.contains(asset), "{error}");
            assert!(error.contains("ppt/slides/slide1.xml"), "{error}");
        }
        let oversized = vec![0; IMAGE_LIMIT as usize + 1];
        assert!(
            load_pptx_bytes(&rewrite(&bytes, &[(asset, Some(&oversized))]))
                .unwrap_err()
                .to_string()
                .contains("8 MiB")
        );
        let path = "ppt/slides/_rels/slide1.xml.rels";
        let rels = part(&bytes, path);
        for changed in [
            rels.replace("Id=\"rId2\"", "Id=\"missing\""),
            rels.replace(
                "Target=\"../media/slide1-object7.png\"",
                "TargetMode=\"External\" Target=\"https://example.com/image.png\"",
            ),
            rels.replace("relationships/image", "relationships/slide"),
            rels.replace("../media/slide1-object7.png", "../../../outside.png"),
        ] {
            assert!(
                load_pptx_bytes(&rewrite(&bytes, &[(path, Some(changed.as_bytes()))])).is_err()
            );
        }
    }
    #[test]
    fn unsupported_edits_in_basic_profile_are_rejected() {
        let bytes = write_pptx_bytes(&deck()).unwrap();
        let path = "ppt/slides/slide1.xml";
        let xml = part(&bytes, path);
        for changed in [
            xml.replace("<a:xfrm>", "<a:xfrm rot=\"60000\">"),
            xml.replace(
                "id=\"2\" name=\"Title\"",
                "id=\"2\" name=\"Title\" hidden=\"1\"",
            ),
            xml.replace("</p:sld>", "<p:transition/></p:sld>"),
            xml.replace("prst=\"ellipse\"", "prst=\"triangle\""),
            xml.replace("</a:prstGeom></p:spPr></p:pic>","</a:prstGeom><a:solidFill><a:srgbClr val=\"FFFFFF\"/></a:solidFill></p:spPr></p:pic>"),
            xml.replace("<a:stretch>", "<a:srcRect l=\"20000\"/><a:stretch>"),
            xml.replace("</p:spTree>", "<p:graphicFrame/></p:spTree>"),
            xml.replace("sz=\"2375\"", "sz=\"2375\" i=\"1\""),
            xml.replace(
                "<a:ln><a:noFill/></a:ln>",
                "<a:ln><a:solidFill><a:srgbClr val=\"FFFFFF\"/></a:solidFill></a:ln>",
            ),
        ] {
            let error =
                load_pptx_bytes(&rewrite(&bytes, &[(path, Some(changed.as_bytes()))])).unwrap_err();
            assert!(error.to_string().contains(path));
        }
    }
    #[test]
    fn invalid_notes_or_changed_slide_size_are_not_silently_discarded() {
        let bytes = write_pptx_bytes(&deck()).unwrap();
        let notes = "ppt/notesSlides/notesSlide1.xml";
        for data in [None, Some(b"<p:notes>".as_slice())] {
            let error = load_pptx_bytes(&rewrite(&bytes, &[(notes, data)])).unwrap_err();
            assert!(error.to_string().contains("ppt/slides/slide1.xml"));
        }
        assert!(
            load_pptx_bytes(&rewrite(&bytes, &[("ppt/theme/theme1.xml", None)]))
                .unwrap_err()
                .to_string()
                .contains("missing ppt/theme/theme1.xml")
        );
        let path = "ppt/presentation.xml";
        let changed = part(&bytes, path).replace("cx=\"12192000\"", "cx=\"9144000\"");
        assert!(
            load_pptx_bytes(&rewrite(&bytes, &[(path, Some(changed.as_bytes()))]))
                .unwrap_err()
                .to_string()
                .contains("slide size")
        );
        let oversized = vec![b' '; 8 * 1024 * 1024 + 1];
        assert!(
            load_pptx_bytes(&rewrite(&bytes, &[(notes, Some(&oversized))]))
                .unwrap_err()
                .to_string()
                .contains("XML part exceeds")
        );
    }

    #[test]
    fn strict_namespaces_aliases_and_nonstandard_media_paths_are_supported() {
        let bytes = write_pptx_bytes(&deck()).unwrap();
        let mut input = ZipArchive::new(Cursor::new(&bytes)).unwrap();
        let mut output = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for i in 0..input.len() {
            let mut file = input.by_index(i).unwrap();
            let name = file.name().to_owned();
            let mut data = Vec::new();
            file.read_to_end(&mut data).unwrap();
            if name.ends_with(".xml") || name.ends_with(".rels") {
                let mut text = String::from_utf8(data).unwrap();
                text = text
                    .replace(PRESENTATION_NS[0], PRESENTATION_NS[1])
                    .replace(DRAWING_NS[0], DRAWING_NS[1])
                    .replace(REL_NS[0], REL_NS[1]);
                text = text
                    .replace("xmlns:p=", "xmlns:slide=")
                    .replace("<p:", "<slide:")
                    .replace("</p:", "</slide:");
                text = text
                    .replace("xmlns:r=", "xmlns:link=")
                    .replace(" r:", " link:");
                text = text.replace(
                    "../media/slide1-object7.png",
                    "/ppt/media/%73lide1-object7.png",
                );
                data = text.into_bytes();
            }
            output
                .start_file(name, zip::write::FileOptions::default())
                .unwrap();
            output.write_all(&data).unwrap();
        }
        compare(
            &deck(),
            &load_pptx_bytes(&output.finish().unwrap().into_inner()).unwrap(),
        );
    }
    #[test]
    fn references_to_the_same_image_share_storage_across_slides() {
        let bytes = write_pptx_bytes(&deck()).unwrap();
        let path = "ppt/slides/_rels/slide3.xml.rels";
        let rels = part(&bytes, path)
            .replace("../media/slide3-object7.png", "../media/slide1-object7.png");
        let loaded = load_pptx_bytes(&rewrite(&bytes, &[(path, Some(rels.as_bytes()))])).unwrap();
        let ObjectKind::Image { data: first } = &loaded.slides[0].objects[3].kind else {
            panic!()
        };
        let ObjectKind::Image { data: last } = &loaded.slides[2].objects[3].kind else {
            panic!()
        };
        assert!(Arc::ptr_eq(first, last));
    }
    #[test]
    fn excessive_slide_counts_and_declared_archive_expansion_fail_before_import() {
        let bytes = write_pptx_bytes(&deck()).unwrap();
        let path = "ppt/presentation.xml";
        let list = (0..1001)
            .map(|i| format!(r#"<p:sldId id="{}" r:id="rId{}"/>"#, 256 + i, i + 1))
            .collect::<String>();
        let manifest = format!(
            r#"<p:presentation xmlns:p="{}" xmlns:r="{}"><p:sldIdLst>{list}</p:sldIdLst></p:presentation>"#,
            PRESENTATION_NS[0], REL_NS[0]
        );
        assert!(
            load_pptx_bytes(&rewrite(&bytes, &[(path, Some(manifest.as_bytes()))]))
                .unwrap_err()
                .to_string()
                .contains("1000-slide limit")
        );
        let mut bomb = bytes;
        let header = bomb
            .windows(4)
            .enumerate()
            .find_map(|(i, w)| {
                (w == b"PK\x01\x02"
                    && bomb
                        .get(i + 46..)
                        .is_some_and(|tail| tail.starts_with(b"ppt/media/")))
                .then_some(i)
            })
            .unwrap();
        bomb[header + 24..header + 28].copy_from_slice(&(128u32 * 1024 * 1024 + 1).to_le_bytes());
        assert!(load_pptx_bytes(&bomb)
            .unwrap_err()
            .to_string()
            .contains("expanded parts"));
    }
    #[test]
    fn oversized_files_and_export_models_do_not_replace_documents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.pptx");
        std::fs::File::create(&path)
            .unwrap()
            .set_len(crate::pptx_limits::MAX_PACKAGE_BYTES + 1)
            .unwrap();
        assert!(crate::load_pptx_path(&path)
            .unwrap_err()
            .to_string()
            .contains("file limit"));
        let mut p = Presentation::new();
        p.slides = vec![Slide::blank(); 1001];
        std::fs::write(&path, b"original").unwrap();
        assert!(write_pptx_path(&p, &path)
            .unwrap_err()
            .to_string()
            .contains("slide limit"));
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        p.slides = vec![Slide::blank()];
        p.slides[0].objects = vec![SlideObject::shape(ShapeKind::Rectangle); 999];
        assert!(write_pptx_path(&p, &path)
            .unwrap_err()
            .to_string()
            .contains("object limit"));
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
    }

    #[test]
    fn empty_deck_has_a_valid_blank_slide_and_repeated_exports_are_independent() {
        let mut p = Presentation::new();
        p.slides.clear();
        let bytes = write_pptx_bytes(&p).unwrap();
        assert_eq!(
            load_pptx_bytes(&bytes).unwrap().slides,
            vec![Slide::blank()]
        );
        let p = deck();
        let first = write_pptx_bytes(&p).unwrap();
        let second = write_pptx_bytes(&p).unwrap();
        compare(&p, &load_pptx_bytes(&first).unwrap());
        compare(&p, &load_pptx_bytes(&second).unwrap());
        let ObjectKind::Image { data } =
            &load_pptx_bytes(&first).unwrap().slides[0].objects[3].kind
        else {
            panic!()
        };
        assert_eq!(crate::decode_image(data).unwrap().dimensions(), (4, 3));
    }
}
