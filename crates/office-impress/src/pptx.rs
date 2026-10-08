//! Minimal PPTX (OOXML) writer — title + body text per slide.

use std::io::{Cursor, Seek, Write};
use std::path::Path;

use thiserror::Error;
use zip::write::FileOptions;
use zip::{CompressionMethod, ZipWriter};

use crate::model::{Presentation, SLIDE_HEIGHT_PT, SLIDE_WIDTH_PT};

#[derive(Debug, Error)]
pub enum PptxError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("zip error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("pptx parse error: {0}")]
    Parse(String),
}

pub fn write_pptx_path(presentation: &Presentation, path: &Path) -> Result<(), PptxError> {
    let bytes = write_pptx_bytes(presentation)?;
    office_core::storage::atomic_write(path, &bytes)?;
    Ok(())
}

pub fn write_pptx_bytes(presentation: &Presentation) -> Result<Vec<u8>, PptxError> {
    let mut cursor = Cursor::new(Vec::new());
    write_pptx(presentation, &mut cursor)?;
    Ok(cursor.into_inner())
}

fn write_pptx<W: Write + Seek>(presentation: &Presentation, writer: W) -> Result<(), PptxError> {
    let mut zip = ZipWriter::new(writer);
    let opts = FileOptions::default().compression_method(CompressionMethod::Deflated);

    let n = presentation.slides.len().max(1);

    zip.start_file("[Content_Types].xml", opts)?;
    zip.write_all(content_types(n).as_bytes())?;

    zip.start_file("_rels/.rels", opts)?;
    zip.write_all(ROOT_RELS.as_bytes())?;

    zip.start_file("ppt/presentation.xml", opts)?;
    zip.write_all(presentation_xml(n).as_bytes())?;

    zip.start_file("ppt/_rels/presentation.xml.rels", opts)?;
    zip.write_all(presentation_rels(n).as_bytes())?;

    zip.start_file("ppt/slideLayouts/slideLayout1.xml", opts)?;
    zip.write_all(SLIDE_LAYOUT.as_bytes())?;

    zip.start_file("ppt/slideLayouts/_rels/slideLayout1.xml.rels", opts)?;
    zip.write_all(SLIDE_LAYOUT_RELS.as_bytes())?;

    zip.start_file("ppt/slideMasters/slideMaster1.xml", opts)?;
    zip.write_all(SLIDE_MASTER.as_bytes())?;

    zip.start_file("ppt/slideMasters/_rels/slideMaster1.xml.rels", opts)?;
    zip.write_all(SLIDE_MASTER_RELS.as_bytes())?;

    zip.start_file("ppt/theme/theme1.xml", opts)?;
    zip.write_all(theme_xml(presentation).as_bytes())?;

    for (i, slide) in presentation.slides.iter().enumerate() {
        let idx = i + 1;
        zip.start_file(format!("ppt/slides/slide{idx}.xml"), opts)?;
        zip.write_all(slide_xml(slide).as_bytes())?;
        zip.start_file(format!("ppt/slides/_rels/slide{idx}.xml.rels"), opts)?;
        zip.write_all(SLIDE_RELS.as_bytes())?;
    }

    zip.finish()?;
    Ok(())
}

fn content_types(n: usize) -> String {
    let mut overrides = String::from(
        r#"  <Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/>
  <Override PartName="/ppt/slideMasters/slideMaster1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml"/>
  <Override PartName="/ppt/slideLayouts/slideLayout1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml"/>
  <Override PartName="/ppt/theme/theme1.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/>
"#,
    );
    for i in 1..=n {
        overrides.push_str(&format!(
            r#"  <Override PartName="/ppt/slides/slide{i}.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>
"#
        ));
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
{overrides}</Types>
"#
    )
}

