//! Layout helpers for rendering Writer pages.
//!
//! Measurement uses egui fonts so the UI canvas and hit-testing stay in sync.
//! Content that exceeds one page flows onto additional A4 pages.

mod images;
mod layout_pdf;

#[cfg(test)]
mod smoke_tests;

use std::collections::HashMap;

use egui::text::{LayoutJob, TextFormat};
use egui::{
    Align, Color32, FontFamily, FontId, Galley, Pos2, Rect, Stroke, StrokeKind, TextureId, Ui, Vec2,
};
use office_core::{
    Alignment, Block, CellAddress, DocPosition, Document, EditFocus, Image, ListKind, Paragraph,
    Selection, Table, TextStyle,
};

pub use images::{
    decode_color_image, ensure_image_textures, fit_display_size, image_from_path_fitted,
    load_image_bytes, probe_pixel_size,
};
pub use layout_pdf::{
    document_layout_to_pdf_bytes, document_to_layout_pdf_bytes, write_document_layout_pdf_path,
};

/// Direction for visual-line caret movement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerticalDir {
    Up,
    Down,
}

/// One laid-out character cell on a line (for hit-testing / caret).
#[derive(Clone)]
pub struct GlyphCell {
    pub paragraph: usize,
    pub char_offset: usize,
    pub rect: Rect,
}

/// A single visual line within a paragraph.
#[derive(Clone)]
pub struct LayoutLine {
    pub paragraph: usize,
    pub start_offset: usize,
    pub end_offset: usize,
    pub rect: Rect,
    pub galley: std::sync::Arc<Galley>,
    /// X origin used when painting this paragraph's galley.
    pub galley_origin_x: f32,
    /// Y offset of this row inside the galley (after line-spacing), used to
    /// align `painter.galley` so this row lands on [`Self::rect`].
    pub row_offset_y: f32,
    pub cells: Vec<GlyphCell>,
    /// Optional list marker drawn to the left of the first visual line.
    pub list_marker: Option<String>,
}

impl LayoutLine {
    fn galley_paint_origin(&self) -> Pos2 {
        Pos2::new(self.galley_origin_x, self.rect.top() - self.row_offset_y)
    }

    fn translate(&mut self, delta: Vec2) {
        self.rect = self.rect.translate(delta);
        for cell in &mut self.cells {
            cell.rect = cell.rect.translate(delta);
        }
    }
}

/// Laid-out contents of one table cell.
#[derive(Clone)]
pub struct TableCellLayout {
    pub address: CellAddress,
    pub rect: Rect,
    /// Local lines (paragraph index always 0 within the cell).
    pub lines: Vec<LayoutLine>,
}

/// Non-text flow decorations (tables / images) placed on a page.
#[derive(Clone)]
pub enum PageDecoration {
    Table {
        rect: Rect,
        block_index: usize,
        cells: Vec<TableCellLayout>,
    },
    Image {
        rect: Rect,
        label: String,
        /// Matches [`office_core::Image::cache_key`].
        cache_key: String,
    },
}

/// Result of clicking in the document canvas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitResult {
    Body(DocPosition),
    Cell { address: CellAddress, offset: usize },
    Header(DocPosition),
    Footer(DocPosition),
}

/// Geometry and lines for one paper page.
#[derive(Clone)]
pub struct PageLayout {
    pub page_rect: Rect,
    pub content_rect: Rect,
    pub lines: Vec<LayoutLine>,
    pub decorations: Vec<PageDecoration>,
    /// Header lines painted in the top margin (may be empty).
    pub header_lines: Vec<LayoutLine>,
    /// Footer lines painted in the bottom margin (may be empty).
    pub footer_lines: Vec<LayoutLine>,
    /// 1-based page number.
    pub page_number: usize,
}

/// Full document layout across one or more pages.
#[derive(Clone)]
pub struct DocumentLayout {
    pub pages: Vec<PageLayout>,
}

impl DocumentLayout {
    pub fn page_count(&self) -> usize {
        self.pages.len().max(1)
    }

    /// 1-based page that contains the caret, or 1 if unknown.
    pub fn page_of(&self, pos: DocPosition) -> usize {
        for page in &self.pages {
            if page.line_index_containing(pos).is_some() {
                return page.page_number;
            }
        }
        1
    }

    pub fn hit_test(&self, pos: Pos2) -> DocPosition {
        match self.hit_test_full(pos) {
            HitResult::Body(p) | HitResult::Header(p) | HitResult::Footer(p) => p,
            HitResult::Cell { offset, .. } => DocPosition::new(0, offset),
        }
    }

    pub fn hit_test_full(&self, pos: Pos2) -> HitResult {
        if self.pages.is_empty() {
            return HitResult::Body(DocPosition::zero());
        }
        if let Some(page) = self
            .pages
            .iter()
            .find(|p| p.page_rect.contains(pos))
            .or_else(|| {
                self.pages.iter().min_by(|a, b| {
                    let da = (a.page_rect.center().y - pos.y).abs();
                    let db = (b.page_rect.center().y - pos.y).abs();
                    da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
                })
            })
        {
            return page.hit_test_full(pos);
        }
        HitResult::Body(DocPosition::zero())
    }

    pub fn margin_caret_rect(
        &self,
        focus: EditFocus,
        pos: DocPosition,
        page_number: usize,
    ) -> Option<Rect> {
        let page = self
            .pages
            .iter()
            .find(|p| p.page_number == page_number)
            .or_else(|| self.pages.first())?;
        match focus {
            EditFocus::Header => {
                page.margin_caret_rect(&page.header_lines, page.header_band(), pos)
            }
            EditFocus::Footer => {
                page.margin_caret_rect(&page.footer_lines, page.footer_band(), pos)
            }
            _ => None,
        }
    }

    pub fn margin_selection_rects(
        &self,
        focus: EditFocus,
        selection: Selection,
        page_number: usize,
    ) -> Vec<Rect> {
        let Some(page) = self
            .pages
            .iter()
            .find(|p| p.page_number == page_number)
            .or_else(|| self.pages.first())
        else {
            return Vec::new();
        };
        match focus {
            EditFocus::Header => page.margin_selection_rects(&page.header_lines, selection),
            EditFocus::Footer => page.margin_selection_rects(&page.footer_lines, selection),
            _ => Vec::new(),
        }
    }

