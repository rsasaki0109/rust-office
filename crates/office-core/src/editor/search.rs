//! Literal, case-sensitive search within body paragraphs across all sections.

use super::{DocumentEditor, EditError};
use crate::{Block, DocPosition, EditFocus, Paragraph, Run, Selection, TextStyle};

impl DocumentEditor {
    /// Non-overlapping matches, in document order. Tables and margins are excluded.
    /// Positions use Unicode scalar offsets, just like normal editor selections.
    pub fn body_matches(&self, query: &str) -> Vec<Selection> {
        if query.is_empty() || query.contains(['\r', '\n']) {
            return Vec::new();
        }
        let length = query.chars().count();
        let mut matches = Vec::new();
        let mut paragraph = 0;
        for section in &self.document.sections {
            for block in &section.blocks {
                if let Block::Paragraph(para) = block {
                    let text = para.plain_text();
                    let mut byte_cursor = 0;
                    let mut char_cursor = 0;
                    for (byte, _) in text.match_indices(query) {
                        char_cursor += text[byte_cursor..byte].chars().count();
                        matches.push(Selection {
                            anchor: DocPosition::new(paragraph, char_cursor),
                            focus: DocPosition::new(paragraph, char_cursor + length),
                        });
                        byte_cursor = byte;
                    }
                    paragraph += 1;
                }
            }
        }
        matches
    }

    /// Select the next/previous match from the body selection, wrapping at the end.
    /// No match leaves focus, selection, document and history untouched.
    pub fn find_body(&mut self, query: &str, forward: bool) -> Option<Selection> {
        let matches = self.body_matches(query);
        let selected = if forward {
            matches
                .iter()
                .find(|found| !self.edit_focus.is_body() || found.start() >= self.selection.end())
                .or(matches.first())
        } else {
            matches
                .iter()
                .rev()
                .find(|found| !self.edit_focus.is_body() || found.end() <= self.selection.start())
                .or(matches.last())
        }
        .copied()?;
        self.edit_focus = EditFocus::Body;
        self.set_selection(selected);
        Some(selected)
    }

    /// Replace only a currently selected, exact search match. Empty text deletes it.
    /// The replacement inherits the first matched character's style and hyperlink.
    pub fn replace_body_match(
        &mut self,
        query: &str,
        replacement: &str,
    ) -> Result<bool, EditError> {
        check_replacement(replacement)?;
        if query == replacement || !self.edit_focus.is_body() {
            return Ok(false);
        }
        let selection = self.selection;
        if !self
            .body_matches(query)
            .iter()
            .any(|found| found.start() == selection.start() && found.end() == selection.end())
        {
            return Ok(false);
        }
        self.push_undo();
        let paragraph = self
            .document
            .paragraph_mut(selection.start().paragraph)
            .ok_or(EditError::InvalidParagraph)?;
        replace_ranges(paragraph, &[selection], replacement);
        self.selection = Selection::caret(DocPosition::new(
            selection.start().paragraph,
            selection.start().offset + replacement.chars().count(),
        ));
        self.dirty = true;
        self.sync_typing_style_from_caret();
        Ok(true)
    }

    /// Replace a snapshot of all body matches as one undoable operation.
    /// Inserted text is never searched again during this operation.
    pub fn replace_all_body(&mut self, query: &str, replacement: &str) -> Result<usize, EditError> {
        check_replacement(replacement)?;
        if query == replacement {
            return Ok(0);
        }
        let matches = self.body_matches(query);
        let Some(first) = matches.first().copied() else {
            return Ok(0);
        };
        self.push_undo();
        let mut next = 0;
        let mut paragraph = 0;
        for section in &mut self.document.sections {
            for block in &mut section.blocks {
                if let Block::Paragraph(para) = block {
                    let start = next;
                    while next < matches.len() && matches[next].start().paragraph == paragraph {
                        next += 1;
                    }
                    if start != next {
                        replace_ranges(para, &matches[start..next], replacement);
                    }
                    paragraph += 1;
                }
            }
        }
        self.edit_focus = EditFocus::Body;
        self.selection = Selection::caret(DocPosition::new(
            first.start().paragraph,
            first.start().offset + replacement.chars().count(),
        ));
        self.dirty = true;
        self.sync_typing_style_from_caret();
        Ok(matches.len())
    }
}