fn presentation_xml(n: usize) -> String {
    // EMUs: 914400 per inch. 960pt = 13.333in, 540pt = 7.5in
    let cx = (SLIDE_WIDTH_PT as f64 / 72.0 * 914400.0).round() as i64;
    let cy = (SLIDE_HEIGHT_PT as f64 / 72.0 * 914400.0).round() as i64;
    let mut sld_id_lst = String::new();
    for i in 1..=n {
        let id = 256 + i as u32;
        sld_id_lst.push_str(&format!(r#"<p:sldId id="{id}" r:id="rId{i}"/>"#));
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentation xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
  <p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rId{master}"/></p:sldMasterIdLst>
  <p:sldIdLst>{sld_id_lst}</p:sldIdLst>
  <p:sldSz cx="{cx}" cy="{cy}"/>
  <p:notesSz cx="{cx}" cy="{cy}"/>
</p:presentation>
"#,
        master = n + 1
    )
}

fn presentation_rels(n: usize) -> String {
    let mut rels = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
"#,
    );
    for i in 1..=n {
        rels.push_str(&format!(
            r#"<Relationship Id="rId{i}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide{i}.xml"/>
"#
        ));
    }
    let master = n + 1;
    let theme = n + 2;
    rels.push_str(&format!(
        r#"<Relationship Id="rId{master}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster" Target="slideMasters/slideMaster1.xml"/>
<Relationship Id="rId{theme}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme" Target="theme/theme1.xml"/>
</Relationships>
"#
    ));
    rels
}

fn slide_xml(slide: &crate::model::Slide) -> String {
    let title = escape_xml(&slide.title.text);
    let body = escape_xml(&slide.body.text);
    let body_paras = body_paragraphs(&body);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
  <p:cSld>
    <p:spTree>
      <p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>
      <p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr>
      <p:sp>
        <p:nvSpPr><p:cNvPr id="2" name="Title"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr>
        <p:spPr>
          <a:xfrm><a:off x="{tx}" y="{ty}"/><a:ext cx="{tw}" cy="{th}"/></a:xfrm>
          <a:prstGeom prst="rect"><a:avLst/></a:prstGeom>
        </p:spPr>
        <p:txBody>
          <a:bodyPr/><a:lstStyle/>
          <a:p><a:r><a:rPr lang="en-US" sz="3600" b="1"/><a:t>{title}</a:t></a:r></a:p>
        </p:txBody>
      </p:sp>
      <p:sp>
        <p:nvSpPr><p:cNvPr id="3" name="Body"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr>
        <p:spPr>
          <a:xfrm><a:off x="{bx}" y="{by}"/><a:ext cx="{bw}" cy="{bh}"/></a:xfrm>
          <a:prstGeom prst="rect"><a:avLst/></a:prstGeom>
        </p:spPr>
        <p:txBody>
          <a:bodyPr/><a:lstStyle/>
          {body_paras}
        </p:txBody>
      </p:sp>
    </p:spTree>
  </p:cSld>
  <p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr>
</p:sld>
"#,
        tx = frac_emu(slide.title.x, SLIDE_WIDTH_PT),
        ty = frac_emu(slide.title.y, SLIDE_HEIGHT_PT),
        tw = frac_emu(slide.title.w, SLIDE_WIDTH_PT),
        th = frac_emu(slide.title.h, SLIDE_HEIGHT_PT),
        bx = frac_emu(slide.body.x, SLIDE_WIDTH_PT),
        by = frac_emu(slide.body.y, SLIDE_HEIGHT_PT),
        bw = frac_emu(slide.body.w, SLIDE_WIDTH_PT),
        bh = frac_emu(slide.body.h, SLIDE_HEIGHT_PT),
    )
}

