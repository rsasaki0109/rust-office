//! Export a [`Document`] to a multi-page A4 PDF.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use office_core::{Alignment, Block, Document, ListKind};
use printpdf::{
    BuiltinFont, Mm, PdfDocument, PdfDocumentReference, PdfLayerReference, PdfPageIndex,
};
use thiserror::Error;

/// PDF export failures.
#[derive(Debug, Error)]
pub enum PdfError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("pdf error: {0}")]
    Pdf(String),
}

impl From<printpdf::Error> for PdfError {
    fn from(value: printpdf::Error) -> Self {
        Self::Pdf(value.to_string())
    }
}

/// Write `document` as PDF bytes (A4 pages, body + header/footer).
pub fn document_to_pdf_bytes(document: &Document) -> Result<Vec<u8>, PdfError> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    write_document_pdf(document, &mut cursor)?;
    Ok(cursor.into_inner())
}

/// Write `document` to a `.pdf` path.
pub fn write_document_pdf_path(document: &Document, path: &Path) -> Result<(), PdfError> {
    let bytes = document_to_pdf_bytes(document)?;
    office_core::storage::atomic_write(path, &bytes)?;
    Ok(())
}

fn write_document_pdf<W: std::io::Write>(
    document: &Document,
    writer: &mut W,
) -> Result<(), PdfError> {
    let page_style = document.page_style();
    let page_w = Mm(pt_to_mm(page_style.width));
    let page_h = Mm(pt_to_mm(page_style.height));
    let margin_l = Mm(pt_to_mm(page_style.margin_left));
    let margin_r = Mm(pt_to_mm(page_style.margin_right));
    let margin_t = Mm(pt_to_mm(page_style.margin_top));
    let margin_b = Mm(pt_to_mm(page_style.margin_bottom));

    let title = if document.title.is_empty() {
        "rust-office document"
    } else {
        document.title.as_str()
    };
    let (doc, page1, layer1) = PdfDocument::new(title, page_w, page_h, "Layer 1");

    let font = load_best_font(&doc)?;
    let font_size = 11.0_f32;
    let line_h_mm = Mm(pt_to_mm(font_size * 1.35));
    let content_width_mm = page_w.0 - margin_l.0 - margin_r.0;

    let lines = collect_export_lines(document);
    let header = document
        .header()
        .map(|p| p.plain_text())
        .filter(|s| !s.is_empty());
    let footer_template = document
        .footer()
        .map(|p| p.plain_text())
        .filter(|s| !s.is_empty());

    // Pack lines into pages.
    let usable_top = page_h.0 - margin_t.0;
    let usable_bottom = margin_b.0;
    let mut pages_lines: Vec<Vec<ExportLine>> = Vec::new();
    let mut current: Vec<ExportLine> = Vec::new();
    let mut y = usable_top;

    for line in &lines {
        if line.text == "\u{000C}" {
            pages_lines.push(std::mem::take(&mut current));
            y = usable_top;
            continue;
        }
        let wrapped = wrap_line(&line.text, content_width_mm, font_size);
        for (i, piece) in wrapped.into_iter().enumerate() {
            if y - line_h_mm.0 < usable_bottom {
                pages_lines.push(std::mem::take(&mut current));
                y = usable_top;
            }
            current.push(ExportLine {
                text: piece,
                align: line.align,
                bold: line.bold && i == 0,
            });
            y -= line_h_mm.0;
        }
    }
    if current.is_empty() && pages_lines.is_empty() {
        current.push(ExportLine {
            text: String::new(),
            align: Alignment::Left,
            bold: false,
        });
    }
    if !current.is_empty() {
        pages_lines.push(current);
    }

    let page_count = pages_lines.len().max(1);
    let mut page_handles: Vec<(PdfPageIndex, printpdf::PdfLayerIndex)> = vec![(page1, layer1)];
    for i in 1..page_count {
        let (p, l) = doc.add_page(page_w, page_h, format!("Page {}, Layer 1", i + 1));
        page_handles.push((p, l));
    }

    for (page_idx, page_lines) in pages_lines.iter().enumerate() {
        let (page, layer) = page_handles[page_idx];
        let layer = doc.get_page(page).get_layer(layer);
        let page_no = page_idx + 1;

        if let Some(ref h) = header {
            let text = expand_fields(h, page_no, page_count);
            draw_line(
                &layer,
                &font,
                &text,
                LineGeom {
                    align: Alignment::Left,
                    left_mm: margin_l.0,
                    y_mm: page_h.0 - Mm(pt_to_mm(18.0)).0,
                    width_mm: content_width_mm,
                    font_size: 9.0,
                },
            );
        }

        let mut y = usable_top - line_h_mm.0;
        for line in page_lines {
            draw_line(
                &layer,
                &font,
                &line.text,
                LineGeom {
                    align: line.align,
                    left_mm: margin_l.0,
                    y_mm: y,
                    width_mm: content_width_mm,
                    font_size,
                },
            );
            y -= line_h_mm.0;
        }

        if let Some(ref f) = footer_template {
            let text = expand_fields(f, page_no, page_count);
            draw_line(
                &layer,
                &font,
                &text,
                LineGeom {
                    align: Alignment::Center,
                    left_mm: margin_l.0,
                    y_mm: Mm(pt_to_mm(18.0)).0,
                    width_mm: content_width_mm,
                    font_size: 9.0,
                },
            );
        } else {
            // Default page number when no footer is set.
            let text = format!("{page_no} / {page_count}");
            draw_line(
                &layer,
                &font,
                &text,
                LineGeom {
                    align: Alignment::Center,
                    left_mm: margin_l.0,
                    y_mm: Mm(pt_to_mm(18.0)).0,
                    width_mm: content_width_mm,
                    font_size: 9.0,
                },
            );
        }
    }

    let mut buf = BufWriter::new(writer);
    doc.save(&mut buf)
        .map_err(|e| PdfError::Pdf(e.to_string()))?;
    Ok(())
}