    pub fn cell_caret_rect(&self, addr: CellAddress, pos: DocPosition) -> Option<Rect> {
        for page in &self.pages {
            for dec in &page.decorations {
                if let PageDecoration::Table { cells, .. } = dec {
                    if let Some(cell) = cells.iter().find(|c| c.address == addr) {
                        return cell_layout_caret_rect(cell, pos);
                    }
                }
            }
        }
        None
    }

    pub fn cell_selection_rects(&self, addr: CellAddress, selection: Selection) -> Vec<Rect> {
        for page in &self.pages {
            for dec in &page.decorations {
                if let PageDecoration::Table { cells, .. } = dec {
                    if let Some(cell) = cells.iter().find(|c| c.address == addr) {
                        return cell_layout_selection_rects(cell, selection);
                    }
                }
            }
        }
        Vec::new()
    }

    pub fn line_index_containing(&self, pos: DocPosition) -> Option<(usize, usize)> {
        for (pi, page) in self.pages.iter().enumerate() {
            if let Some(li) = page.line_index_containing(pos) {
                return Some((pi, li));
            }
        }
        None
    }

    pub fn caret_x(&self, pos: DocPosition) -> Option<f32> {
        self.caret_rect(pos).map(|r| r.left())
    }

    pub fn caret_rect(&self, pos: DocPosition) -> Option<Rect> {
        for page in &self.pages {
            if let Some(r) = page.caret_rect(pos) {
                return Some(r);
            }
        }
        None
    }

    pub fn selection_rects(&self, selection: Selection) -> Vec<Rect> {
        self.pages
            .iter()
            .flat_map(|p| p.selection_rects(selection))
            .collect()
    }

    pub fn move_vertically(
        &self,
        pos: DocPosition,
        preferred_x: f32,
        direction: VerticalDir,
    ) -> DocPosition {
        let flat: Vec<&LayoutLine> = self.pages.iter().flat_map(|p| p.lines.iter()).collect();
        if flat.is_empty() {
            return pos;
        }
        let Some(idx) = flat.iter().position(|line| {
            line.paragraph == pos.paragraph
                && pos.offset >= line.start_offset
                && pos.offset <= line.end_offset
        }) else {
            return pos;
        };
        let target = match direction {
            VerticalDir::Up => idx.checked_sub(1),
            VerticalDir::Down => {
                let next = idx + 1;
                (next < flat.len()).then_some(next)
            }
        };
        let Some(target) = target else {
            return match direction {
                VerticalDir::Up => {
                    DocPosition::new(flat[idx].paragraph, flat[idx].start_offset)
                }
                VerticalDir::Down => {
                    DocPosition::new(flat[idx].paragraph, flat[idx].end_offset)
                }
            };
        };
        hit_test_line(flat[target], preferred_x)
    }

    pub fn line_home(&self, pos: DocPosition) -> DocPosition {
        for page in &self.pages {
            if let Some(i) = page.line_index_containing(pos) {
                let line = &page.lines[i];
                return DocPosition::new(line.paragraph, line.start_offset);
            }
        }
        pos
    }

    pub fn line_end(&self, pos: DocPosition) -> DocPosition {
        for page in &self.pages {
            if let Some(i) = page.line_index_containing(pos) {
                let line = &page.lines[i];
                return DocPosition::new(line.paragraph, line.end_offset);
            }
        }
        pos
    }
}

impl PageLayout {
    pub fn header_band(&self) -> Rect {
        Rect::from_min_max(
            Pos2::new(self.content_rect.left(), self.page_rect.top()),
            Pos2::new(self.content_rect.right(), self.content_rect.top()),
        )
    }

    pub fn footer_band(&self) -> Rect {
        Rect::from_min_max(
            Pos2::new(self.content_rect.left(), self.content_rect.bottom()),
            Pos2::new(self.content_rect.right(), self.page_rect.bottom()),
        )
    }

    pub fn hit_test(&self, pos: Pos2) -> DocPosition {
        match self.hit_test_full(pos) {
            HitResult::Body(p) | HitResult::Header(p) | HitResult::Footer(p) => p,
            HitResult::Cell { offset, .. } => DocPosition::new(0, offset),
        }
    }

    pub fn hit_test_full(&self, pos: Pos2) -> HitResult {
        // Prefer table cells when the pointer is inside a table.
        for dec in &self.decorations {
            if let PageDecoration::Table { rect, cells, .. } = dec {
                if rect.contains(pos) {
                    for cell in cells {
                        if cell.rect.contains(pos) {
                            let offset = hit_test_cell_offset(cell, pos);
                            return HitResult::Cell {
                                address: cell.address,
                                offset,
                            };
                        }
                    }
                    if let Some(cell) = cells.first() {
                        return HitResult::Cell {
                            address: cell.address,
                            offset: 0,
                        };
                    }
                }
            }
        }

        if self.header_band().contains(pos) {
            return HitResult::Header(hit_test_margin_lines(&self.header_lines, pos));
        }
        if self.footer_band().contains(pos) {
            return HitResult::Footer(hit_test_margin_lines(&self.footer_lines, pos));
        }

        if self.lines.is_empty() {
            return HitResult::Body(DocPosition::zero());
        }
        if pos.y < self.lines[0].rect.top() {
            return HitResult::Body(DocPosition::new(
                self.lines[0].paragraph,
                self.lines[0].start_offset,
            ));
        }
        for line in &self.lines {
            if pos.y <= line.rect.bottom() {
                return HitResult::Body(hit_test_line(line, pos.x));
            }
        }
        let last = self.lines.last().unwrap();
        HitResult::Body(DocPosition::new(last.paragraph, last.end_offset))
    }

    fn margin_caret_rect(&self, lines: &[LayoutLine], band: Rect, pos: DocPosition) -> Option<Rect> {
        if lines.is_empty() {
            return Some(Rect::from_min_size(
                Pos2::new(band.left() + 2.0, band.top() + 12.0),
                Vec2::new(1.5, 14.0),
            ));
        }
        let tmp = PageLayout {
            page_rect: band,
            content_rect: band,
            lines: lines.to_vec(),
            decorations: Vec::new(),
            header_lines: Vec::new(),
            footer_lines: Vec::new(),
            page_number: self.page_number,
        };
        tmp.caret_rect(pos).or_else(|| {
            Some(Rect::from_min_size(
                Pos2::new(band.left() + 2.0, band.top() + 12.0),
                Vec2::new(1.5, 14.0),
            ))
        })
    }

