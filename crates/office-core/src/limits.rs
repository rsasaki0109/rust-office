//! Admission limits shared by imported Writer documents and recovery snapshots.
use crate::{Block, Document, ImageSource, Paragraph};
pub const MAX_DOCUMENT_TEXT: usize = 4 * 1024 * 1024;
pub const MAX_PARAGRAPH_TEXT: usize = 64 * 1024;
pub const MAX_DOCUMENT_ITEMS: usize = 50_000;
pub const MAX_IMAGE_BYTES: usize = 16 * 1024 * 1024;

pub fn validate_document(document: &Document) -> Result<(), String> {
    if document.sections.is_empty() || document.sections.len() > 1000 {
        return Err("Writer exceeds section limits".into());
    }
    let mut budget = Budget::default();
    budget.text(document.title.len())?;
    for section in &document.sections {
        if !section.page_style.is_valid() {
            return Err("Invalid section paper size or margins".into());
        }
        for p in section.header.iter().chain(section.footer.iter()) {
            budget.paragraph(p)?;
        }
        for block in &section.blocks {
            budget.item(1)?;
            match block {
                Block::Paragraph(p) => budget.paragraph(p)?,
                Block::Table(table) => {
                    for row in &table.rows {
                        budget.item(1 + row.cells.len())?;
                        if row.cells.len() > 256 {
                            return Err("Writer table exceeds 256 columns".into());
                        }
                        for cell in &row.cells {
                            for p in &cell.paragraphs {
                                budget.paragraph(p)?;
                            }
                        }
                    }
                }
                Block::Image(image) => {
                    if !image.width_pt.is_finite()
                        || !image.height_pt.is_finite()
                        || !(1.0..=2880.0).contains(&image.width_pt)
                        || !(1.0..=2880.0).contains(&image.height_pt)
                    {
                        return Err("Invalid Writer image display dimensions".into());
                    }
                    budget.text(image.alt_text.len())?;
                    match &image.source {
                        ImageSource::Embedded { data, mime } => {
                            budget.images = budget.images.saturating_add(data.len());
                            if data.len() > MAX_IMAGE_BYTES || budget.images > 64 * 1024 * 1024 {
                                return Err("Writer exceeds embedded image byte limits".into());
                            }
                            budget.text(mime.len())?;
                        }
                        ImageSource::Path { path } => budget.text(path.len())?,
                    }
                }
                Block::PageBreak => {}
            }
        }
    }
    check_flow_budget(document)
}
pub fn validate_paragraph(paragraph: &Paragraph) -> Result<(), String> {
    Budget::default().paragraph(paragraph)
}

#[derive(Default)]
struct Budget {
    items: usize,
    text: usize,
    images: usize,
}
impl Budget {
    fn item(&mut self, count: usize) -> Result<(), String> {
        self.items = self.items.saturating_add(count);
        if self.items > MAX_DOCUMENT_ITEMS {
            return Err("Writer exceeds the 50000-item limit".into());
        }
        Ok(())
    }
    fn text(&mut self, count: usize) -> Result<(), String> {
        self.text = self.text.saturating_add(count);
        if self.text > MAX_DOCUMENT_TEXT {
            return Err("Writer exceeds the 4 MiB text limit".into());
        }
        Ok(())
    }
    fn paragraph(&mut self, paragraph: &Paragraph) -> Result<(), String> {
        self.item(1 + paragraph.runs.len())?;
        if paragraph
            .style
            .list
            .is_some_and(|list| list.level > crate::ListStyle::MAX_LEVEL)
        {
            return Err("Invalid Writer list nesting level".into());
        }
        let mut bytes = 0usize;
        if !paragraph.style.space_after.is_finite()
            || !(0.0..=2880.0).contains(&paragraph.style.space_after)
            || !paragraph.style.line_spacing.is_finite()
            || !(0.1..=10.0).contains(&paragraph.style.line_spacing)
        {
            return Err("Invalid Writer paragraph metrics".into());
        }
        for run in &paragraph.runs {
            if !run.style.font_size.is_finite() || !(1.0..=400.0).contains(&run.style.font_size) {
                return Err("Invalid Writer font size".into());
            }
            bytes = bytes.saturating_add(run.text.len());
            self.text(
                run.text.len()
                    + run.style.font_family.len()
                    + run.link.as_ref().map_or(0, String::len),
            )?;
        }
        if bytes > MAX_PARAGRAPH_TEXT {
            return Err("Writer paragraph exceeds the 64 KiB text limit".into());
        }
        Ok(())
    }
}

