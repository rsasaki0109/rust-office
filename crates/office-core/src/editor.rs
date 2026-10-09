//! Document editing operations with undo / redo.

use thiserror::Error;

mod search;

use crate::document::{Block, Document, Image, Paragraph, Run, Section, Table};
use crate::selection::{CellAddress, DocPosition, EditFocus, Selection};
use crate::style::{
    apply_named_paragraph_defaults, Alignment, ListKind, ListStyle, NamedParagraphStyle, PageStyle,
    TextStyle,
};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EditError {
    #[error("paragraph index out of range")]
    InvalidParagraph,
    #[error("character offset out of range")]
    InvalidOffset,
    #[error("empty document")]
    EmptyDocument,
    #[error("table cell out of range")]
    InvalidCell,
    #[error("replacement text must not contain paragraph breaks")]
    InvalidReplacement,
    #[error("section index out of range")]
    InvalidSection,
    #[error("invalid page size or margins")]
    InvalidPageStyle,
}

#[derive(Debug, Clone)]
struct Snapshot {
    document: Document,
    selection: Selection,
    edit_focus: EditFocus,
}

/// Mutable editing façade over a [`Document`].
#[derive(Debug, Clone)]
pub struct DocumentEditor {
    document: Document,
    selection: Selection,
    edit_focus: EditFocus,
    /// Style used for newly typed characters when the caret is collapsed.
    typing_style: TextStyle,
    undo_stack: Vec<Snapshot>,
    redo_stack: Vec<Snapshot>,
    dirty: bool,
}

impl Default for DocumentEditor {
    fn default() -> Self {
        Self::new(Document::new())
    }
}