    fn margin_selection_rects(&self, lines: &[LayoutLine], selection: Selection) -> Vec<Rect> {
        if selection.is_collapsed() || lines.is_empty() {
            return Vec::new();
        }
        let tmp = PageLayout {
            page_rect: self.page_rect,
            content_rect: self.content_rect,
            lines: lines.to_vec(),
            decorations: Vec::new(),
            header_lines: Vec::new(),
            footer_lines: Vec::new(),
            page_number: self.page_number,
        };
        tmp.selection_rects(selection)
    }

    pub fn line_index_containing(&self, pos: DocPosition) -> Option<usize> {
        self.lines.iter().position(|line| {
            line.paragraph == pos.paragraph
                && pos.offset >= line.start_offset
                && pos.offset <= line.end_offset
        })
    }

    pub fn caret_rect(&self, pos: DocPosition) -> Option<Rect> {
        for line in &self.lines {
            if line.paragraph != pos.paragraph {
                continue;
            }
            if pos.offset < line.start_offset || pos.offset > line.end_offset {
                continue;
            }
            if pos.offset == line.end_offset {
                let x = line
                    .cells
                    .last()
                    .map(|c| c.rect.right())
                    .unwrap_or(line.rect.left());
                return Some(Rect::from_min_size(
                    Pos2::new(x, line.rect.top()),
                    Vec2::new(1.5, line.rect.height().max(12.0)),
                ));
            }
            if let Some(cell) = line.cells.iter().find(|c| c.char_offset == pos.offset) {
                return Some(Rect::from_min_size(
                    Pos2::new(cell.rect.left(), line.rect.top()),
                    Vec2::new(1.5, line.rect.height().max(12.0)),
                ));
            }
        }
        self.lines
            .iter()
            .find(|l| l.paragraph == pos.paragraph)
            .map(|line| {
                Rect::from_min_size(
                    Pos2::new(line.rect.left(), line.rect.top()),
                    Vec2::new(1.5, line.rect.height().max(12.0)),
                )
            })
    }

    pub fn selection_rects(&self, selection: Selection) -> Vec<Rect> {
        if selection.is_collapsed() {
            return Vec::new();
        }
        let start = selection.start();
        let end = selection.end();
        let mut rects = Vec::new();
        for line in &self.lines {
            if line.paragraph < start.paragraph || line.paragraph > end.paragraph {
                continue;
            }
            let from = if line.paragraph == start.paragraph {
                start.offset.max(line.start_offset)
            } else {
                line.start_offset
            };
            let to = if line.paragraph == end.paragraph {
                end.offset.min(line.end_offset)
            } else {
                line.end_offset
            };
            if from >= to {
                continue;
            }
            let left = line
                .cells
                .iter()
                .find(|c| c.char_offset == from)
                .map(|c| c.rect.left())
                .unwrap_or(line.rect.left());
            let right = if to >= line.end_offset {
                line.cells
                    .last()
                    .map(|c| c.rect.right())
                    .unwrap_or(left + 1.0)
            } else {
                line.cells
                    .iter()
                    .find(|c| c.char_offset == to)
                    .map(|c| c.rect.left())
                    .unwrap_or(line.rect.right())
            };
            rects.push(Rect::from_min_max(
                Pos2::new(left, line.rect.top()),
                Pos2::new(right.max(left + 1.0), line.rect.bottom()),
            ));
        }
        rects
    }
}

fn hit_test_line(line: &LayoutLine, x: f32) -> DocPosition {
    if line.cells.is_empty() {
        return DocPosition::new(line.paragraph, line.start_offset);
    }
    for cell in &line.cells {
        let mid = (cell.rect.left() + cell.rect.right()) * 0.5;
        if x < mid {
            return DocPosition::new(cell.paragraph, cell.char_offset);
        }
    }
    DocPosition::new(line.paragraph, line.end_offset)
}

fn hit_test_margin_lines(lines: &[LayoutLine], pos: Pos2) -> DocPosition {
    if lines.is_empty() {
        return DocPosition::zero();
    }
    if pos.y < lines[0].rect.top() {
        return DocPosition::new(lines[0].paragraph, lines[0].start_offset);
    }
    for line in lines {
        if pos.y <= line.rect.bottom() {
            return hit_test_line(line, pos.x);
        }
    }
    let last = lines.last().unwrap();
    DocPosition::new(last.paragraph, last.end_offset)
}

/// Vertical gap between stacked pages in the canvas.
const PAGE_GAP: f32 = 24.0;

struct PageMetrics {
    origin: Pos2,
    zoom: f32,
    page_size: Vec2,
    margin_left: f32,
    margin_top: f32,
    margin_right: f32,
    margin_bottom: f32,
    content_width: f32,
    content_height: f32,
}

/// Layout the document into one or more A4 pages.
pub fn layout_document(ui: &Ui, document: &Document, origin: Pos2, zoom: f32) -> DocumentLayout {
    let page_style = document.page_style();
    let page_size = Vec2::new(page_style.width, page_style.height) * zoom;
    let margin_top = page_style.margin_top * zoom;
    let margin_bottom = page_style.margin_bottom * zoom;
    let margin_left = page_style.margin_left * zoom;
    let margin_right = page_style.margin_right * zoom;
    let metrics = PageMetrics {
        origin,
        zoom,
        page_size,
        margin_left,
        margin_top,
        margin_right,
        margin_bottom,
        content_width: (page_size.x - margin_left - margin_right).max(1.0),
        content_height: (page_size.y - margin_top - margin_bottom).max(1.0),
    };
    layout_document_paginated(ui, document, &metrics)
}

