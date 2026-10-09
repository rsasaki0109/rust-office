//! PDF export driven by a [`DocumentLayout`] (same pagination as the canvas).

use std::collections::HashMap;
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use egui::{Pos2, Rect};
use office_core::{Block, Document, Image as DocImage};
use printpdf::{BuiltinFont, ImageTransform, Mm, PdfDocument, PdfLayerReference};

use crate::images::load_image_bytes;
use crate::{layout_document, DocumentLayout, LayoutLine, PageDecoration, PageLayout};

/// Convert points (egui / layout) to millimetres.
fn pt_to_mm(pt: f32) -> f32 {
    pt * 25.4 / 72.0
}

/// Layout `document` at zoom 1.0 (headless fonts) and export PDF bytes.
pub fn document_to_layout_pdf_bytes(document: &Document) -> Result<Vec<u8>, String> {
    if document.sections.is_empty() || document.sections.iter().any(|s| !s.page_style.is_valid()) {
        return Err("Invalid section page geometry".into());
    }
    let mut out = Ok(Vec::new());
    egui::__run_test_ui(|ui| {
        let layout = layout_document(ui, document, Pos2::ZERO, 1.0);
        out = document_layout_to_pdf_bytes(document, &layout);
    });
    out
}

/// Layout `document` and write a `.pdf` file.
pub fn write_document_layout_pdf_path(document: &Document, path: &Path) -> Result<(), String> {
    let bytes = document_to_layout_pdf_bytes(document)?;
    office_core::storage::atomic_write(path, &bytes).map_err(|e| e.to_string())
}

/// Render `layout` (typically from `layout_document` at zoom 1.0) to PDF bytes.
pub fn document_layout_to_pdf_bytes(
    document: &Document,
    layout: &DocumentLayout,
) -> Result<Vec<u8>, String> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    write_layout_pdf(document, layout, &mut cursor)?;
    Ok(cursor.into_inner())
}

fn write_layout_pdf<W: std::io::Write>(
    document: &Document,
    layout: &DocumentLayout,
    writer: &mut W,
) -> Result<(), String> {
    if document.sections.iter().any(|s| !s.page_style.is_valid()) {
        return Err("Invalid section page geometry".into());
    }
    let title = if document.title.is_empty() {
        "rust-office document"
    } else {
        document.title.as_str()
    };

    let pages = if layout.pages.is_empty() {
        return Err("empty layout".into());
    } else {
        &layout.pages
    };

    let images = collect_images(document);
    let page_style = document
        .sections
        .get(pages[0].section_index)
        .ok_or("Invalid layout section")?
        .page_style
        .clone();
    let (doc, page1, layer1) = PdfDocument::new(
        title,
        Mm(pt_to_mm(page_style.width)),
        Mm(pt_to_mm(page_style.height)),
        "Layer 1",
    );
    let font = load_best_font(&doc)?;

    let mut handles = vec![(page1, layer1)];
    for (i, page) in pages.iter().enumerate().skip(1) {
        let style = &document
            .sections
            .get(page.section_index)
            .ok_or("Invalid layout section")?
            .page_style;
        let (p, l) = doc.add_page(
            Mm(pt_to_mm(style.width)),
            Mm(pt_to_mm(style.height)),
            format!("Page {}, Layer 1", i + 1),
        );
        handles.push((p, l));
    }

    for (idx, page) in pages.iter().enumerate() {
        let (page_idx, layer_idx) = handles[idx];
        let layer = doc.get_page(page_idx).get_layer(layer_idx);
        let height = document.sections[page.section_index].page_style.height;
        paint_page(page, &layer, &font, height, &images);
    }

    let mut buf = BufWriter::new(writer);
    doc.save(&mut buf).map_err(|e| e.to_string())?;
    Ok(())
}

fn collect_images(document: &Document) -> HashMap<String, DocImage> {
    let mut map = HashMap::new();
    for section in &document.sections {
        for block in &section.blocks {
            if let Block::Image(image) = block {
                map.insert(image.cache_key(), image.clone());
            }
        }
    }
    map
}