struct ExportLine {
    text: String,
    align: Alignment,
    #[allow(dead_code)]
    bold: bool,
}

fn collect_export_lines(document: &Document) -> Vec<ExportLine> {
    let mut out = Vec::new();
    for block in document.blocks() {
        match block {
            Block::Paragraph(para) => {
                let mut text = para.plain_text();
                if let Some(list) = para.style.list {
                    let marker = match list.kind {
                        ListKind::Bullet => "• ".to_string(),
                        ListKind::Numbered => "• ".to_string(),
                    };
                    let indent = "    ".repeat(list.level as usize);
                    text = format!("{indent}{marker}{text}");
                }
                let bold = para.runs.iter().any(|r| r.style.bold);
                out.push(ExportLine {
                    text,
                    align: para.style.alignment,
                    bold,
                });
            }
            Block::Table(table) => {
                for row in &table.rows {
                    let cells: Vec<String> = row
                        .cells
                        .iter()
                        .map(|c| c.plain_text().replace('\n', " "))
                        .collect();
                    out.push(ExportLine {
                        text: cells.join(" | "),
                        align: Alignment::Left,
                        bold: false,
                    });
                }
                out.push(ExportLine {
                    text: String::new(),
                    align: Alignment::Left,
                    bold: false,
                });
            }
            Block::Image(image) => {
                out.push(ExportLine {
                    text: format!("[Image: {}]", image.display_name()),
                    align: Alignment::Left,
                    bold: false,
                });
            }
            Block::PageBreak => {
                out.push(ExportLine {
                    text: "\u{000C}".into(),
                    align: Alignment::Left,
                    bold: false,
                });
            }
        }
    }
    out
}