fn layout_document_paginated(
    ui: &Ui,
    document: &Document,
    metrics: &PageMetrics,
) -> DocumentLayout {
    let PageMetrics {
        origin,
        zoom,
        page_size,
        margin_left,
        margin_top,
        margin_right,
        margin_bottom,
        content_width,
        content_height,
    } = *metrics;

    let flow_left = 0.0_f32;
    let mut flow_lines: Vec<LayoutLine> = Vec::new();
    let mut flow_decorations: Vec<(f32, PageDecoration)> = Vec::new(); // (flow_top, dec)
    let mut forced_breaks: Vec<f32> = Vec::new();
    let mut cursor_y = 0.0_f32;
    let mut para_idx = 0usize;

    for (sec_i, section) in document.sections.iter().enumerate() {
        if sec_i > 0 {
            forced_breaks.push(cursor_y);
        }
        for (block_idx, block) in section.blocks.iter().enumerate() {
            match block {
                Block::Paragraph(paragraph) => {
                    let (indent, marker) = list_layout_extras(document, para_idx, paragraph, zoom);
                    let left = flow_left + indent;
                    let width = (content_width - indent).max(8.0);
                    let mut para_lines = layout_paragraph(
                        ui,
                        paragraph,
                        para_idx,
                        left,
                        cursor_y,
                        width,
                        zoom,
                    );
                    if let Some(first) = para_lines.first_mut() {
                        first.list_marker = marker.clone();
                    }
                    if let Some(last) = para_lines.last() {
                        cursor_y = last.rect.bottom() + paragraph.style.space_after * zoom;
                        flow_lines.extend(para_lines);
                    } else {
                        let font_size = 12.0 * zoom;
                        let height = font_size * paragraph.style.line_spacing;
                        let rect = Rect::from_min_size(
                            Pos2::new(left, cursor_y),
                            Vec2::new(width, height),
                        );
                        let galley = ui.fonts_mut(|f| {
                            f.layout(
                                String::new(),
                                FontId::new(font_size, FontFamily::Proportional),
                                Color32::BLACK,
                                width,
                            )
                        });
                        flow_lines.push(LayoutLine {
                            paragraph: para_idx,
                            start_offset: 0,
                            end_offset: 0,
                            rect,
                            galley,
                            galley_origin_x: left,
                            row_offset_y: 0.0,
                            cells: Vec::new(),
                            list_marker: marker,
                        });
                        cursor_y = rect.bottom() + paragraph.style.space_after * zoom;
                    }
                    para_idx += 1;
                }
                Block::Table(table) => {
                    let (height, decoration) = layout_table_decoration(
                        ui,
                        table,
                        sec_i,
                        block_idx,
                        flow_left,
                        cursor_y,
                        content_width,
                        zoom,
                    );
                    flow_decorations.push((cursor_y, decoration));
                    cursor_y += height + 8.0 * zoom;
                }
                Block::Image(image) => {
                    let (height, decoration) =
                        layout_image_decoration(image, flow_left, cursor_y, content_width, zoom);
                    flow_decorations.push((cursor_y, decoration));
                    cursor_y += height + 8.0 * zoom;
                }
                Block::PageBreak => {
                    forced_breaks.push(cursor_y);
                }
            }
        }
    }

    if flow_lines.is_empty() && flow_decorations.is_empty() && forced_breaks.is_empty() {
        let page_origin = origin;
        let page_rect = Rect::from_min_size(page_origin, page_size);
        let content_rect = Rect::from_min_max(
            Pos2::new(page_origin.x + margin_left, page_origin.y + margin_top),
            Pos2::new(
                page_origin.x + page_size.x - margin_right,
                page_origin.y + page_size.y - margin_bottom,
            ),
        );
        let mut layout = DocumentLayout {
            pages: vec![PageLayout {
                page_rect,
                content_rect,
                lines: Vec::new(),
                decorations: Vec::new(),
                header_lines: Vec::new(),
                footer_lines: Vec::new(),
                page_number: 1,
            }],
        };
        layout_header_footer(ui, document, &mut layout, zoom);
        return layout;
    }

    // Unified vertical extents for pagination.
    let mut extents: Vec<(f32, f32)> = flow_lines
        .iter()
        .map(|l| (l.rect.top(), l.rect.bottom()))
        .collect();
    for (top, dec) in &flow_decorations {
        let h = match dec {
            PageDecoration::Table { rect, .. } | PageDecoration::Image { rect, .. } => {
                rect.height()
            }
        };
        extents.push((*top, *top + h));
    }
    extents.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    let mut page_breaks: Vec<f32> = vec![0.0]; // flow_y starts of pages
    let mut page_start = 0.0_f32;
    for &(top, bottom) in &extents {
        for &fy in &forced_breaks {
            if fy > page_start + f32::EPSILON && fy <= top + f32::EPSILON {
                page_breaks.push(fy);
                page_start = fy;
            }
        }
        if bottom - page_start > content_height && top > page_start {
            page_breaks.push(top);
            page_start = top;
        }
    }
    // Trailing explicit breaks with no following content still open a new page.
    for &fy in &forced_breaks {
        if fy > page_start + f32::EPSILON {
            page_breaks.push(fy);
            page_start = fy;
        }
    }
    let total_pages = page_breaks.len();

    let mut pages: Vec<PageLayout> = (0..total_pages)
        .map(|i| {
            let page_origin = Pos2::new(
                origin.x,
                origin.y + i as f32 * (page_size.y + PAGE_GAP * zoom.max(0.5)),
            );
            let page_rect = Rect::from_min_size(page_origin, page_size);
            let content_rect = Rect::from_min_max(
                Pos2::new(page_origin.x + margin_left, page_origin.y + margin_top),
                Pos2::new(
                    page_origin.x + page_size.x - margin_right,
                    page_origin.y + page_size.y - margin_bottom,
                ),
            );
            PageLayout {
                page_rect,
                content_rect,
                lines: Vec::new(),
                decorations: Vec::new(),
                header_lines: Vec::new(),
                footer_lines: Vec::new(),
                page_number: i + 1,
            }
        })
        .collect();

    let page_index_for_y = |y: f32| -> usize {
        let mut idx = 0usize;
        for (i, start) in page_breaks.iter().enumerate() {
            if y + f32::EPSILON >= *start {
                idx = i;
            }
        }
        idx
    };

    for mut line in flow_lines {
        let p = page_index_for_y(line.rect.top());
        let flow_origin = page_breaks[p];
        let content = pages[p].content_rect;
        let local_top = line.rect.top() - flow_origin;
        // Preserve flow-space X (list indent); only shift by the page content origin.
        let delta = Vec2::new(
            content.left() - flow_left,
            content.top() + local_top - line.rect.top(),
        );
        line.translate(delta);
        line.galley_origin_x += delta.x;
        pages[p].lines.push(line);
    }

    for (top, mut dec) in flow_decorations {
        let p = page_index_for_y(top);
        let flow_origin = page_breaks[p];
        let content = pages[p].content_rect;
        let local_top = top - flow_origin;
        let delta = Vec2::new(content.left() - flow_left, content.top() + local_top - top);
        match &mut dec {
            PageDecoration::Table { rect, cells, .. } => {
                *rect = rect.translate(delta);
                for cell in cells {
                    cell.rect = cell.rect.translate(delta);
                    for line in &mut cell.lines {
                        line.translate(delta);
                        line.galley_origin_x += delta.x;
                    }
                }
            }
            PageDecoration::Image { rect, .. } => {
                *rect = rect.translate(delta);
            }
        }
        pages[p].decorations.push(dec);
    }

    let mut layout = DocumentLayout { pages };
    layout_header_footer(ui, document, &mut layout, zoom);
    layout
}