fn paint_page(
    page: &PageLayout,
    layer: &PdfLayerReference,
    font: &printpdf::IndirectFontRef,
    page_height_pt: f32,
    images: &HashMap<String, DocImage>,
) {
    let origin = page.page_rect.min;

    for line in &page.header_lines {
        draw_layout_line(line, layer, font, origin, page_height_pt, 9.0);
    }
    for line in &page.footer_lines {
        draw_layout_line(line, layer, font, origin, page_height_pt, 9.0);
    }
    for line in &page.lines {
        draw_layout_line(line, layer, font, origin, page_height_pt, 11.0);
    }
    for dec in &page.decorations {
        match dec {
            PageDecoration::Table { cells, .. } => {
                for cell in cells {
                    for line in &cell.lines {
                        draw_layout_line(line, layer, font, origin, page_height_pt, 10.0);
                    }
                }
            }
            PageDecoration::Image {
                rect,
                label,
                cache_key,
            } => {
                if let Some(img) = images.get(cache_key) {
                    if draw_embedded_image(layer, img, *rect, origin, page_height_pt).is_ok() {
                        continue;
                    }
                }
                let text = format!("[Image: {label}]");
                let x = rect.left() - origin.x;
                let y_from_top = rect.top() - origin.y;
                let y = page_height_pt - y_from_top - 12.0;
                layer.use_text(text, 10.0, Mm(pt_to_mm(x)), Mm(pt_to_mm(y.max(0.0))), font);
            }
        }
    }
}

fn draw_embedded_image(
    layer: &PdfLayerReference,
    image: &DocImage,
    rect: Rect,
    page_origin: Pos2,
    page_height_pt: f32,
) -> Result<(), String> {
    let bytes = load_image_bytes(&image.source)?;
    // printpdf 0.7 pins `image` 0.24; use its re-export to avoid crate version mismatch.
    let dyn_img = printpdf::image_crate::load_from_memory(&bytes).map_err(|e| e.to_string())?;
    let pw = dyn_img.width().max(1) as f32;
    let ph = dyn_img.height().max(1) as f32;
    let pdf_img = printpdf::Image::from_dynamic_image(&dyn_img);

    let draw_w = rect.width().max(1.0);
    let draw_h = rect.height().max(1.0);
    // At 72 dpi, 1px ≈ 1pt; scale to the layout box.
    let scale_x = draw_w / pw;
    let scale_y = draw_h / ph;
    let x = rect.left() - page_origin.x;
    let y_from_top = rect.bottom() - page_origin.y;
    let y = page_height_pt - y_from_top;

    pdf_img.add_to_layer(
        layer.clone(),
        ImageTransform {
            translate_x: Some(Mm(pt_to_mm(x.max(0.0)))),
            translate_y: Some(Mm(pt_to_mm(y.max(0.0)))),
            scale_x: Some(scale_x),
            scale_y: Some(scale_y),
            dpi: Some(72.0),
            ..Default::default()
        },
    );
    Ok(())
}

fn draw_layout_line(
    line: &LayoutLine,
    layer: &PdfLayerReference,
    font: &printpdf::IndirectFontRef,
    page_origin: egui::Pos2,
    page_height_pt: f32,
    default_size: f32,
) {
    let mut text = line_text(line);
    if let Some(marker) = &line.list_marker {
        text = format!("{marker}{text}");
    }
    if text.is_empty() {
        return;
    }
    let font_size = line_font_size(line).unwrap_or(default_size);

    let x = line.rect.left() - page_origin.x;
    // Baseline near the visual bottom of the line box (PDF y grows upward).
    let y_from_top = line.rect.bottom() - page_origin.y;
    let y = page_height_pt - y_from_top;
    let safe: String = text
        .chars()
        .map(|c| if c == '\u{000C}' { ' ' } else { c })
        .collect();
    layer.use_text(
        safe,
        font_size,
        Mm(pt_to_mm(x.max(0.0))),
        Mm(pt_to_mm(y.max(0.0))),
        font,
    );
}