impl DocumentEditor {
    pub fn new(document: Document) -> Self {
        let mut document = document;
        document.main_section_mut().ensure_paragraph();
        Self {
            document,
            selection: Selection::default(),
            edit_focus: EditFocus::Body,
            typing_style: TextStyle::default(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            dirty: false,
        }
    }

    pub fn document(&self) -> &Document {
        &self.document
    }

    pub fn document_mut(&mut self) -> &mut Document {
        &mut self.document
    }

    pub fn selection(&self) -> Selection {
        self.selection
    }

    pub fn edit_focus(&self) -> EditFocus {
        self.edit_focus
    }

    pub fn focus_body(&mut self) {
        self.edit_focus = EditFocus::Body;
        self.selection = self.clamp_selection(self.selection);
        self.sync_typing_style_from_caret();
    }

    /// Move editing into a table cell (caret at given offset in the cell's first paragraph).
    pub fn focus_cell(&mut self, addr: CellAddress, offset: usize) -> Result<(), EditError> {
        let len = self
            .document
            .cell_paragraph(addr)
            .ok_or(EditError::InvalidCell)?
            .char_len();
        self.edit_focus = EditFocus::Cell(addr);
        self.selection = Selection::caret(DocPosition::new(0, offset.min(len)));
        self.sync_typing_style_from_caret();
        Ok(())
    }

    pub fn current_section(&self) -> usize {
        match self.edit_focus {
            EditFocus::Body => self
                .document
                .locate_paragraph(self.selection.focus.paragraph)
                .map(|(section, _)| section)
                .unwrap_or(0),
            EditFocus::Cell(address) => address.section,
            EditFocus::Header(section) | EditFocus::Footer(section) => section,
        }
    }

    pub fn focus_header(&mut self, offset: usize) {
        let _ = self.focus_section_header(0, offset);
    }

    pub fn focus_footer(&mut self, offset: usize) {
        let _ = self.focus_section_footer(0, offset);
    }

    pub fn focus_section_header(&mut self, section: usize, offset: usize) -> Result<(), EditError> {
        if section >= self.document.sections.len() {
            return Err(EditError::InvalidSection);
        }
        let len = self
            .document
            .section_header(section)
            .map(|p| p.char_len())
            .unwrap_or(0);
        self.edit_focus = EditFocus::Header(section);
        self.selection = Selection::caret(DocPosition::new(0, offset.min(len)));
        self.sync_typing_style_from_caret();
        Ok(())
    }

    pub fn focus_section_footer(&mut self, section: usize, offset: usize) -> Result<(), EditError> {
        if section >= self.document.sections.len() {
            return Err(EditError::InvalidSection);
        }
        let len = self
            .document
            .section_footer(section)
            .map(|p| p.char_len())
            .unwrap_or(0);
        self.edit_focus = EditFocus::Footer(section);
        self.selection = Selection::caret(DocPosition::new(0, offset.min(len)));
        self.sync_typing_style_from_caret();
        Ok(())
    }

    pub fn set_selection(&mut self, selection: Selection) {
        if self.edit_focus.is_body() {
            self.selection = self.clamp_selection(selection);
        } else {
            self.selection = self.clamp_selection_in_focus(selection);
        }
        self.sync_typing_style_from_caret();
    }

    pub fn typing_style(&self) -> &TextStyle {
        &self.typing_style
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn mark_clean(&mut self) {
        self.dirty = false;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    fn push_undo(&mut self) {
        self.undo_stack.push(Snapshot {
            document: self.document.clone(),
            selection: self.selection,
            edit_focus: self.edit_focus,
        });
        self.redo_stack.clear();
        // Cap undo history for memory.
        const MAX_UNDO: usize = 200;
        if self.undo_stack.len() > MAX_UNDO {
            self.undo_stack.remove(0);
        }
    }

    pub fn undo(&mut self) -> bool {
        let Some(prev) = self.undo_stack.pop() else {
            return false;
        };
        self.redo_stack.push(Snapshot {
            document: self.document.clone(),
            selection: self.selection,
            edit_focus: self.edit_focus,
        });
        self.document = prev.document;
        self.selection = prev.selection;
        self.edit_focus = prev.edit_focus;
        self.dirty = true;
        self.sync_typing_style_from_caret();
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo_stack.pop() else {
            return false;
        };
        self.undo_stack.push(Snapshot {
            document: self.document.clone(),
            selection: self.selection,
            edit_focus: self.edit_focus,
        });
        self.document = next.document;
        self.selection = next.selection;
        self.edit_focus = next.edit_focus;
        self.dirty = true;
        self.sync_typing_style_from_caret();
        true
    }

    fn block_loc_of_paragraph(&self, paragraph: usize) -> Option<(usize, usize)> {
        self.document.block_loc_of_paragraph(paragraph)
    }

    fn paragraph_index_of_block(&self, section: usize, block_idx: usize) -> usize {
        self.document.paragraph_index_of_block(section, block_idx)
    }

    fn paragraph_at(&self, pos: DocPosition) -> Option<&Paragraph> {
        match self.edit_focus {
            EditFocus::Body => self.document.paragraph(pos.paragraph),
            EditFocus::Cell(addr) => self.document.cell_paragraph(addr),
            EditFocus::Header(section) => self.document.section_header(section),
            EditFocus::Footer(section) => self.document.section_footer(section),
        }
    }

    fn paragraph_at_mut(&mut self, pos: DocPosition) -> Option<&mut Paragraph> {
        match self.edit_focus {
            EditFocus::Body => self.document.paragraph_mut(pos.paragraph),
            EditFocus::Cell(addr) => self.document.cell_paragraph_mut(addr),
            EditFocus::Header(section) => self.document.ensure_section_header_mut(section),
            EditFocus::Footer(section) => self.document.ensure_section_footer_mut(section),
        }
    }

    fn clamp_position(&self, pos: DocPosition) -> DocPosition {
        match self.edit_focus {
            EditFocus::Body => {
                let count = self.document.paragraph_count();
                if count == 0 {
                    return DocPosition::zero();
                }
                let paragraph = pos.paragraph.min(count - 1);
                let offset = pos
                    .offset
                    .min(self.document.paragraph(paragraph).map(|p| p.char_len()).unwrap_or(0));
                DocPosition::new(paragraph, offset)
            }
            EditFocus::Cell(addr) => {
                let len = self
                    .document
                    .cell_paragraph(addr)
                    .map(|p| p.char_len())
                    .unwrap_or(0);
                DocPosition::new(0, pos.offset.min(len))
            }
            EditFocus::Header(section) => {
                let len = self
                    .document
                    .section_header(section)
                    .map(|p| p.char_len())
                    .unwrap_or(0);
                DocPosition::new(0, pos.offset.min(len))
            }
            EditFocus::Footer(section) => {
                let len = self
                    .document
                    .section_footer(section)
                    .map(|p| p.char_len())
                    .unwrap_or(0);
                DocPosition::new(0, pos.offset.min(len))
            }
        }
    }

    fn clamp_selection(&self, sel: Selection) -> Selection {
        Selection {
            anchor: self.clamp_position(sel.anchor),
            focus: self.clamp_position(sel.focus),
        }
    }

    fn clamp_selection_in_focus(&self, sel: Selection) -> Selection {
        self.clamp_selection(sel)
    }

    fn sync_typing_style_from_caret(&mut self) {
        let pos = self.selection.focus;
        let Some(para) = self.paragraph_at(pos) else {
            self.typing_style = TextStyle::default();
            return;
        };
        if para.runs.is_empty() {
            return;
        }
        // Prefer the style of the character before the caret.
        let look = if pos.offset == 0 {
            0
        } else {
            pos.offset - 1
        };
        let mut remaining = look;
        for run in &para.runs {
            let len = run.char_len();
            if remaining < len || run == para.runs.last().unwrap() {
                self.typing_style = run.style.clone();
                return;
            }
            remaining -= len;
        }
    }

    /// Insert text at the caret (replacing any selection).
    pub fn insert_text(&mut self, text: &str) -> Result<(), EditError> {
        if text.is_empty() {
            return Ok(());
        }
        self.push_undo();
        if !self.selection.is_collapsed() {
            self.delete_selection_internal()?;
        }
        let style = self.typing_style.clone();
        let pos = self.selection.focus;
        self.insert_plain_at(pos, text, &style)?;
        self.dirty = true;
        Ok(())
    }

    fn insert_plain_at(
        &mut self,
        pos: DocPosition,
        text: &str,
        style: &TextStyle,
    ) -> Result<(), EditError> {
        // In cells / margins, keep a single paragraph: newlines become literal line breaks.
        if self.edit_focus.is_single_paragraph() {
            let mut cur = pos;
            if !text.is_empty() {
                self.insert_run_fragment(cur, text, style)?;
                cur.offset += text.chars().count();
            }
            self.selection = Selection::caret(self.clamp_position(cur));
            return Ok(());
        }

        let parts: Vec<&str> = text.split('\n').collect();
        let mut cur = pos;

        for (i, part) in parts.iter().enumerate() {
            if !part.is_empty() {
                self.insert_run_fragment(cur, part, style)?;
                cur.offset += part.chars().count();
            }
            if i + 1 < parts.len() {
                // Enter on an empty list item exits the list instead of adding another item.
                let exit_list = self
                    .paragraph_at(cur)
                    .map(|p| p.is_empty() && p.style.list.is_some())
                    .unwrap_or(false);
                if exit_list {
                    if let Some(p) = self.paragraph_at_mut(cur) {
                        p.style.list = None;
                    }
                    continue;
                }
                self.split_paragraph_at(cur)?;
                cur = DocPosition::new(cur.paragraph + 1, 0);
            }
        }
        self.selection = Selection::caret(cur);
        Ok(())
    }

    fn insert_run_fragment(
        &mut self,
        pos: DocPosition,
        text: &str,
        style: &TextStyle,
    ) -> Result<(), EditError> {
        let para = self
            .paragraph_at_mut(pos)
            .ok_or(EditError::InvalidParagraph)?;

        if para.runs.is_empty() {
            para.runs.push(Run::new(text, style.clone()));
            return Ok(());
        }

        let mut remaining = pos.offset;
        let mut run_idx = 0;
        while run_idx < para.runs.len() {
            let run_len = para.runs[run_idx].char_len();
            if remaining <= run_len {
                break;
            }
            remaining -= run_len;
            run_idx += 1;
        }

        if run_idx >= para.runs.len() {
            // Append at end.
            let last = para.runs.last_mut().unwrap();
            if &last.style == style {
                last.text.push_str(text);
            } else {
                para.runs.push(Run::new(text, style.clone()));
            }
            para.normalize();
            return Ok(());
        }

        let run = &mut para.runs[run_idx];
        if &run.style == style {
            let byte_idx = char_to_byte_index(&run.text, remaining);
            run.text.insert_str(byte_idx, text);
        } else {
            let byte_idx = char_to_byte_index(&run.text, remaining);
            let after = run.text[byte_idx..].to_string();
            let after_style = run.style.clone();
            let after_link = run.link.clone();
            run.text.truncate(byte_idx);
            let before_empty = run.text.is_empty();
            let mut inserts = Vec::new();
            inserts.push(Run::new(text, style.clone()));
            if !after.is_empty() {
                inserts.push(Run::new(after, after_style).with_link(after_link));
            }
            if before_empty {
                para.runs.splice(run_idx..=run_idx, inserts);
            } else {
                para.runs.splice(run_idx + 1..run_idx + 1, inserts);
            }
        }
        para.normalize();
        Ok(())
    }

    fn split_paragraph_at(&mut self, pos: DocPosition) -> Result<(), EditError> {
        if !self.edit_focus.is_body() {
            return Err(EditError::InvalidParagraph);
        }
        let (sec, block_idx) = self
            .block_loc_of_paragraph(pos.paragraph)
            .ok_or(EditError::InvalidParagraph)?;
        let para = self.document.sections[sec].blocks[block_idx]
            .as_paragraph_mut()
            .ok_or(EditError::InvalidParagraph)?;
        let (left_runs, right_runs) = split_runs_at(&para.runs, pos.offset);
        let right_style = para.style.clone();
        para.runs = left_runs;
        para.normalize();
        let mut right = Paragraph {
            runs: right_runs,
            style: right_style,
        };
        right.normalize();
        self.document.sections[sec]
            .blocks
            .insert(block_idx + 1, Block::Paragraph(right));
        Ok(())
    }

    /// Insert a table after the current paragraph block and focus the first cell.
    pub fn insert_table(&mut self, rows: usize, cols: usize) -> Result<(), EditError> {
        self.insert_block_after_caret(Block::Table(Table::new(rows, cols)))?;
        let caret_para = self.selection.focus.paragraph;
        if let Some((sec, para_block)) = self.block_loc_of_paragraph(caret_para) {
            if para_block > 0 {
                if let Block::Table(_) = &self.document.sections[sec].blocks[para_block - 1] {
                    let _ = self.focus_cell(CellAddress::in_section(sec, para_block - 1, 0, 0), 0);
                }
            }
        }
        Ok(())
    }

    /// Insert an image (from filesystem path) after the current paragraph block.
    pub fn insert_image_path(
        &mut self,
        path: String,
        width_pt: f32,
        height_pt: f32,
    ) -> Result<(), EditError> {
        self.insert_image(Image::from_path(path, width_pt, height_pt))
    }

    /// Insert a prepared [`Image`] block after the caret paragraph.
    pub fn insert_image(&mut self, image: Image) -> Result<(), EditError> {
        self.focus_body();
        self.insert_block_after_caret(Block::Image(image))
    }

    /// Insert an explicit page break after the caret paragraph.
    pub fn insert_page_break(&mut self) -> Result<(), EditError> {
        self.focus_body();
        self.insert_block_after_caret(Block::PageBreak)
    }

    /// Insert a section break after the caret paragraph.
    ///
    /// Subsequent blocks move into a new [`Section`] that clones the current
    /// page style and header/footer (override with `page_style` when set).
    pub fn insert_section_break(&mut self) -> Result<(), EditError> {
        self.insert_section_break_with_style(None)
    }

    /// Like [`Self::insert_section_break`], optionally setting the new section's page style.
    pub fn insert_section_break_with_style(
        &mut self,
        page_style: Option<PageStyle>,
    ) -> Result<(), EditError> {
        if page_style.as_ref().is_some_and(|style| !style.is_valid()) {
            return Err(EditError::InvalidPageStyle);
        }
        self.focus_body();
        let para_idx = self.selection.focus.paragraph;
        let (sec, block_idx) = self
            .block_loc_of_paragraph(para_idx)
            .ok_or(EditError::InvalidParagraph)?;
        self.push_undo();
        let split_at = block_idx + 1;
        let section = &mut self.document.sections[sec];
        let mut right_blocks = if split_at < section.blocks.len() {
            section.blocks.split_off(split_at)
        } else {
            Vec::new()
        };
        if !right_blocks.iter().any(Block::is_paragraph) {
            right_blocks.push(Block::Paragraph(Paragraph::empty()));
        }
        section.ensure_paragraph();
        let new_style = page_style.unwrap_or_else(|| section.page_style.clone());
        let header = section.header.clone();
        let footer = section.footer.clone();
        let new_section = Section {
            blocks: right_blocks,
            page_style: new_style,
            header,
            footer,
        };
        self.document.sections.insert(sec + 1, new_section);
        let caret_para = self.document.paragraph_offset_of_section(sec + 1);
        self.selection = Selection::caret(DocPosition::new(caret_para, 0));
        self.edit_focus = EditFocus::Body;
        self.dirty = true;
        self.sync_typing_style_from_caret();
        Ok(())
    }

    /// Apply page geometry and local margin content as one undoable operation.
    /// `None` inherits the first section; an empty paragraph explicitly clears it.
    pub fn set_section_settings(
        &mut self,
        section: usize,
        page_style: PageStyle,
        header: Option<Paragraph>,
        footer: Option<Paragraph>,
    ) -> Result<(), EditError> {
        let current = self.document.sections.get(section)
            .ok_or(EditError::InvalidSection)?;
        if !page_style.is_valid() {
            return Err(EditError::InvalidPageStyle);
        }
        if current.page_style == page_style && current.header == header && current.footer == footer {
            return Ok(());
        }
        self.push_undo();
        let current = &mut self.document.sections[section];
        current.page_style = page_style;
        current.header = header;
        current.footer = footer;
        self.selection = self.clamp_selection(self.selection);
        self.sync_typing_style_from_caret();
        self.dirty = true;
        Ok(())
    }

    /// Set (or clear) the document header from plain text.
    pub fn set_header_text(&mut self, text: &str) -> Result<(), EditError> {
        self.push_undo();
        self.document.main_section_mut().header = if text.is_empty() {
            None
        } else {
            Some(Paragraph::from_text(text))
        };
        self.dirty = true;
        Ok(())
    }

    /// Set (or clear) the document footer from plain text.
    pub fn set_footer_text(&mut self, text: &str) -> Result<(), EditError> {
        self.push_undo();
        self.document.main_section_mut().footer = if text.is_empty() {
            None
        } else {
            Some(Paragraph::from_text(text))
        };
        self.dirty = true;
        Ok(())
    }

    fn insert_block_after_caret(&mut self, block: Block) -> Result<(), EditError> {
        self.focus_body();
        self.push_undo();
        let para_idx = self.selection.focus.paragraph;
        let (sec, block_idx) = self
            .block_loc_of_paragraph(para_idx)
            .ok_or(EditError::InvalidParagraph)?;
        let insert_at = block_idx + 1;
        self.document.sections[sec].blocks.insert(insert_at, block);

        let after_idx = insert_at + 1;
        let need_paragraph = after_idx >= self.document.sections[sec].blocks.len()
            || !self.document.sections[sec].blocks[after_idx].is_paragraph();
        if need_paragraph {
            self.document.sections[sec]
                .blocks
                .insert(after_idx, Block::Paragraph(Paragraph::empty()));
        }

        let caret_para = self.paragraph_index_of_block(sec, after_idx);
        self.selection = Selection::caret(DocPosition::new(caret_para, 0));
        self.edit_focus = EditFocus::Body;
        self.dirty = true;
        self.sync_typing_style_from_caret();
        Ok(())
    }

    /// Move focus to the next / previous table cell (wraps within the table).
    pub fn move_cell(&mut self, forward: bool) -> Result<(), EditError> {
        let EditFocus::Cell(addr) = self.edit_focus else {
            return Ok(());
        };
        let Block::Table(table) = self
            .document
            .sections
            .get(addr.section)
            .and_then(|s| s.blocks.get(addr.block))
            .ok_or(EditError::InvalidCell)?
        else {
            return Err(EditError::InvalidCell);
        };
        let rows = table.row_count();
        let cols = table.column_count();
        if rows == 0 || cols == 0 {
            return Ok(());
        }
        let mut r = addr.row;
        let mut c = addr.col;
        if forward {
            c += 1;
            if c >= cols {
                c = 0;
                r += 1;
                if r >= rows {
                    r = 0;
                }
            }
        } else if c == 0 {
            c = cols - 1;
            if r == 0 {
                r = rows - 1;
            } else {
                r -= 1;
            }
        } else {
            c -= 1;
        }
        self.focus_cell(CellAddress::in_section(addr.section, addr.block, r, c), 0)
    }

    pub fn delete_backward(&mut self) -> Result<(), EditError> {
        self.push_undo();
        if !self.selection.is_collapsed() {
            self.delete_selection_internal()?;
            self.dirty = true;
            return Ok(());
        }
        let pos = self.selection.focus;
        if pos.offset == 0 {
            if self.edit_focus.is_single_paragraph() || pos.paragraph == 0 {
                self.undo_stack.pop();
                return Ok(());
            }
            let prev_len = self
                .document
                .paragraph(pos.paragraph - 1)
                .ok_or(EditError::InvalidParagraph)?
                .char_len();
            self.merge_with_previous(pos.paragraph)?;
            self.selection = Selection::caret(DocPosition::new(pos.paragraph - 1, prev_len));
        } else {
            let del_start = DocPosition::new(pos.paragraph, pos.offset - 1);
            self.delete_range(del_start, pos)?;
            self.selection = Selection::caret(del_start);
        }
        self.dirty = true;
        self.sync_typing_style_from_caret();
        Ok(())
    }

    pub fn delete_forward(&mut self) -> Result<(), EditError> {
        self.push_undo();
        if !self.selection.is_collapsed() {
            self.delete_selection_internal()?;
            self.dirty = true;
            return Ok(());
        }
        let pos = self.selection.focus;
        let para_len = self
            .paragraph_at(pos)
            .ok_or(EditError::InvalidParagraph)?
            .char_len();
        if pos.offset < para_len {
            let del_end = DocPosition::new(pos.paragraph, pos.offset + 1);
            self.delete_range(pos, del_end)?;
        } else if self.edit_focus.is_body() && pos.paragraph + 1 < self.document.paragraph_count() {
            self.merge_with_previous(pos.paragraph + 1)?;
        } else {
            self.undo_stack.pop();
            return Ok(());
        }
        self.selection = Selection::caret(pos);
        self.dirty = true;
        Ok(())
    }

    fn delete_selection_internal(&mut self) -> Result<(), EditError> {
        let start = self.selection.start();
        let end = self.selection.end();
        self.delete_range(start, end)?;
        self.selection = Selection::caret(start);
        Ok(())
    }

    fn merge_with_previous(&mut self, paragraph: usize) -> Result<(), EditError> {
        if !self.edit_focus.is_body() || paragraph == 0 {
            return Ok(());
        }
        let (sec, block_idx) = self
            .block_loc_of_paragraph(paragraph)
            .ok_or(EditError::InvalidParagraph)?;
        let (prev_sec, _) = self
            .block_loc_of_paragraph(paragraph - 1)
            .ok_or(EditError::InvalidParagraph)?;
        if sec != prev_sec {
            // Section boundary: pull the first paragraph of `sec` into the previous section.
            let right = match self.document.sections[sec].blocks.remove(block_idx) {
                Block::Paragraph(p) => p,
                other => {
                    self.document.sections[sec].blocks.insert(block_idx, other);
                    return Err(EditError::InvalidParagraph);
                }
            };
            if !self.document.sections[sec]
                .blocks
                .iter()
                .any(Block::is_paragraph)
            {
                let mut rest = std::mem::take(&mut self.document.sections[sec].blocks);
                self.document.sections[prev_sec].blocks.append(&mut rest);
                self.document.sections.remove(sec);
            }
            let left = self
                .document
                .paragraph_mut(paragraph - 1)
                .ok_or(EditError::InvalidParagraph)?;
            left.runs.extend(right.runs);
            left.normalize();
            return Ok(());
        }
        let right = match self.document.sections[sec].blocks.remove(block_idx) {
            Block::Paragraph(p) => p,
            _ => return Err(EditError::InvalidParagraph),
        };
        let left = self
            .document
            .paragraph_mut(paragraph - 1)
            .ok_or(EditError::InvalidParagraph)?;
        left.runs.extend(right.runs);
        left.normalize();
        Ok(())
    }

    fn delete_range(&mut self, start: DocPosition, end: DocPosition) -> Result<(), EditError> {
        if start == end {
            return Ok(());
        }
        if self.edit_focus.is_single_paragraph() || start.paragraph == end.paragraph {
            let para = self
                .paragraph_at_mut(start)
                .ok_or(EditError::InvalidParagraph)?;
            para.runs = delete_from_runs(&para.runs, start.offset, end.offset);
            para.normalize();
            return Ok(());
        }

        {
            let para = self
                .document
                .paragraph_mut(start.paragraph)
                .ok_or(EditError::InvalidParagraph)?;
            let len = para.char_len();
            para.runs = delete_from_runs(&para.runs, start.offset, len);
            para.normalize();
        }
        {
            let para = self
                .document
                .paragraph_mut(end.paragraph)
                .ok_or(EditError::InvalidParagraph)?;
            para.runs = delete_from_runs(&para.runs, 0, end.offset);
            para.normalize();
        }

        let (end_sec, end_block) = self
            .block_loc_of_paragraph(end.paragraph)
            .ok_or(EditError::InvalidParagraph)?;
        let mut last = match self.document.sections[end_sec].blocks.remove(end_block) {
            Block::Paragraph(p) => p,
            _ => return Err(EditError::InvalidParagraph),
        };
        for para_idx in (start.paragraph + 1..end.paragraph).rev() {
            let (sec, block_idx) = self
                .block_loc_of_paragraph(para_idx)
                .ok_or(EditError::InvalidParagraph)?;
            self.document.sections[sec].blocks.remove(block_idx);
        }
        let first = self
            .document
            .paragraph_mut(start.paragraph)
            .ok_or(EditError::InvalidParagraph)?;
        first.runs.append(&mut last.runs);
        first.normalize();
        Ok(())
    }

    pub fn move_left(&mut self, extend: bool) {
        let pos = self.selection.focus;
        let new_pos = if pos.offset > 0 {
            DocPosition::new(pos.paragraph, pos.offset - 1)
        } else if self.edit_focus.is_body() && pos.paragraph > 0 {
            let prev_len = self
                .document
                .paragraph(pos.paragraph - 1)
                .map(|p| p.char_len())
                .unwrap_or(0);
            DocPosition::new(pos.paragraph - 1, prev_len)
        } else {
            pos
        };
        self.apply_move(new_pos, extend);
    }

    pub fn move_right(&mut self, extend: bool) {
        let pos = self.selection.focus;
        let para_len = self.paragraph_at(pos).map(|p| p.char_len()).unwrap_or(0);
        let new_pos = if pos.offset < para_len {
            DocPosition::new(pos.paragraph, pos.offset + 1)
        } else if self.edit_focus.is_body() && pos.paragraph + 1 < self.document.paragraph_count() {
            DocPosition::new(pos.paragraph + 1, 0)
        } else {
            pos
        };
        self.apply_move(new_pos, extend);
    }

    pub fn move_up(&mut self, extend: bool) {
        if matches!(self.edit_focus, EditFocus::Cell(_)) {
            let _ = self.move_cell(false);
            return;
        }
        if self.edit_focus.is_margin() {
            self.move_home(extend);
            return;
        }
        let pos = self.selection.focus;
        let new_pos = if pos.paragraph > 0 {
            let prev_len = self
                .document
                .paragraph(pos.paragraph - 1)
                .map(|p| p.char_len())
                .unwrap_or(0);
            DocPosition::new(pos.paragraph - 1, pos.offset.min(prev_len))
        } else {
            DocPosition::new(0, 0)
        };
        self.apply_move(new_pos, extend);
    }

    pub fn move_down(&mut self, extend: bool) {
        if matches!(self.edit_focus, EditFocus::Cell(_)) {
            let _ = self.move_cell(true);
            return;
        }
        if self.edit_focus.is_margin() {
            self.move_end(extend);
            return;
        }
        let pos = self.selection.focus;
        let count = self.document.paragraph_count();
        let new_pos = if pos.paragraph + 1 < count {
            let next_len = self
                .document
                .paragraph(pos.paragraph + 1)
                .map(|p| p.char_len())
                .unwrap_or(0);
            DocPosition::new(pos.paragraph + 1, pos.offset.min(next_len))
        } else {
            let len = self
                .document
                .paragraph(pos.paragraph)
                .map(|p| p.char_len())
                .unwrap_or(0);
            DocPosition::new(pos.paragraph, len)
        };
        self.apply_move(new_pos, extend);
    }

    pub fn move_home(&mut self, extend: bool) {
        let pos = self.selection.focus;
        self.apply_move(DocPosition::new(pos.paragraph, 0), extend);
    }

    pub fn move_end(&mut self, extend: bool) {
        let pos = self.selection.focus;
        let len = self.paragraph_at(pos).map(|p| p.char_len()).unwrap_or(0);
        self.apply_move(DocPosition::new(pos.paragraph, len), extend);
    }

    pub fn select_all(&mut self) {
        if self.edit_focus.is_single_paragraph() {
            let end_off = self
                .paragraph_at(DocPosition::zero())
                .map(|p| p.char_len())
                .unwrap_or(0);
            self.selection = Selection {
                anchor: DocPosition::zero(),
                focus: DocPosition::new(0, end_off),
            };
            return;
        }
        let last = self.document.paragraph_count().saturating_sub(1);
        let end_off = self
            .document
            .paragraph(last)
            .map(|p| p.char_len())
            .unwrap_or(0);
        self.selection = Selection {
            anchor: DocPosition::zero(),
            focus: DocPosition::new(last, end_off),
        };
    }

    fn apply_move(&mut self, new_pos: DocPosition, extend: bool) {
        let new_pos = self.clamp_position(new_pos);
        if extend {
            self.selection = self.selection.with_focus(new_pos);
        } else {
            self.selection = Selection::caret(new_pos);
        }
        self.sync_typing_style_from_caret();
    }

    /// Selected plain text (or empty if caret).
    pub fn selected_text(&self) -> String {
        if self.selection.is_collapsed() {
            return String::new();
        }
        let start = self.selection.start();
        let end = self.selection.end();
        if self.edit_focus.is_single_paragraph() {
            let Some(para) = self.paragraph_at(DocPosition::zero()) else {
                return String::new();
            };
            return extract_text(std::slice::from_ref(para), start, end);
        }
        let paragraphs = self.document.paragraphs_cloned();
        extract_text(&paragraphs, start, end)
    }

    pub fn copy_text(&self) -> String {
        self.selected_text()
    }

    pub fn cut(&mut self) -> Result<String, EditError> {
        let text = self.selected_text();
        if text.is_empty() {
            return Ok(text);
        }
        self.push_undo();
        self.delete_selection_internal()?;
        self.dirty = true;
        Ok(text)
    }

    pub fn paste_text(&mut self, text: &str) -> Result<(), EditError> {
        self.insert_text(text)
    }

    pub fn set_alignment(&mut self, alignment: Alignment) -> Result<(), EditError> {
        self.push_undo();
        if self.edit_focus.is_single_paragraph() {
            if let Some(p) = self.paragraph_at_mut(DocPosition::zero()) {
                p.style.alignment = alignment;
            }
            self.dirty = true;
            return Ok(());
        }
        let start = self.selection.start().paragraph;
        let end = self.selection.end().paragraph;
        for i in start..=end {
            if let Some(p) = self.document.paragraph_mut(i) {
                p.style.alignment = alignment;
            }
        }
        self.dirty = true;
        Ok(())
    }

    /// Assign a built-in named paragraph style (Normal / Heading 1–3).
    ///
    /// Bakes paragraph metrics and run character styles from the named defaults
    /// while preserving alignment and list membership.
    pub fn set_named_paragraph_style(
        &mut self,
        named: NamedParagraphStyle,
    ) -> Result<(), EditError> {
        self.push_undo();
        let text = named.builtin_text();
        if self.edit_focus.is_single_paragraph() {
            if let Some(p) = self.paragraph_at_mut(DocPosition::zero()) {
                apply_named_to_paragraph(p, named, &text);
            }
            self.typing_style = text;
            self.dirty = true;
            return Ok(());
        }
        let start = self.selection.start().paragraph;
        let end = self.selection.end().paragraph;
        for i in start..=end {
            if let Some(p) = self.document.paragraph_mut(i) {
                apply_named_to_paragraph(p, named, &text);
            }
        }
        self.typing_style = text;
        self.dirty = true;
        Ok(())
    }

    /// Named style of the caret paragraph (Body / Header / Footer / Cell).
    pub fn current_named_paragraph_style(&self) -> NamedParagraphStyle {
        self.paragraph_at(self.selection.focus)
            .map(|p| p.style.named)
            .unwrap_or_default()
    }

    /// Toggle bullet or numbered list on the selected paragraphs.
    pub fn toggle_list(&mut self, kind: ListKind) -> Result<(), EditError> {
        self.push_undo();
        if self.edit_focus.is_single_paragraph() {
            if let Some(p) = self.paragraph_at_mut(DocPosition::zero()) {
                let level = p.style.list.map(|l| l.level).unwrap_or(0);
                let target = ListStyle { kind, level };
                if p.style.list.map(|l| l.kind) == Some(kind) {
                    p.style.list = None;
                } else {
                    p.style.list = Some(target);
                }
            }
            self.dirty = true;
            return Ok(());
        }
        let start = self.selection.start().paragraph;
        let end = self.selection.end().paragraph;
        let all_this_kind = (start..=end).all(|i| {
            self.document
                .paragraph(i)
                .and_then(|p| p.style.list)
                .map(|l| l.kind)
                == Some(kind)
        });
        for i in start..=end {
            if let Some(p) = self.document.paragraph_mut(i) {
                if all_this_kind {
                    p.style.list = None;
                } else {
                    let level = p.style.list.map(|l| l.level).unwrap_or(0);
                    p.style.list = Some(ListStyle { kind, level });
                }
            }
        }
        self.dirty = true;
        Ok(())
    }

    pub fn toggle_bullet_list(&mut self) -> Result<(), EditError> {
        self.toggle_list(ListKind::Bullet)
    }

    pub fn toggle_numbered_list(&mut self) -> Result<(), EditError> {
        self.toggle_list(ListKind::Numbered)
    }

    /// Increase list nesting for the selected paragraphs (no-op if not in a list).
    pub fn indent_list(&mut self) -> Result<(), EditError> {
        self.change_list_level(1)
    }

    /// Decrease list nesting for the selected paragraphs.
    pub fn outdent_list(&mut self) -> Result<(), EditError> {
        self.change_list_level(-1)
    }

    fn change_list_level(&mut self, delta: i8) -> Result<(), EditError> {
        if delta == 0 {
            return Ok(());
        }
        self.push_undo();
        let mut changed = false;
        if self.edit_focus.is_single_paragraph() {
            if let Some(p) = self.paragraph_at_mut(DocPosition::zero()) {
                changed |= adjust_list_level(&mut p.style.list, delta);
            }
        } else {
            let start = self.selection.start().paragraph;
            let end = self.selection.end().paragraph;
            for i in start..=end {
                if let Some(p) = self.document.paragraph_mut(i) {
                    changed |= adjust_list_level(&mut p.style.list, delta);
                }
            }
        }
        if changed {
            self.dirty = true;
        } else {
            self.undo_stack.pop();
        }
        Ok(())
    }

    /// Hyperlink URL covering the character at `pos` (or the previous char at EOL).
    pub fn link_at(&self, pos: DocPosition) -> Option<&str> {
        self.document.link_at(pos)
    }

    /// Apply (or clear) a hyperlink on the current selection.
    pub fn set_hyperlink(&mut self, url: Option<String>) -> Result<(), EditError> {
        if self.selection.is_collapsed() {
            return Ok(());
        }
        self.push_undo();
        self.apply_link_to_selection(url);
        self.dirty = true;
        Ok(())
    }

    fn apply_link_to_selection(&mut self, url: Option<String>) {
        let start = self.selection.start();
        let end = self.selection.end();
        if self.edit_focus.is_single_paragraph() || start.paragraph == end.paragraph {
            let para = self.paragraph_at_mut(start).unwrap();
            para.runs = map_runs_link(&para.runs, start.offset, end.offset, url);
            para.normalize();
            return;
        }
        {
            let para = self.document.paragraph_mut(start.paragraph).unwrap();
            let len = para.char_len();
            para.runs = map_runs_link(&para.runs, start.offset, len, url.clone());
            para.normalize();
        }
        for i in start.paragraph + 1..end.paragraph {
            if let Some(para) = self.document.paragraph_mut(i) {
                for run in &mut para.runs {
                    run.link = url.clone();
                }
            }
        }
        {
            let para = self.document.paragraph_mut(end.paragraph).unwrap();
            para.runs = map_runs_link(&para.runs, 0, end.offset, url);
            para.normalize();
        }
    }

    pub fn toggle_bold(&mut self) {
        self.toggle_style_flag(|s| &mut s.bold);
    }

    pub fn toggle_italic(&mut self) {
        self.toggle_style_flag(|s| &mut s.italic);
    }

    pub fn toggle_underline(&mut self) {
        self.toggle_style_flag(|s| &mut s.underline);
    }

    pub fn set_font_size(&mut self, size: f32) {
        let size = size.clamp(6.0, 96.0);
        if self.selection.is_collapsed() {
            self.typing_style.font_size = size;
            return;
        }
        self.push_undo();
        self.apply_style_to_selection(|s| s.font_size = size);
        self.dirty = true;
    }

    fn toggle_style_flag(&mut self, flag: impl Fn(&mut TextStyle) -> &mut bool) {
        if self.selection.is_collapsed() {
            let v = !*flag(&mut self.typing_style);
            *flag(&mut self.typing_style) = v;
            return;
        }
        // If any selected char lacks the flag, turn on; else turn off.
        let enable = !self.selection_all_flag(&flag);
        self.push_undo();
        self.apply_style_to_selection(|s| *flag(s) = enable);
        self.typing_style = self.typing_style.clone();
        *flag(&mut self.typing_style) = enable;
        self.dirty = true;
    }

    fn selection_all_flag(&self, flag: &impl Fn(&mut TextStyle) -> &mut bool) -> bool {
        let start = self.selection.start();
        let end = self.selection.end();
        let mut all = true;
        let paragraphs = if self.edit_focus.is_single_paragraph() {
            self.paragraph_at(DocPosition::zero())
                .cloned()
                .into_iter()
                .collect::<Vec<_>>()
        } else {
            self.document.paragraphs_cloned()
        };
        for_each_style_in_range(&paragraphs, start, end, |style| {
            let mut s = style.clone();
            if !*flag(&mut s) {
                all = false;
            }
        });
        all
    }

    fn apply_style_to_selection(&mut self, mut apply: impl FnMut(&mut TextStyle)) {
        let start = self.selection.start();
        let end = self.selection.end();
        if self.edit_focus.is_single_paragraph() || start.paragraph == end.paragraph {
            let para = self.paragraph_at_mut(start).unwrap();
            para.runs = map_runs_range(&para.runs, start.offset, end.offset, &mut apply);
            para.normalize();
            return;
        }
        {
            let para = self.document.paragraph_mut(start.paragraph).unwrap();
            let len = para.char_len();
            para.runs = map_runs_range(&para.runs, start.offset, len, &mut apply);
            para.normalize();
        }
        for i in start.paragraph + 1..end.paragraph {
            if let Some(para) = self.document.paragraph_mut(i) {
                for run in &mut para.runs {
                    apply(&mut run.style);
                }
            }
        }
        {
            let para = self.document.paragraph_mut(end.paragraph).unwrap();
            para.runs = map_runs_range(&para.runs, 0, end.offset, &mut apply);
            para.normalize();
        }
    }

    /// Replace the entire document (e.g. after Open). Clears undo history.
    pub fn replace_document(&mut self, document: Document) {
        self.document = document;
        self.document.main_section_mut().ensure_paragraph();
        self.selection = Selection::default();
        self.edit_focus = EditFocus::Body;
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.dirty = false;
        self.typing_style = TextStyle::default();
    }

    /// Restore an unsaved snapshot, without restoring its runtime undo history.
    pub fn restore_unsaved_document(&mut self, document: Document) {
        self.replace_document(document);
        self.dirty = true;
    }

    pub fn new_document(&mut self) {
        self.replace_document(Document::new());
    }
}

fn apply_named_to_paragraph(
    para: &mut Paragraph,
    named: NamedParagraphStyle,
    text: &TextStyle,
) {
    apply_named_paragraph_defaults(&mut para.style, named);
    if para.runs.is_empty() {
        return;
    }
    for run in &mut para.runs {
        let link = run.link.clone();
        run.style = text.clone();
        run.link = link;
    }
}

fn adjust_list_level(list: &mut Option<ListStyle>, delta: i8) -> bool {
    let Some(current) = list.as_mut() else {
        return false;
    };
    let next = (current.level as i16 + delta as i16).clamp(0, ListStyle::MAX_LEVEL as i16) as u8;
    if next == current.level {
        return false;
    }
    current.level = next;
    true
}

fn char_to_byte_index(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}

fn split_runs_at(runs: &[Run], offset: usize) -> (Vec<Run>, Vec<Run>) {
    if offset == 0 {
        return (Vec::new(), runs.to_vec());
    }
    let mut remaining = offset;
    let mut left = Vec::new();
    let mut right = Vec::new();
    let mut splitting = true;
    for run in runs {
        let len = run.char_len();
        if !splitting {
            right.push(run.clone());
            continue;
        }
        if remaining >= len {
            left.push(run.clone());
            remaining -= len;
            if remaining == 0 {
                splitting = false;
            }
        } else {
            let byte_idx = char_to_byte_index(&run.text, remaining);
            let before = run.text[..byte_idx].to_string();
            let after = run.text[byte_idx..].to_string();
            if !before.is_empty() {
                left.push(Run::new(before, run.style.clone()).with_link(run.link.clone()));
            }
            if !after.is_empty() {
                right.push(Run::new(after, run.style.clone()).with_link(run.link.clone()));
            }
            splitting = false;
        }
    }
    (left, right)
}

fn delete_from_runs(runs: &[Run], start: usize, end: usize) -> Vec<Run> {
    let (left, rest) = split_runs_at(runs, start);
    let (_mid, right) = split_runs_at(&rest, end - start);
    let mut out = left;
    out.extend(right);
    out
}


fn map_runs_link(runs: &[Run], start: usize, end: usize, url: Option<String>) -> Vec<Run> {
    if start >= end {
        return runs.to_vec();
    }
    let mut out = Vec::new();
    let mut cursor = 0usize;
    for run in runs {
        let len = run.char_len();
        let run_start = cursor;
        let run_end = cursor + len;
        cursor = run_end;
        if run_end <= start || run_start >= end {
            out.push(run.clone());
            continue;
        }
        let local_start = start.saturating_sub(run_start);
        let local_end = end.min(run_end) - run_start;
        let before = run.text.chars().take(local_start).collect::<String>();
        let mid = run.text.chars().skip(local_start).take(local_end - local_start).collect::<String>();
        let after = run.text.chars().skip(local_end).collect::<String>();
        if !before.is_empty() {
            out.push(Run::new(before, run.style.clone()).with_link(run.link.clone()));
        }
        if !mid.is_empty() {
            out.push(Run::new(mid, run.style.clone()).with_link(url.clone()));
        }
        if !after.is_empty() {
            out.push(Run::new(after, run.style.clone()).with_link(run.link.clone()));
        }
    }
    out
}

fn map_runs_range(
    runs: &[Run],
    start: usize,
    end: usize,
    apply: &mut impl FnMut(&mut TextStyle),
) -> Vec<Run> {
    let (left, rest) = split_runs_at(runs, start);
    let (mid, right) = split_runs_at(&rest, end - start);
    let mut out = left;
    for mut run in mid {
        apply(&mut run.style);
        out.push(run);
    }
    out.extend(right);
    out
}

fn extract_text(paragraphs: &[Paragraph], start: DocPosition, end: DocPosition) -> String {
    if start.paragraph == end.paragraph {
        let text = paragraphs[start.paragraph].plain_text();
        return text
            .chars()
            .skip(start.offset)
            .take(end.offset - start.offset)
            .collect();
    }
    let mut out = String::new();
    let first = paragraphs[start.paragraph].plain_text();
    out.extend(first.chars().skip(start.offset));
    out.push('\n');
    for para in &paragraphs[start.paragraph + 1..end.paragraph] {
        out.push_str(&para.plain_text());
        out.push('\n');
    }
    let last = paragraphs[end.paragraph].plain_text();
    out.extend(last.chars().take(end.offset));
    out
}

fn for_each_style_in_range(
    paragraphs: &[Paragraph],
    start: DocPosition,
    end: DocPosition,
    mut f: impl FnMut(&TextStyle),
) {
    let apply_para = |para: &Paragraph, from: usize, to: usize, f: &mut dyn FnMut(&TextStyle)| {
        let mut idx = 0;
        for run in &para.runs {
            let len = run.char_len();
            let run_start = idx;
            let run_end = idx + len;
            if run_end > from && run_start < to {
                f(&run.style);
            }
            idx = run_end;
        }
    };

    if start.paragraph == end.paragraph {
        apply_para(
            &paragraphs[start.paragraph],
            start.offset,
            end.offset,
            &mut f,
        );
        return;
    }
    apply_para(
        &paragraphs[start.paragraph],
        start.offset,
        paragraphs[start.paragraph].char_len(),
        &mut f,
    );
    for para in &paragraphs[start.paragraph + 1..end.paragraph] {
        for run in &para.runs {
            f(&run.style);
        }
    }
    apply_para(&paragraphs[end.paragraph], 0, end.offset, &mut f);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::Alignment;

    #[test]
    fn editing_inherited_margin_is_local_and_undo_restores_inheritance() {
        let mut document = Document::with_text("Body");
        document.sections[0].header = Some(Paragraph::from_text("Inherited"));
        document.sections.push(document.sections[0].clone());
        document.sections[1].header = None;
        let before = document.clone();
        let mut editor = DocumentEditor::new(document);
        editor.focus_section_header(1, 9).unwrap();
        assert!(!editor.is_dirty() && !editor.can_undo());
        assert_eq!(editor.current_section(), 1);
        editor.insert_text(" local").unwrap();
        assert_eq!(
            editor.document().section_header(1).unwrap().plain_text(),
            "Inherited local"
        );
        assert_eq!(
            editor.document().header().unwrap().plain_text(),
            "Inherited"
        );
        assert!(editor.undo());
        assert_eq!(editor.document(), &before);
        assert_eq!(editor.edit_focus(), EditFocus::Header(1));
        assert!(editor.redo());
        assert_eq!(
            editor.focus_section_footer(100, 0),
            Err(EditError::InvalidSection)
        );
        assert_eq!(editor.edit_focus(), EditFocus::Header(1));
        editor
            .set_section_settings(
                1,
                PageStyle::default(),
                Some(Paragraph::from_text("X")),
                None,
            )
            .unwrap();
        assert_eq!(editor.selection().focus.offset, 1);
    }

    #[test]
    fn section_settings_are_atomic_and_preserve_other_sections() {
        let mut ed = DocumentEditor::default();
        ed.set_header_text("Inherited").unwrap();
        ed.insert_section_break().unwrap();
        let before = ed.document().clone();
        let mut style = PageStyle::default();
        std::mem::swap(&mut style.width, &mut style.height);
        ed.set_section_settings(
            1,
            style.clone(),
            Some(Paragraph::empty()),
            Some(Paragraph::from_text("  Page {page}  ")),
        ).unwrap();
        assert_eq!(ed.document().sections[0], before.sections[0]);
        assert_eq!(ed.document().section_header(1).unwrap().plain_text(), "");
        assert_eq!(ed.document().section_footer(1).unwrap().plain_text(), "  Page {page}  ");
        assert!(ed.undo());
        assert_eq!(ed.document(), &before);
        assert!(ed.redo());
        assert_eq!(ed.document().sections[1].page_style, style);
    }

    #[test]
    fn invalid_or_unchanged_section_settings_preserve_history() {
        let mut ed = DocumentEditor::default();
        ed.insert_text("draft").unwrap();
        ed.undo();
        ed.mark_clean();
        let before = ed.document().clone();
        ed.set_section_settings(0, PageStyle::default(), None, None).unwrap();
        let bad = PageStyle {
            margin_left: f32::NAN,
            ..PageStyle::default()
        };
        assert_eq!(ed.set_section_settings(0, bad.clone(), None, None), Err(EditError::InvalidPageStyle));
        assert_eq!(ed.insert_section_break_with_style(Some(bad)), Err(EditError::InvalidPageStyle));
        assert_eq!(ed.set_section_settings(99, PageStyle::default(), None, None), Err(EditError::InvalidSection));
        assert_eq!(ed.document(), &before);
        assert!(!ed.is_dirty());
        assert!(!ed.can_undo());
        assert!(ed.can_redo());
    }

    #[test]
    fn named_heading_bakes_metrics_and_runs() {
        let mut ed = DocumentEditor::default();
        ed.insert_text("Title").unwrap();
        ed.set_named_paragraph_style(NamedParagraphStyle::Heading1)
            .unwrap();
        let para = ed.document().paragraph(0).unwrap();
        assert_eq!(para.style.named, NamedParagraphStyle::Heading1);
        assert!(para.style.space_after > 6.0);
        assert!(para.runs.iter().all(|r| r.style.bold && r.style.font_size == 20.0));
        assert_eq!(
            ed.current_named_paragraph_style(),
            NamedParagraphStyle::Heading1
        );
        ed.set_named_paragraph_style(NamedParagraphStyle::Normal)
            .unwrap();
        let para = ed.document().paragraph(0).unwrap();
        assert_eq!(para.style.named, NamedParagraphStyle::Normal);
        assert!(!para.runs[0].style.bold);
    }

    #[test]
    fn insert_and_plain_text() {
        let mut ed = DocumentEditor::default();
        ed.insert_text("Hello").unwrap();
        ed.insert_text(" ").unwrap();
        ed.insert_text("世界").unwrap();
        assert_eq!(ed.document().plain_text(), "Hello 世界");
        assert_eq!(ed.document().word_count(), 2);
    }

    #[test]
    fn newline_creates_paragraph() {
        let mut ed = DocumentEditor::default();
        ed.insert_text("a\nb\nc").unwrap();
        assert_eq!(ed.document().paragraph_count(), 3);
        assert_eq!(ed.document().plain_text(), "a\nb\nc");
    }

    #[test]
    fn backspace_and_delete() {
        let mut ed = DocumentEditor::default();
        ed.insert_text("abcd").unwrap();
        ed.delete_backward().unwrap();
        assert_eq!(ed.document().plain_text(), "abc");
        ed.move_home(false);
        ed.delete_forward().unwrap();
        assert_eq!(ed.document().plain_text(), "bc");
    }

    #[test]
    fn undo_redo() {
        let mut ed = DocumentEditor::default();
        ed.insert_text("one").unwrap();
        ed.insert_text(" two").unwrap();
        assert!(ed.undo());
        assert_eq!(ed.document().plain_text(), "one");
        assert!(ed.redo());
        assert_eq!(ed.document().plain_text(), "one two");
    }

    #[test]
    fn cut_copy_paste() {
        let mut ed = DocumentEditor::default();
        ed.insert_text("hello world").unwrap();
        ed.set_selection(Selection {
            anchor: DocPosition::new(0, 0),
            focus: DocPosition::new(0, 5),
        });
        assert_eq!(ed.copy_text(), "hello");
        let cut = ed.cut().unwrap();
        assert_eq!(cut, "hello");
        assert_eq!(ed.document().plain_text(), " world");
        ed.paste_text("hi").unwrap();
        assert_eq!(ed.document().plain_text(), "hi world");
    }

    #[test]
    fn bold_selection() {
        let mut ed = DocumentEditor::default();
        ed.insert_text("xyz").unwrap();
        ed.set_selection(Selection {
            anchor: DocPosition::new(0, 0),
            focus: DocPosition::new(0, 3),
        });
        ed.toggle_bold();
        let run = &ed.document().paragraph(0).unwrap().runs[0];
        assert!(run.style.bold);
        assert_eq!(run.text, "xyz");
    }

    #[test]
    fn alignment() {
        let mut ed = DocumentEditor::default();
        ed.insert_text("centered").unwrap();
        ed.set_alignment(Alignment::Center).unwrap();
        assert_eq!(
            ed.document().paragraph(0).unwrap().style.alignment,
            Alignment::Center
        );
    }

    #[test]
    fn merge_paragraphs_on_backspace() {
        let mut ed = DocumentEditor::default();
        ed.insert_text("a\nb").unwrap();
        ed.set_selection(Selection::caret(DocPosition::new(1, 0)));
        ed.delete_backward().unwrap();
        assert_eq!(ed.document().plain_text(), "ab");
        assert_eq!(ed.document().paragraph_count(), 1);
    }

    #[test]
    fn select_all_and_replace() {
        let mut ed = DocumentEditor::default();
        ed.insert_text("old\ntext").unwrap();
        ed.select_all();
        ed.insert_text("new").unwrap();
        assert_eq!(ed.document().plain_text(), "new");
    }

    #[test]
    fn insert_table_adds_block_and_caret_after() {
        let mut ed = DocumentEditor::default();
        ed.insert_text("before").unwrap();
        ed.insert_table(2, 2).unwrap();

        assert!(matches!(ed.document().blocks()[0], Block::Paragraph(_)));
        assert!(matches!(ed.document().blocks()[1], Block::Table(_)));
        assert!(matches!(ed.document().blocks()[2], Block::Paragraph(_)));
        assert_eq!(
            ed.edit_focus(),
            EditFocus::Cell(CellAddress::new(1, 0, 0))
        );
        assert_eq!(ed.selection().focus.offset, 0);

        ed.insert_text("A1").unwrap();
        ed.move_cell(true).unwrap();
        ed.insert_text("B1").unwrap();
        assert!(ed.document().plain_text().contains("A1"));
        assert!(ed.document().plain_text().contains("B1"));
    }

    #[test]
    fn cell_edit_typing_and_backspace() {
        let mut ed = DocumentEditor::default();
        ed.insert_table(1, 2).unwrap();
        let addr = CellAddress::new(1, 0, 0);
        assert_eq!(ed.edit_focus(), EditFocus::Cell(addr));
        ed.insert_text("hi").unwrap();
        assert_eq!(
            ed.document().cell_paragraph(addr).unwrap().plain_text(),
            "hi"
        );
        ed.delete_backward().unwrap();
        assert_eq!(
            ed.document().cell_paragraph(addr).unwrap().plain_text(),
            "h"
        );
        // Backspace at start stays in cell.
        ed.set_selection(Selection::caret(DocPosition::zero()));
        ed.delete_backward().unwrap();
        assert_eq!(ed.edit_focus(), EditFocus::Cell(addr));
    }

    #[test]
    fn insert_page_break_and_header_footer() {
        let mut ed = DocumentEditor::default();
        ed.insert_text("before").unwrap();
        ed.insert_page_break().unwrap();
        ed.insert_text("after").unwrap();
        assert!(matches!(ed.document().blocks()[0], Block::Paragraph(_)));
        assert!(matches!(ed.document().blocks()[1], Block::PageBreak));
        assert!(matches!(ed.document().blocks()[2], Block::Paragraph(_)));
        assert_eq!(ed.document().plain_text(), "before\nafter");

        ed.set_header_text("Title").unwrap();
        ed.set_footer_text("Page {page} of {pages}").unwrap();
        assert_eq!(ed.document().header().unwrap().plain_text(), "Title");
        assert_eq!(
            ed.document().footer().unwrap().plain_text(),
            "Page {page} of {pages}"
        );
        ed.set_header_text("").unwrap();
        assert!(ed.document().header().is_none());
    }

    #[test]
    fn insert_section_break_splits_document() {
        let mut ed = DocumentEditor::default();
        ed.insert_text("Section A").unwrap();
        ed.insert_section_break().unwrap();
        ed.insert_text("Section B").unwrap();
        assert_eq!(ed.document().section_count(), 2);
        assert_eq!(ed.document().paragraph_count(), 2);
        assert_eq!(ed.document().paragraph(0).unwrap().plain_text(), "Section A");
        assert_eq!(ed.document().paragraph(1).unwrap().plain_text(), "Section B");
        assert!(ed.document().plain_text().contains("Section A"));
        assert!(ed.document().plain_text().contains("Section B"));
    }

    #[test]
    fn insert_image_path_adds_block_and_alt_in_plain_text() {
        let mut ed = DocumentEditor::default();
        ed.insert_text("cap").unwrap();
        ed.insert_image_path("/tmp/photos/logo.png".into(), 120.0, 80.0)
            .unwrap();

        assert!(matches!(ed.document().blocks()[0], Block::Paragraph(_)));
        assert!(matches!(ed.document().blocks()[1], Block::Image(_)));
        assert!(matches!(ed.document().blocks()[2], Block::Paragraph(_)));
        assert_eq!(ed.selection().focus.paragraph, 1);

        let plain = ed.document().plain_text();
        assert!(plain.contains("[logo.png]"));
        if let Block::Image(img) = &ed.document().blocks()[1] {
            assert_eq!(img.width_pt, 120.0);
            assert_eq!(img.height_pt, 80.0);
        }
    }

    #[test]
    fn header_footer_in_place_edit() {
        let mut ed = DocumentEditor::default();
        ed.focus_header(0);
        assert_eq!(ed.edit_focus(), EditFocus::Header(0));
        ed.insert_text("Title").unwrap();
        assert_eq!(ed.document().header().unwrap().plain_text(), "Title");
        ed.focus_footer(0);
        ed.insert_text("Page {page}").unwrap();
        assert_eq!(
            ed.document().footer().unwrap().plain_text(),
            "Page {page}"
        );
        ed.select_all();
        assert_eq!(ed.selected_text(), "Page {page}");
        ed.focus_body();
        assert_eq!(ed.edit_focus(), EditFocus::Body);
    }

    #[test]
    fn nested_list_indent_outdent() {
        let mut ed = DocumentEditor::default();
        ed.insert_text("A\nB\nC").unwrap();
        ed.select_all();
        ed.toggle_bullet_list().unwrap();
        ed.set_selection(Selection::caret(DocPosition::new(1, 0)));
        ed.indent_list().unwrap();
        assert_eq!(
            ed.document().paragraph(1).unwrap().style.list.map(|l| l.level),
            Some(1)
        );
        ed.outdent_list().unwrap();
        assert_eq!(
            ed.document().paragraph(1).unwrap().style.list.map(|l| l.level),
            Some(0)
        );
        // Outdent at 0 is a no-op (still a list).
        ed.outdent_list().unwrap();
        assert_eq!(
            ed.document().paragraph(1).unwrap().style.list.map(|l| l.level),
            Some(0)
        );
    }

    #[test]
    fn toggle_bullet_list_and_enter_exits() {
        let mut ed = DocumentEditor::default();
        ed.insert_text("one").unwrap();
        ed.toggle_bullet_list().unwrap();
        assert_eq!(
            ed.document().paragraph(0).unwrap().style.list,
            Some(ListStyle::bullet())
        );
        ed.set_selection(Selection::caret(DocPosition::new(0, 3)));
        ed.insert_text("\n").unwrap();
        assert_eq!(
            ed.document().paragraph(1).unwrap().style.list,
            Some(ListStyle::bullet())
        );
        // empty second item + enter exits
        ed.insert_text("\n").unwrap();
        assert!(ed.document().paragraph(1).unwrap().style.list.is_none());
        assert_eq!(ed.document().paragraph_count(), 2);
    }

    #[test]
    fn hyperlink_on_selection() {
        let mut ed = DocumentEditor::default();
        ed.insert_text("hello").unwrap();
        ed.set_selection(Selection {
            anchor: DocPosition::new(0, 0),
            focus: DocPosition::new(0, 5),
        });
        ed.set_hyperlink(Some("https://example.com".into())).unwrap();
        let run = &ed.document().paragraph(0).unwrap().runs[0];
        assert_eq!(run.link.as_deref(), Some("https://example.com"));
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;

    #[test]
    fn restored_document_is_unsaved_and_new_edits_undo_to_recovered_content() {
        let doc = Document::with_text("recovered 日本語");
        let mut editor = DocumentEditor::default();
        editor.insert_text("unrelated history").unwrap();
        editor.restore_unsaved_document(doc.clone());
        assert!(editor.is_dirty());
        assert!(!editor.can_undo());
        assert!(!editor.can_redo());
        editor.insert_text("added ").unwrap();
        editor.undo();
        assert_eq!(editor.document(), &doc);
        assert!(editor.is_dirty());
    }
}