fn expand_page_fields(text: &str, page: usize, pages: usize) -> String {
    text.replace("{page}", &page.to_string())
        .replace("{pages}", &pages.to_string())
}

fn paragraph_with_page_fields(para: &Paragraph, page: usize, pages: usize) -> Paragraph {
    let mut out = para.clone();
    for run in &mut out.runs {
        run.text = expand_page_fields(&run.text, page, pages);
    }
    out
}

fn layout_header_footer(ui: &Ui, document: &Document, layout: &mut DocumentLayout, zoom: f32) {
    let page_count = layout.pages.len().max(1);
    let header = document.header().cloned();
    let footer = document.footer().cloned();
    for page in &mut layout.pages {
        let page_no = page.page_number;
        let width = page.content_rect.width().max(8.0);
        let left = page.content_rect.left();
        if let Some(ref header_para) = header {
            let para = paragraph_with_page_fields(header_para, page_no, page_count);
            let band_top = page.page_rect.top() + 12.0 * zoom;
            let band_bottom = page.content_rect.top() - 4.0 * zoom;
            let max_h = (band_bottom - band_top).max(8.0);
            let mut lines = layout_paragraph(ui, &para, 0, left, band_top, width, zoom);
            lines.retain(|l| l.rect.top() < band_top + max_h);
            page.header_lines = lines;
        }
        if let Some(ref footer_para) = footer {
            let para = paragraph_with_page_fields(footer_para, page_no, page_count);
            let band_top = page.content_rect.bottom() + 4.0 * zoom;
            let band_bottom = page.page_rect.bottom() - 12.0 * zoom;
            let max_h = (band_bottom - band_top).max(8.0);
            let mut lines = layout_paragraph(ui, &para, 0, left, band_top, width, zoom);
            lines.retain(|l| l.rect.top() < band_top + max_h);
            page.footer_lines = lines;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn layout_table_decoration(
    ui: &Ui,
    table: &Table,
    section_index: usize,
    block_index: usize,
    left: f32,
    top: f32,
    width: f32,
    zoom: f32,
) -> (f32, PageDecoration) {
    let rows = table.row_count().max(1);
    let cols = table.column_count().max(1);
    let pad = 4.0 * zoom;
    let min_row_h = 22.0 * zoom;
    let col_w = width / cols as f32;

    // Measure content heights with a dry layout at the origin.
    let mut row_heights = vec![min_row_h; rows];
    for (r, row) in table.rows.iter().enumerate() {
        for cell in row.cells.iter().take(cols) {
            let content_w = (col_w - pad * 2.0).max(8.0);
            let content_h = if let Some(para) = cell.paragraphs.first() {
                if para.is_empty() {
                    12.0 * zoom
                } else {
                    layout_paragraph(ui, para, 0, 0.0, 0.0, content_w, zoom)
                        .last()
                        .map(|l| l.rect.bottom())
                        .unwrap_or(12.0 * zoom)
                }
            } else {
                12.0 * zoom
            };
            row_heights[r] = row_heights[r].max(content_h + pad * 2.0);
        }
    }

    let height: f32 = row_heights.iter().sum();
    let rect = Rect::from_min_size(Pos2::new(left, top), Vec2::new(width, height));

    let mut cells = Vec::new();
    let mut y = top;
    for (r, row_h) in row_heights.iter().enumerate() {
        for c in 0..cols {
            let x = left + c as f32 * col_w;
            let cell_rect = Rect::from_min_size(Pos2::new(x, y), Vec2::new(col_w, *row_h));
            let content_w = (col_w - pad * 2.0).max(8.0);
            let origin = Pos2::new(x + pad, y + pad);
            let lines = table
                .rows
                .get(r)
                .and_then(|row| row.cells.get(c))
                .and_then(|cell| cell.paragraphs.first())
                .map(|para| {
                    if para.is_empty() {
                        Vec::new()
                    } else {
                        layout_paragraph(ui, para, 0, origin.x, origin.y, content_w, zoom)
                    }
                })
                .unwrap_or_default();
            cells.push(TableCellLayout {
                address: CellAddress::in_section(section_index, block_index, r, c),
                rect: cell_rect,
                lines,
            });
        }
        y += *row_h;
    }

    (
        height,
        PageDecoration::Table {
            rect,
            block_index,
            cells,
        },
    )
}

fn layout_image_decoration(
    image: &Image,
    left: f32,
    top: f32,
    width: f32,
    zoom: f32,
) -> (f32, PageDecoration) {
    let w = image.width_pt * zoom;
    let h = image.height_pt * zoom;
    let draw_w = w.min(width);
    let draw_h = if w > 0.0 { h * (draw_w / w) } else { h };
    let rect = Rect::from_min_size(Pos2::new(left, top), Vec2::new(draw_w, draw_h.max(24.0 * zoom)));
    (
        rect.height(),
        PageDecoration::Image {
            rect,
            label: image.display_name().to_string(),
            cache_key: image.cache_key(),
        },
    )
}

fn layout_paragraph(
    ui: &Ui,
    paragraph: &Paragraph,
    paragraph_index: usize,
    left: f32,
    top: f32,
    width: f32,
    zoom: f32,
) -> Vec<LayoutLine> {
    if paragraph.is_empty() {
        return Vec::new();
    }

    let mut job = LayoutJob {
        halign: align_egui(paragraph.style.alignment),
        wrap: egui::text::TextWrapping {
            max_width: width,
            ..Default::default()
        },
        ..Default::default()
    };

    for run in &paragraph.runs {
        job.append(&run.text, 0.0, format_for_run(&run.style, run.link.as_deref(), zoom));
    }

    let galley = ui.fonts_mut(|f| f.layout_job(job));
    let mut lines = Vec::new();
    let mut char_offset = 0usize;
    let line_spacing = paragraph.style.line_spacing;

    for row in galley.rows.iter() {
        let row_rect = row.rect();
        let row_height = (row_rect.height() * line_spacing).max(1.0);
        let row_offset_y = row_rect.min.y * line_spacing;
        let y = top + row_offset_y;
        let full_line_rect = Rect::from_min_size(Pos2::new(left, y), Vec2::new(width, row_height));

        let start_offset = char_offset;
        let mut cells = Vec::new();
        for glyph in &row.glyphs {
            let gx = left + row.pos.x + glyph.pos.x;
            let gw = glyph.size().x.max(1.0);
            cells.push(GlyphCell {
                paragraph: paragraph_index,
                char_offset,
                rect: Rect::from_min_size(Pos2::new(gx, y), Vec2::new(gw, row_height)),
            });
            char_offset += 1;
        }
        let end_offset = char_offset;

        lines.push(LayoutLine {
            paragraph: paragraph_index,
            start_offset,
            end_offset,
            rect: full_line_rect,
            galley: galley.clone(),
            galley_origin_x: left,
            row_offset_y,
            cells,
            list_marker: None,
        });
    }

    lines
}

fn align_egui(alignment: Alignment) -> Align {
    match alignment {
        Alignment::Left | Alignment::Justify => Align::LEFT,
        Alignment::Center => Align::Center,
        Alignment::Right => Align::RIGHT,
    }
}

fn list_layout_extras(
    document: &Document,
    para_idx: usize,
    paragraph: &Paragraph,
    zoom: f32,
) -> (f32, Option<String>) {
    let Some(list) = paragraph.style.list else {
        return (0.0, None);
    };
    let indent = 22.0 * zoom * (list.level as f32 + 1.0);
    let marker = match list.kind {
        ListKind::Bullet => {
            const BULLETS: &[&str] = &["•", "○", "■", "–"];
            BULLETS[list.level as usize % BULLETS.len()].to_string()
        }
        ListKind::Numbered => {
            let n = numbered_item_index(document, para_idx, list.level);
            format!("{n}.")
        }
    };
    (indent, Some(marker))
}

fn numbered_item_index(document: &Document, para_idx: usize, level: u8) -> usize {
    let paras: Vec<&Paragraph> = document
        .blocks()
        .iter()
        .filter_map(Block::as_paragraph)
        .collect();
    let mut n = 1usize;
    let mut i = para_idx;
    while i > 0 {
        i -= 1;
        match paras.get(i).and_then(|p| p.style.list) {
            Some(list) if list.kind == ListKind::Numbered && list.level == level => n += 1,
            // Nested items under a previous sibling do not reset the counter.
            Some(list) if list.kind == ListKind::Numbered && list.level > level => {}
            _ => break,
        }
    }
    n
}

fn format_for_run(style: &TextStyle, link: Option<&str>, zoom: f32) -> TextFormat {
    let mut size = style.font_size * zoom;
    if style.bold {
        size *= 1.04;
    }
    let linked = link.is_some();
    let color = if linked {
        Color32::from_rgb(20, 80, 180)
    } else if style.bold {
        Color32::BLACK
    } else {
        Color32::from_rgb(25, 25, 25)
    };
    TextFormat {
        font_id: FontId::new(size, FontFamily::Proportional),
        color,
        italics: style.italic,
        underline: if style.underline || linked {
            Stroke::new((1.0 * zoom).max(0.8), color)
        } else {
            Stroke::NONE
        },
        extra_letter_spacing: if style.bold { 0.25 * zoom } else { 0.0 },
        ..Default::default()
    }
}

/// Paint all pages: paper, selection, text (clipped per page), and caret.
pub fn paint_document(
    ui: &Ui,
    layout: &DocumentLayout,
    selection: Selection,
    caret: DocPosition,
    show_caret: bool,
    focus: EditFocus,
    image_textures: &HashMap<String, TextureId>,
) {
    for page in &layout.pages {
        paint_page_chrome(ui, page);
    }

    match focus {
        EditFocus::Body => {
            for rect in layout.selection_rects(selection) {
                ui.painter().rect_filled(
                    rect,
                    0.0,
                    Color32::from_rgba_unmultiplied(80, 140, 220, 90),
                );
            }
        }
        EditFocus::Cell(addr) => {
            for rect in layout.cell_selection_rects(addr, selection) {
                ui.painter().rect_filled(
                    rect,
                    0.0,
                    Color32::from_rgba_unmultiplied(80, 140, 220, 90),
                );
            }
            // Highlight active cell.
            for page in &layout.pages {
                for dec in &page.decorations {
                    if let PageDecoration::Table { cells, .. } = dec {
                        if let Some(cell) = cells.iter().find(|c| c.address == addr) {
                            ui.painter().rect_stroke(
                                cell.rect,
                                0.0,
                                Stroke::new(1.5, Color32::from_rgb(40, 110, 200)),
                                StrokeKind::Inside,
                            );
                        }
                    }
                }
            }
        }
        EditFocus::Header | EditFocus::Footer => {
            for page in &layout.pages {
                let band = if matches!(focus, EditFocus::Header) {
                    page.header_band()
                } else {
                    page.footer_band()
                };
                ui.painter().rect_stroke(
                    band,
                    0.0,
                    Stroke::new(1.0, Color32::from_rgb(40, 110, 200)),
                    StrokeKind::Inside,
                );
                for rect in layout.margin_selection_rects(focus, selection, page.page_number) {
                    ui.painter().rect_filled(
                        rect,
                        0.0,
                        Color32::from_rgba_unmultiplied(80, 140, 220, 90),
                    );
                }
            }
        }
    }

    for page in &layout.pages {
        paint_page_text(ui, page);
        paint_margin_text(ui, page);
        paint_page_decorations(ui, page, focus, image_textures);
    }

    if show_caret {
        let caret_rects: Vec<Rect> = match focus {
            EditFocus::Body => layout.caret_rect(caret).into_iter().collect(),
            EditFocus::Cell(addr) => layout.cell_caret_rect(addr, caret).into_iter().collect(),
            EditFocus::Header | EditFocus::Footer => layout
                .pages
                .iter()
                .filter_map(|p| layout.margin_caret_rect(focus, caret, p.page_number))
                .collect(),
        };
        for rect in caret_rects {
            ui.painter()
                .rect_filled(rect, 0.0, Color32::from_rgb(20, 20, 20));
        }
    }
}

fn hit_test_cell_offset(cell: &TableCellLayout, pos: Pos2) -> usize {
    if cell.lines.is_empty() {
        return 0;
    }
    if pos.y < cell.lines[0].rect.top() {
        return cell.lines[0].start_offset;
    }
    for line in &cell.lines {
        if pos.y <= line.rect.bottom() {
            return hit_test_line(line, pos.x).offset;
        }
    }
    cell.lines.last().map(|l| l.end_offset).unwrap_or(0)
}

fn cell_layout_caret_rect(cell: &TableCellLayout, pos: DocPosition) -> Option<Rect> {
    if cell.lines.is_empty() {
        return Some(Rect::from_min_size(
            Pos2::new(cell.rect.left() + 4.0, cell.rect.top() + 4.0),
            Vec2::new(1.5, 14.0),
        ));
    }
    // Reuse PageLayout caret logic by building a temporary page.
    let page = PageLayout {
        page_rect: cell.rect,
        content_rect: cell.rect,
        lines: cell.lines.clone(),
        decorations: Vec::new(),
        header_lines: Vec::new(),
        footer_lines: Vec::new(),
        page_number: 1,
    };
    page.caret_rect(pos).or_else(|| {
        Some(Rect::from_min_size(
            Pos2::new(cell.rect.left() + 4.0, cell.rect.top() + 4.0),
            Vec2::new(1.5, cell.rect.height().min(18.0)),
        ))
    })
}

fn cell_layout_selection_rects(cell: &TableCellLayout, selection: Selection) -> Vec<Rect> {
    if selection.is_collapsed() || cell.lines.is_empty() {
        return Vec::new();
    }
    let page = PageLayout {
        page_rect: cell.rect,
        content_rect: cell.rect,
        lines: cell.lines.clone(),
        decorations: Vec::new(),
        header_lines: Vec::new(),
        footer_lines: Vec::new(),
        page_number: 1,
    };
    page.selection_rects(selection)
}

fn paint_page_chrome(ui: &Ui, page: &PageLayout) {
    let painter = ui.painter();
    let shadow = page.page_rect.translate(Vec2::new(4.0, 4.0));
    painter.rect_filled(shadow, 0.0, Color32::from_black_alpha(40));
    painter.rect_filled(page.page_rect, 0.0, Color32::WHITE);
    painter.rect_stroke(
        page.page_rect,
        0.0,
        Stroke::new(1.0, Color32::from_gray(180)),
        StrokeKind::Outside,
    );
}

fn paint_page_text(ui: &Ui, page: &PageLayout) {
    // Clip to content so galley rows belonging to other pages are hidden.
    let painter = ui.painter().with_clip_rect(page.content_rect);
    let marker_font = FontId::new(12.0, FontFamily::Proportional);

    // One paint per paragraph segment on this page.
    let mut i = 0;
    while i < page.lines.len() {
        let para = page.lines[i].paragraph;
        let first = &page.lines[i];
        if let Some(marker) = &first.list_marker {
            let x = (first.rect.left() - 16.0).max(page.content_rect.left());
            painter.text(
                Pos2::new(x, first.rect.top()),
                egui::Align2::LEFT_TOP,
                marker,
                marker_font.clone(),
                Color32::from_rgb(30, 30, 30),
            );
        }
        let origin = first.galley_paint_origin();
        painter.galley(origin, first.galley.clone(), Color32::BLACK);
        while i < page.lines.len() && page.lines[i].paragraph == para {
            i += 1;
        }
    }
}

fn paint_margin_text(ui: &Ui, page: &PageLayout) {
    let paint_band = |lines: &[LayoutLine], clip: Rect| {
        if lines.is_empty() {
            return;
        }
        let painter = ui.painter().with_clip_rect(clip);
        let mut i = 0;
        while i < lines.len() {
            let para = lines[i].paragraph;
            let first = &lines[i];
            painter.galley(first.galley_paint_origin(), first.galley.clone(), Color32::DARK_GRAY);
            while i < lines.len() && lines[i].paragraph == para {
                i += 1;
            }
        }
    };
    let header_clip = Rect::from_min_max(
        Pos2::new(page.content_rect.left(), page.page_rect.top()),
        Pos2::new(page.content_rect.right(), page.content_rect.top()),
    );
    let footer_clip = Rect::from_min_max(
        Pos2::new(page.content_rect.left(), page.content_rect.bottom()),
        Pos2::new(page.content_rect.right(), page.page_rect.bottom()),
    );
    paint_band(&page.header_lines, header_clip);
    paint_band(&page.footer_lines, footer_clip);
}

fn paint_page_decorations(
    ui: &Ui,
    page: &PageLayout,
    focus: EditFocus,
    image_textures: &HashMap<String, TextureId>,
) {
    let painter = ui.painter().with_clip_rect(page.content_rect);
    let stroke = Stroke::new(1.0, Color32::from_gray(90));
    let font = FontId::new(11.0, FontFamily::Proportional);
    let _ = focus;

    for dec in &page.decorations {
        match dec {
            PageDecoration::Table { rect, cells, .. } => {
                painter.rect_filled(*rect, 0.0, Color32::from_gray(248));
                painter.rect_stroke(*rect, 0.0, stroke, StrokeKind::Inside);
                for cell in cells {
                    painter.rect_stroke(cell.rect, 0.0, stroke, StrokeKind::Inside);
                    // Paint laid-out cell text.
                    let mut i = 0;
                    while i < cell.lines.len() {
                        let first = &cell.lines[i];
                        let origin = first.galley_paint_origin();
                        painter.galley(origin, first.galley.clone(), Color32::BLACK);
                        let para = first.paragraph;
                        while i < cell.lines.len() && cell.lines[i].paragraph == para {
                            i += 1;
                        }
                    }
                }
            }
            PageDecoration::Image {
                rect,
                label,
                cache_key,
            } => {
                if let Some(tex) = image_textures.get(cache_key) {
                    painter.image(
                        *tex,
                        *rect,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                        Color32::WHITE,
                    );
                    painter.rect_stroke(*rect, 0.0, stroke, StrokeKind::Outside);
                } else {
                    painter.rect_filled(*rect, 0.0, Color32::from_gray(230));
                    painter.rect_stroke(*rect, 0.0, stroke, StrokeKind::Inside);
                    painter.line_segment([rect.left_top(), rect.right_bottom()], stroke);
                    painter.line_segment([rect.right_top(), rect.left_bottom()], stroke);
                    painter.text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        label,
                        font.clone(),
                        Color32::from_rgb(40, 40, 40),
                    );
                }
            }
        }
    }
}

/// Backward-compatible alias used by older call sites.
pub fn paint_page(
    ui: &Ui,
    layout: &PageLayout,
    selection: Selection,
    caret: DocPosition,
    show_caret: bool,
) {
    let doc = DocumentLayout {
        pages: vec![layout.clone()],
    };
    paint_document(
        ui,
        &doc,
        selection,
        caret,
        show_caret,
        EditFocus::Body,
        &HashMap::new(),
    );
}

/// Caret rectangle for IME candidate positioning.
pub fn ime_caret_screen_rect(layout: &DocumentLayout, caret: DocPosition) -> Option<Rect> {
    layout.caret_rect(caret)
}

/// Total canvas height needed to show all pages (including gaps).
pub fn document_canvas_size(layout: &DocumentLayout) -> Vec2 {
    if layout.pages.is_empty() {
        return Vec2::ZERO;
    }
    let first = layout.pages.first().unwrap().page_rect;
    let last = layout.pages.last().unwrap().page_rect;
    Vec2::new(
        first.width().max(last.width()),
        last.bottom() - first.top(),
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use egui::text::LayoutJob;
    use egui::{Galley, Pos2, Rect, Vec2};
    use office_core::{DocPosition, PageStyle};

    use super::{DocumentLayout, GlyphCell, LayoutLine, PageLayout, VerticalDir};

    #[test]
    fn a4_content_box() {
        let page = PageStyle::default();
        assert!(page.content_width() > 400.0);
        assert!(page.content_height() > 600.0);
    }

    fn stub_line(paragraph: usize, start: usize, end: usize, y: f32, left: f32, width: f32) -> LayoutLine {
        let height = 16.0;
        let rect = Rect::from_min_size(Pos2::new(left, y), Vec2::new(width, height));
        let mut cells = Vec::new();
        for (i, off) in (start..end).enumerate() {
            cells.push(GlyphCell {
                paragraph,
                char_offset: off,
                rect: Rect::from_min_size(
                    Pos2::new(left + i as f32 * 10.0, y),
                    Vec2::new(10.0, height),
                ),
            });
        }
        LayoutLine {
            paragraph,
            start_offset: start,
            end_offset: end,
            rect,
            galley: Arc::new(Galley::concat(Arc::new(LayoutJob::default()), &[], 1.0)),
            galley_origin_x: left,
            row_offset_y: 0.0,
            cells,
            list_marker: None,
        }
    }

    #[test]
    fn vertical_move_across_pages() {
        let layout = DocumentLayout {
            pages: vec![
                PageLayout {
                    page_rect: Rect::from_min_size(Pos2::ZERO, Vec2::new(100.0, 100.0)),
                    content_rect: Rect::from_min_size(Pos2::ZERO, Vec2::new(100.0, 100.0)),
                    lines: vec![stub_line(0, 0, 5, 0.0, 0.0, 50.0)],
                    decorations: Vec::new(),
                    header_lines: Vec::new(),
                    footer_lines: Vec::new(),
                    page_number: 1,
                },
                PageLayout {
                    page_rect: Rect::from_min_size(Pos2::new(0.0, 120.0), Vec2::new(100.0, 100.0)),
                    content_rect: Rect::from_min_size(Pos2::new(0.0, 120.0), Vec2::new(100.0, 100.0)),
                    lines: vec![stub_line(0, 5, 10, 120.0, 0.0, 50.0)],
                    decorations: Vec::new(),
                    header_lines: Vec::new(),
                    footer_lines: Vec::new(),
                    page_number: 2,
                },
            ],
        };
        let from = DocPosition::new(0, 3);
        let down = layout.move_vertically(from, 30.0, VerticalDir::Down);
        assert_eq!(down, DocPosition::new(0, 8));
        assert_eq!(layout.page_of(down), 2);
        assert_eq!(layout.page_count(), 2);
    }

    #[test]
    fn line_home_end() {
        let layout = DocumentLayout {
            pages: vec![PageLayout {
                page_rect: Rect::ZERO,
                content_rect: Rect::ZERO,
                lines: vec![stub_line(0, 0, 5, 0.0, 0.0, 50.0)],
                decorations: Vec::new(),
                header_lines: Vec::new(),
                footer_lines: Vec::new(),
                page_number: 1,
            }],
        };
        let mid = DocPosition::new(0, 2);
        assert_eq!(layout.line_home(mid), DocPosition::new(0, 0));
        assert_eq!(layout.line_end(mid), DocPosition::new(0, 5));
    }

    #[test]
    fn pagination_assigns_overflow_line_to_next_page() {
        let content_height = 40.0;
        let lines = [
            stub_line(0, 0, 1, 0.0, 0.0, 50.0),  // 0..16
            stub_line(0, 1, 2, 16.0, 0.0, 50.0), // 16..32 — still on page 1
            stub_line(0, 2, 3, 32.0, 0.0, 50.0), // 32..48 — overflows
        ];
        let mut page_idx = 0usize;
        let mut page_start = 0.0;
        let mut pages = Vec::new();
        for line in &lines {
            if line.rect.bottom() - page_start > content_height && line.rect.top() > page_start {
                page_idx += 1;
                page_start = line.rect.top();
            }
            pages.push(page_idx);
        }
        assert_eq!(pages, vec![0, 0, 1]);
    }
}