fn body_paragraphs(escaped: &str) -> String {
    if escaped.is_empty() {
        return "<a:p><a:endParaRPr lang=\"en-US\" sz=\"2000\"/></a:p>".into();
    }
    escaped
        .split('\n')
        .map(|line| {
            format!(r#"<a:p><a:r><a:rPr lang="en-US" sz="2000"/><a:t>{line}</a:t></a:r></a:p>"#)
        })
        .collect::<Vec<_>>()
        .join("")
}

fn frac_emu(frac: f32, total_pt: f32) -> i64 {
    let pt = frac * total_pt;
    (pt as f64 / 72.0 * 914400.0).round() as i64
}

fn theme_xml(presentation: &Presentation) -> String {
    let [r, g, b] = presentation.theme.accent;
    let accent = format!("{r:02X}{g:02X}{b:02X}");
    let [r, g, b] = presentation.theme.background;
    let bg = format!("{r:02X}{g:02X}{b:02X}");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="{name}">
  <a:themeElements>
    <a:clrScheme name="{name}">
      <a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1>
      <a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1>
      <a:dk2><a:srgbClr val="1F497D"/></a:dk2>
      <a:lt2><a:srgbClr val="{bg}"/></a:lt2>
      <a:accent1><a:srgbClr val="{accent}"/></a:accent1>
      <a:accent2><a:srgbClr val="C0504D"/></a:accent2>
      <a:accent3><a:srgbClr val="9BBB59"/></a:accent3>
      <a:accent4><a:srgbClr val="8064A2"/></a:accent4>
      <a:accent5><a:srgbClr val="4BACC6"/></a:accent5>
      <a:accent6><a:srgbClr val="F79646"/></a:accent6>
      <a:hlink><a:srgbClr val="0000FF"/></a:hlink>
      <a:folHlink><a:srgbClr val="800080"/></a:folHlink>
    </a:clrScheme>
    <a:fontScheme name="Office">
      <a:majorFont><a:latin typeface="Calibri"/><a:ea typeface=""/><a:cs typeface=""/></a:majorFont>
      <a:minorFont><a:latin typeface="Calibri"/><a:ea typeface=""/><a:cs typeface=""/></a:minorFont>
    </a:fontScheme>
    <a:fmtScheme name="Office">
      <a:fillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:fillStyleLst>
      <a:lnStyleLst><a:ln w="9525"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln><a:ln w="9525"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln><a:ln w="9525"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln></a:lnStyleLst>
      <a:effectStyleLst><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle></a:effectStyleLst>
      <a:bgFillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:bgFillStyleLst>
    </a:fmtScheme>
  </a:themeElements>
</a:theme>
"#,
        name = escape_xml(&presentation.theme.name)
    )
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

const ROOT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/>
</Relationships>
"#;

const SLIDE_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" Target="../slideLayouts/slideLayout1.xml"/>
</Relationships>
"#;

const SLIDE_LAYOUT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster" Target="../slideMasters/slideMaster1.xml"/>
</Relationships>
"#;

const SLIDE_MASTER_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" Target="../slideLayouts/slideLayout1.xml"/>
  <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme" Target="../theme/theme1.xml"/>
</Relationships>
"#;

const SLIDE_LAYOUT: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sldLayout xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" type="blank" preserve="1">
  <p:cSld name="Blank"><p:spTree>
    <p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>
    <p:grpSpPr/>
  </p:spTree></p:cSld>
  <p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr>
</p:sldLayout>
"#;

const SLIDE_MASTER: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sldMaster xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
  <p:cSld><p:bg><p:bgRef idx="1001"><a:schemeClr val="bg1"/></p:bgRef></p:bg><p:spTree>
    <p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>
    <p:grpSpPr/>
  </p:spTree></p:cSld>
  <p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/>
  <p:sldLayoutIdLst><p:sldLayoutId id="2147483649" r:id="rId1"/></p:sldLayoutIdLst>
</p:sldMaster>
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Presentation;

    #[test]
    fn pptx_export_is_zip_with_slides() {
        let p = Presentation::demo();
        let bytes = write_pptx_bytes(&p).unwrap();
        assert!(bytes.starts_with(b"PK"));
        let cursor = Cursor::new(bytes);
        let mut zip = zip::ZipArchive::new(cursor).unwrap();
        assert!(zip.by_name("ppt/slides/slide1.xml").is_ok());
        assert!(zip.by_name("ppt/slides/slide3.xml").is_ok());
        assert!(zip.by_name("ppt/presentation.xml").is_ok());
    }

    #[test]
    fn pptx_round_trip_demo() {
        let p = Presentation::demo();
        let bytes = write_pptx_bytes(&p).unwrap();
        let restored = crate::pptx_read::load_pptx_bytes(&bytes).unwrap();
        assert_eq!(restored.slides.len(), 3);
        assert!(restored.slides[0].title.text.contains("Impress"));
        assert!(restored.slides[1].title.text.contains("Roadmap"));
        assert!(restored.slides[2].title.text.contains("Thank"));
        assert!(restored.slides[0].body.text.contains("Native"));
    }
}