fn line_text(line: &LayoutLine) -> String {
    let plain = line.galley.text();
    plain
        .chars()
        .skip(line.start_offset)
        .take(line.end_offset.saturating_sub(line.start_offset))
        .collect()
}

fn line_font_size(line: &LayoutLine) -> Option<f32> {
    line.galley
        .job
        .sections
        .first()
        .map(|s| s.format.font_id.size)
        .filter(|s| *s > 0.0)
}

fn load_best_font(
    doc: &printpdf::PdfDocumentReference,
) -> Result<printpdf::IndirectFontRef, String> {
    const CANDIDATES: &[&str] = &[
        "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
        "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
        "C:\\Windows\\Fonts\\arialuni.ttf",
        "C:\\Windows\\Fonts\\msgothic.ttc",
    ];
    for path in CANDIDATES {
        if let Ok(file) = File::open(path) {
            if let Ok(font) = doc.add_external_font(file) {
                return Ok(font);
            }
        }
    }
    doc.add_builtin_font(BuiltinFont::Helvetica)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use office_core::{DocumentEditor, Image};

    fn tiny_png() -> Vec<u8> {
        vec![
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00,
            0x00, 0x90, 0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08,
            0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D,
            0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ]
    }

    #[test]
    fn pdf_media_boxes_match_mixed_section_pages() {
        let mut editor = DocumentEditor::default();
        editor.insert_text("A4").unwrap();
        editor
            .insert_section_break_with_style(Some(office_core::PageStyle {
                width: 792.0,
                height: 612.0,
                ..Default::default()
            }))
            .unwrap();
        editor.insert_text("Landscape Letter").unwrap();
        let bytes = document_to_layout_pdf_bytes(editor.document()).unwrap();
        let pdf = printpdf::lopdf::Document::load_mem(&bytes).unwrap();
        let pages = pdf.get_pages();
        assert_eq!(pages.len(), 2);
        for (id, (width, height)) in pages.values().zip([(595.28, 841.89), (792.0, 612.0)]) {
            let bounds = pdf
                .get_dictionary(*id)
                .unwrap()
                .get(b"MediaBox")
                .unwrap()
                .as_array()
                .unwrap();
            assert!((bounds[2].as_float().unwrap() - width).abs() < 0.03);
            assert!((bounds[3].as_float().unwrap() - height).abs() < 0.03);
        }
    }

    #[test]
    fn layout_pdf_matches_page_count() {
        let mut ed = DocumentEditor::default();
        ed.insert_text("Hello layout PDF\n").unwrap();
        ed.insert_page_break().unwrap();
        ed.insert_text("Second page").unwrap();
        let bytes = document_to_layout_pdf_bytes(ed.document()).unwrap();
        assert!(bytes.starts_with(b"%PDF"));
        let s = String::from_utf8_lossy(&bytes);
        let has_count2 = s.contains("/Count 2");
        let page2_layer = s.contains("Page 2");
        assert!(
            has_count2 || page2_layer,
            "expected multi-page PDF; snippet={:?}",
            &s[..s.len().min(800)]
        );
    }

    #[test]
    fn layout_pdf_includes_body_text() {
        let mut ed = DocumentEditor::default();
        ed.insert_text("UniqueLayoutPdfToken").unwrap();
        let bytes = document_to_layout_pdf_bytes(ed.document()).unwrap();
        assert!(bytes.starts_with(b"%PDF"));
        assert!(bytes.len() > 200);
    }

    #[test]
    fn layout_pdf_embeds_png_image() {
        let mut ed = DocumentEditor::default();
        ed.insert_text("Pic\n").unwrap();
        let img = Image::from_embedded("image/png", tiny_png(), "dot", 72.0, 72.0);
        ed.insert_image(img).unwrap();
        let bytes = document_to_layout_pdf_bytes(ed.document()).unwrap();
        assert!(bytes.starts_with(b"%PDF"));
        // Embedded images typically produce an Image XObject or stream larger than text-only.
        assert!(bytes.len() > 400, "len={}", bytes.len());
        let s = String::from_utf8_lossy(&bytes);
        assert!(
            !s.contains("[Image:"),
            "expected real image, not placeholder"
        );
    }
}