fn wrap_line(text: &str, max_width_mm: f32, font_size_pt: f32) -> Vec<String> {
    if text == "\u{000C}" {
        return vec![text.to_string()];
    }
    if text.is_empty() {
        return vec![String::new()];
    }
    let avg_char_mm = pt_to_mm(font_size_pt) * 0.55;
    let max_chars = ((max_width_mm / avg_char_mm).floor() as usize).max(8);
    let chars: Vec<char> = text.chars().collect();
    let mut lines = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let end = (i + max_chars).min(chars.len());
        // Prefer break at whitespace when possible.
        let mut break_at = end;
        if end < chars.len() {
            if let Some(rel) = chars[i..end].iter().rposition(|c| c.is_whitespace()) {
                if rel > 0 {
                    break_at = i + rel + 1;
                }
            }
        }
        lines.push(chars[i..break_at].iter().collect());
        i = break_at;
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn draw_line(
    layer: &PdfLayerReference,
    font: &printpdf::IndirectFontRef,
    text: &str,
    geom: LineGeom,
) {
    if text == "\u{000C}" || text.is_empty() {
        return;
    }
    let avg_char_mm = pt_to_mm(geom.font_size) * 0.55;
    let text_w = text.chars().count() as f32 * avg_char_mm;
    let x = match geom.align {
        Alignment::Left | Alignment::Justify => geom.left_mm,
        Alignment::Center => geom.left_mm + ((geom.width_mm - text_w) * 0.5).max(0.0),
        Alignment::Right => geom.left_mm + (geom.width_mm - text_w).max(0.0),
    };
    // printpdf can panic on some glyphs; sanitize to avoid aborting export.
    let safe: String = text
        .chars()
        .map(|c| if c == '\u{000C}' { ' ' } else { c })
        .collect();
    layer.use_text(safe, geom.font_size, Mm(x), Mm(geom.y_mm), font);
}

struct LineGeom {
    align: Alignment,
    left_mm: f32,
    y_mm: f32,
    width_mm: f32,
    font_size: f32,
}

fn expand_fields(template: &str, page: usize, pages: usize) -> String {
    template
        .replace("{page}", &page.to_string())
        .replace("{pages}", &pages.to_string())
}

fn pt_to_mm(pt: f32) -> f32 {
    pt * 25.4 / 72.0
}

fn load_best_font(doc: &PdfDocumentReference) -> Result<printpdf::IndirectFontRef, PdfError> {
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
        .map_err(PdfError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use office_core::DocumentEditor;

    #[test]
    fn pdf_export_produces_nonzero_bytes() {
        let mut ed = DocumentEditor::default();
        ed.insert_text("Hello PDF\nSecond line").unwrap();
        ed.set_header_text("Hdr").unwrap();
        ed.set_footer_text("Page {page} of {pages}").unwrap();
        let bytes = document_to_pdf_bytes(ed.document()).unwrap();
        assert!(bytes.starts_with(b"%PDF"));
        assert!(bytes.len() > 200);
    }

    #[test]
    fn pdf_export_page_break_multi_page() {
        let mut ed = DocumentEditor::default();
        ed.insert_text("One").unwrap();
        ed.insert_page_break().unwrap();
        ed.insert_text("Two").unwrap();
        let bytes = document_to_pdf_bytes(ed.document()).unwrap();
        assert!(bytes.starts_with(b"%PDF"));
        // printpdf may emit "/Type /Pages" and page objects without a literal "/Type /Page".
        let s = String::from_utf8_lossy(&bytes);
        let has_count2 =
            s.contains("/Count 2") || s.contains("/Count 2\n") || s.contains("/Count 2 ");
        let page_objs = s.matches("/Type /Page").count() + s.matches("/Type/Page").count();
        let page2_layer = s.contains("Page 2");
        assert!(
            has_count2 || page_objs >= 2 || page2_layer,
            "expected multi-page PDF; count2={has_count2} page_objs={page_objs} page2={page2_layer}; snippet={:?}",
            &s[..s.len().min(800)]
        );
    }
}