/// Conservative flow budget before egui allocates pages and repeated margin galleys.
fn check_flow_budget(document: &Document) -> Result<(), String> {
    fn height(p: &Paragraph, width: f32) -> f64 {
        let font = p
            .runs
            .iter()
            .map(|r| r.style.font_size)
            .fold(12.0_f32, f32::max) as f64;
        let indent = p.style.list.map_or(0.0, |l| f32::from(l.level + 1) * 36.0);
        let width = f64::from((width - indent).max(8.0));
        let chars = p.runs.iter().map(|r| r.text.chars().count()).sum::<usize>();
        let breaks = p
            .runs
            .iter()
            .map(|r| r.text.bytes().filter(|b| *b == b'\n').count())
            .sum::<usize>();
        let capacity = (width / (font * 2.0)).floor().max(1.0);
        let lines = ((chars as f64) / capacity).ceil().max(1.0) + breaks as f64;
        lines * font * 2.0 * f64::from(p.style.line_spacing) + f64::from(p.style.space_after)
    }
    let mut pages = 0usize;
    let mut margin_chars = 0usize;
    for (index, section) in document.sections.iter().enumerate() {
        let width = section.page_style.content_width();
        let mut flow = 0.0_f64;
        let mut breaks = 0usize;
        for block in &section.blocks {
            flow += match block {
                Block::Paragraph(p) => height(p, width),
                Block::Image(image) => f64::from(image.height_pt) + 12.0,
                Block::Table(table) => table
                    .rows
                    .iter()
                    .map(|row| {
                        let cell_width = width / (row.cells.len().max(1) as f32);
                        row.cells
                            .iter()
                            .map(|cell| {
                                cell.paragraphs
                                    .iter()
                                    .map(|p| height(p, cell_width))
                                    .sum::<f64>()
                            })
                            .fold(0.0, f64::max)
                            + 12.0
                    })
                    .sum(),
                Block::PageBreak => {
                    breaks += 1;
                    0.0
                }
            };
        }
        let section_pages = (flow / f64::from(section.page_style.content_height()))
            .ceil()
            .max(1.0) as usize
            + breaks;
        pages = pages.saturating_add(section_pages);
        let chars = document
            .section_header(index)
            .into_iter()
            .chain(document.section_footer(index))
            .map(Paragraph::char_len)
            .sum::<usize>();
        margin_chars = margin_chars.saturating_add(chars.saturating_mul(section_pages));
        if pages > 2000 || margin_chars > MAX_DOCUMENT_TEXT {
            return Err("Writer exceeds estimated 2000-page or repeated margin text limits".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn table_margin_and_run_text_share_the_same_budget() {
        let mut doc = Document::with_text("ok");
        doc.sections[0].header = Some(Paragraph::from_text("x".repeat(MAX_PARAGRAPH_TEXT + 1)));
        assert!(validate_document(&doc).unwrap_err().contains("64 KiB"));
        doc.sections[0].header = None;
        doc.sections[0]
            .blocks
            .push(Block::Table(crate::Table::new(1, 257)));
        assert!(validate_document(&doc).unwrap_err().contains("256 columns"));
    }
    #[test]
    fn small_input_can_exceed_the_flow_budget() {
        let mut doc = Document::with_text(&"x".repeat(1000));
        let style = &mut doc.sections[0].page_style;
        style.margin_left = style.width - 72.0;
        style.margin_right = 36.0;
        style.margin_top = style.height - 72.0;
        style.margin_bottom = 36.0;
        doc.sections[0].blocks[0].as_paragraph_mut().unwrap().runs[0]
            .style
            .font_size = 400.0;
        assert!(validate_document(&doc).unwrap_err().contains("2000-page"));
    }
}