fn check_replacement(replacement: &str) -> Result<(), EditError> {
    if replacement.contains(['\r', '\n']) {
        Err(EditError::InvalidReplacement)
    } else {
        Ok(())
    }
}

// Walk each affected paragraph once; avoid repeatedly cloning/splitting its runs.
fn replace_ranges(paragraph: &mut Paragraph, matches: &[Selection], replacement: &str) {
    let mut characters = paragraph
        .runs
        .iter()
        .flat_map(|run| run.text.chars().map(move |c| (c, &run.style, &run.link)));
    let mut runs = Vec::new();
    let mut offset = 0;
    let mut next = 0;
    while let Some((character, style, link)) = characters.next() {
        if next < matches.len() && matches[next].start().offset == offset {
            push_piece(&mut runs, replacement, style, link);
            let length = matches[next].end().offset - offset;
            for _ in 1..length {
                characters.next();
            }
            offset += length;
            next += 1;
        } else {
            let mut buffer = [0; 4];
            push_piece(&mut runs, character.encode_utf8(&mut buffer), style, link);
            offset += 1;
        }
    }
    paragraph.runs = runs;
    paragraph.normalize();
}

fn push_piece(runs: &mut Vec<Run>, text: &str, style: &TextStyle, link: &Option<String>) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = runs
        .last_mut()
        .filter(|last| &last.style == style && &last.link == link)
    {
        last.text.push_str(text);
    } else {
        runs.push(Run::new(text, style.clone()).with_link(link.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Document, Section, Table};

    #[test]
    fn unicode_offsets_next_previous_wrap_and_read_only_search() {
        let mut editor = DocumentEditor::new(Document::with_text("🙂日本語 日本語\n日本語"));
        let original = editor.document().clone();
        let first = editor.find_body("日本語", true).unwrap();
        assert_eq!(first.start(), DocPosition::new(0, 1));
        assert_eq!(first.end(), DocPosition::new(0, 4));
        assert_eq!(editor.selected_text(), "日本語");
        assert_eq!(
            editor.find_body("日本語", true).unwrap().start(),
            DocPosition::new(0, 5)
        );
        assert_eq!(
            editor.find_body("日本語", true).unwrap().start(),
            DocPosition::new(1, 0)
        );
        assert_eq!(editor.find_body("日本語", true), Some(first));
        assert_eq!(
            editor.find_body("日本語", false).unwrap().start(),
            DocPosition::new(1, 0)
        );
        let selection = editor.selection();
        assert!(editor.find_body("missing", true).is_none());
        assert_eq!(editor.selection(), selection);
        assert_eq!(editor.document(), &original);
        assert!(!editor.is_dirty());
        assert!(!editor.can_undo());
    }

    #[test]
    fn search_crosses_runs_and_sections_but_excludes_tables_and_margins() {
        let mut doc = Document::with_text("日本");
        doc.paragraph_mut(0).unwrap().runs.push(Run::plain("語"));
        doc.main_section_mut().header = Some(Paragraph::from_text("日本語"));
        doc.main_section_mut().footer = Some(Paragraph::from_text("日本語"));
        let mut table = Table::new(1, 1);
        table.rows[0].cells[0].paragraphs[0] = Paragraph::from_text("日本語");
        doc.blocks_mut().push(Block::Table(table));
        let mut section = Section::empty();
        section.blocks = vec![Block::Paragraph(Paragraph::from_text("日本語"))];
        doc.sections.push(section);
        let mut editor = DocumentEditor::new(doc);
        assert_eq!(editor.body_matches("日本語").len(), 2);
        let header = editor.document().header().cloned();
        let table = editor.document().blocks()[1].clone();
        assert_eq!(editor.replace_all_body("日本語", "English").unwrap(), 2);
        assert_eq!(editor.document().header().cloned(), header);
        assert_eq!(editor.document().blocks()[1], table);
        assert_eq!(
            editor.document().paragraph(1).unwrap().plain_text(),
            "English"
        );
    }

    #[test]
    fn replacement_preserves_surrounding_styles_links_and_paragraph_properties() {
        let mut doc = Document::with_text("");
        let bold = TextStyle {
            bold: true,
            ..Default::default()
        };
        let paragraph = doc.paragraph_mut(0).unwrap();
        paragraph.style.alignment = crate::Alignment::Center;
        paragraph.runs = vec![
            Run::plain("before "),
            Run::new("日本", bold.clone()).with_link(Some("https://example.com".into())),
            Run::plain("語 after"),
        ];
        let mut editor = DocumentEditor::new(doc.clone());
        editor.find_body("日本語", true).unwrap();
        assert!(editor.replace_body_match("日本語", "置換🙂").unwrap());
        let paragraph = editor.document().paragraph(0).unwrap();
        assert_eq!(paragraph.plain_text(), "before 置換🙂 after");
        assert_eq!(paragraph.style, doc.paragraph(0).unwrap().style);
        assert_eq!(paragraph.runs[1].style, bold);
        assert_eq!(
            paragraph.runs[1].link.as_deref(),
            Some("https://example.com")
        );
        assert_eq!(paragraph.runs[2].style, TextStyle::default());
        assert!(editor.undo());
        assert_eq!(editor.document(), &doc);
        assert!(editor.redo());
        assert_eq!(
            editor.document().paragraph(0).unwrap().plain_text(),
            "before 置換🙂 after"
        );
    }

    #[test]
    fn replace_all_is_one_history_entry_handles_delete_and_does_not_rescan() {
        let mut editor = DocumentEditor::new(Document::with_text("aaa aa\naa"));
        let original = editor.document().clone();
        assert_eq!(editor.replace_all_body("aa", "aaaa").unwrap(), 3);
        assert_eq!(editor.document().plain_text(), "aaaaa aaaa\naaaa");
        assert!(editor.undo());
        assert_eq!(editor.document(), &original);
        assert!(!editor.can_undo());
        assert!(editor.redo());
        assert_eq!(editor.replace_all_body("aaaa", "").unwrap(), 3);
        assert_eq!(editor.document().plain_text(), "a \n");
    }

    #[test]
    fn no_op_invalid_input_and_stale_selection_preserve_history_and_focus() {
        let mut editor = DocumentEditor::new(Document::with_text("Text text"));
        editor.find_body("Text", true);
        assert!(!editor.replace_body_match("other", "changed").unwrap());
        assert!(!editor.replace_body_match("Text", "Text").unwrap());
        assert_eq!(editor.replace_all_body("", "changed").unwrap(), 0);
        assert_eq!(editor.replace_all_body("not found", "changed").unwrap(), 0);
        assert_eq!(
            editor.replace_all_body("Text", "a\nb"),
            Err(EditError::InvalidReplacement)
        );
        assert_eq!(editor.body_matches("text").len(), 1);
        assert!(editor.body_matches("Text\ntext").is_empty());
        assert!(!editor.is_dirty());
        assert!(!editor.can_undo());
        editor.document_mut().ensure_header_mut().runs = vec![Run::plain("header")];
        editor.focus_header(2);
        editor.mark_clean();
        let selection = editor.selection();
        assert!(editor.find_body("missing", true).is_none());
        assert_eq!(editor.edit_focus(), EditFocus::Header);
        assert_eq!(editor.selection(), selection);
        assert!(!editor.is_dirty());
        assert!(!editor.replace_body_match("header", "changed").unwrap());
    }

    #[test]
    fn many_matches_are_replaced_in_one_pass_and_undo_restores_original_focus() {
        let mut editor = DocumentEditor::new(Document::with_text(&"日本語 ".repeat(10_000)));
        editor.focus_header(0);
        let original = editor.document().clone();
        assert_eq!(editor.replace_all_body("日本語", "🙂").unwrap(), 10_000);
        assert_eq!(editor.document().plain_text(), "🙂 ".repeat(10_000));
        assert!(editor.undo());
        assert_eq!(editor.document(), &original);
        assert_eq!(editor.edit_focus(), EditFocus::Header);
    }
}
