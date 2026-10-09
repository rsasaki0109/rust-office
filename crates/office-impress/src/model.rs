//! Slide / presentation document model.

use serde::{Deserialize, Serialize};

use crate::theme::Theme;

/// Standard 16:9 slide size in points (like PowerPoint widescreen).
pub const SLIDE_WIDTH_PT: f32 = 960.0;
pub const SLIDE_HEIGHT_PT: f32 = 540.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextBox {
    pub text: String,
    /// Normalized position 0–1 within the slide.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl TextBox {
    pub fn title(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            x: 0.08,
            y: 0.12,
            w: 0.84,
            h: 0.22,
        }
    }

    pub fn body(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            x: 0.08,
            y: 0.38,
            w: 0.84,
            h: 0.50,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Slide {
    pub title: TextBox,
    pub body: TextBox,
    /// Optional speaker notes.
    #[serde(default)]
    pub notes: String,
}

impl Slide {
    pub fn blank() -> Self {
        Self {
            title: TextBox::title(""),
            body: TextBox::body(""),
            notes: String::new(),
        }
    }

    pub fn title_and_body(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            title: TextBox::title(title),
            body: TextBox::body(body),
            notes: String::new(),
        }
    }

    pub fn plain_text(&self) -> String {
        let mut out = self.title.text.clone();
        if !self.body.text.is_empty() {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&self.body.text);
        }
        out
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Change {
    Insert {
        index: usize,
        slide: Slide,
        previous: usize,
    },
    Delete {
        index: usize,
        slide: Slide,
        next: usize,
    },
    Move {
        from: usize,
        to: usize,
    },
    Slide {
        index: usize,
        before: Slide,
        after: Slide,
    },
    Theme {
        before: Theme,
        after: Theme,
    },
}

#[derive(Debug, Clone, PartialEq)]
struct Edit {
    change: Change,
    before: u64,
    after: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Presentation {
    pub title: String,
    pub theme: Theme,
    pub slides: Vec<Slide>,
    #[serde(default)]
    pub active: usize,
    #[serde(skip)]
    dirty: bool,
    #[serde(skip)]
    undo: Vec<Edit>,
    #[serde(skip)]
    redo: Vec<Edit>,
    #[serde(skip)]
    revision: u64,
    #[serde(skip)]
    saved_revision: u64,
    #[serde(skip)]
    next_revision: u64,
}

impl Default for Presentation {
    fn default() -> Self {
        Self::new()
    }
}

impl Presentation {
    pub fn new() -> Self {
        Self {
            title: "Untitled presentation".into(),
            theme: Theme::default(),
            slides: vec![Slide::title_and_body(
                "Welcome to rust-office Impress",
                "• Add slides from the Insert menu\n• Edit title and body on the right\n• File → Export PPTX for a minimal package",
            )],
            active: 0,
            dirty: false,
            undo: Vec::new(),
            redo: Vec::new(),
            revision: 0,
            saved_revision: 0,
            next_revision: 0,
        }
    }

    pub fn demo() -> Self {
        let mut p = Self::new();
        p.title = "Demo deck".into();
        p.theme = Theme::ocean();
        p.slides = vec![
            Slide::title_and_body(
                "rust-office Impress",
                "Native presentation editing in Rust.\nThemes · slides · PPTX export (MVP).",
            ),
            Slide::title_and_body(
                "Roadmap",
                "1. Writer\n2. Calc\n3. Impress\n4. Packaging & CI",
            ),
            Slide::title_and_body("Thank you", "Questions?"),
        ];
        p.active = 0;
        p.mark_clean();
        p
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn mark_clean(&mut self) {
        self.dirty = false;
        self.saved_revision = self.revision;
    }

    /// Direct mutations cannot safely retain an earlier operation history.
    pub fn mark_dirty(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.next_revision += 1;
        self.revision = self.next_revision;
        self.dirty = true;
    }

    fn record(&mut self, change: Change) {
        self.next_revision += 1;
        self.undo.push(Edit {
            change,
            before: self.revision,
            after: self.next_revision,
        });
        if self.undo.len() > 100 {
            self.undo.remove(0);
        }
        self.revision = self.next_revision;
        self.redo.clear();
        self.dirty = self.revision != self.saved_revision;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    fn replay(&mut self, change: &Change, forward: bool) {
        match change {
            Change::Insert {
                index,
                slide,
                previous,
            } => {
                if forward {
                    self.slides.insert(*index, slide.clone());
                    self.active = *index;
                } else {
                    self.slides.remove(*index);
                    self.active = *previous;
                }
            }
            Change::Delete { index, slide, next } => {
                if forward {
                    self.slides.remove(*index);
                    self.active = *next;
                } else {
                    self.slides.insert(*index, slide.clone());
                    self.active = *index;
                }
            }
            Change::Move { from, to } => {
                let (from, to) = if forward { (*from, *to) } else { (*to, *from) };
                let slide = self.slides.remove(from);
                self.slides.insert(to, slide);
                self.active = to;
            }
            Change::Slide {
                index,
                before,
                after,
            } => {
                self.slides[*index] = if forward {
                    after.clone()
                } else {
                    before.clone()
                };
                self.active = *index;
            }
            Change::Theme { before, after } => {
                self.theme = if forward {
                    after.clone()
                } else {
                    before.clone()
                };
            }
        }
    }

    pub fn undo(&mut self) -> bool {
        let Some(edit) = self.undo.pop() else {
            return false;
        };
        self.replay(&edit.change, false);
        self.revision = edit.before;
        self.dirty = self.revision != self.saved_revision;
        self.redo.push(edit);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(edit) = self.redo.pop() else {
            return false;
        };
        self.replay(&edit.change, true);
        self.revision = edit.after;
        self.dirty = self.revision != self.saved_revision;
        self.undo.push(edit);
        true
    }

    /// Coalesce consecutive typing in one field, but never across a save point.
    pub fn update_active_slide(&mut self, slide: Slide, coalesce: bool) -> bool {
        let Some(before) = self.active_slide().cloned() else {
            return false;
        };
        if before == slide {
            return false;
        }
        if coalesce && self.redo.is_empty() && self.revision != self.saved_revision {
            if let Some(Edit {
                change: Change::Slide { index, after, .. },
                ..
            }) = self.undo.last_mut()
            {
                if *index == self.active {
                    *after = slide.clone();
                    self.slides[self.active] = slide;
                    self.dirty = true;
                    return true;
                }
            }
        }
        self.slides[self.active] = slide.clone();
        self.record(Change::Slide {
            index: self.active,
            before,
            after: slide,
        });
        true
    }

    pub fn active_slide(&self) -> Option<&Slide> {
        self.slides.get(self.active)
    }

    pub fn active_slide_mut(&mut self) -> Option<&mut Slide> {
        self.slides.get_mut(self.active)
    }

    pub fn set_active(&mut self, index: usize) {
        if index < self.slides.len() {
            self.active = index;
        }
    }

    fn insert_slide(&mut self, index: usize, slide: Slide) {
        let previous = self.active;
        self.slides.insert(index, slide.clone());
        self.active = index;
        self.record(Change::Insert {
            index,
            slide,
            previous,
        });
    }

    pub fn add_slide(&mut self) {
        self.insert_slide(self.slides.len(), Slide::blank());
    }

    pub fn insert_slide_after_current(&mut self) {
        self.insert_slide(
            self.active.saturating_add(1).min(self.slides.len()),
            Slide::blank(),
        );
    }

    pub fn duplicate_active_slide(&mut self) -> bool {
        let Some(slide) = self.active_slide().cloned() else {
            return false;
        };
        self.insert_slide(self.active + 1, slide);
        true
    }

    pub fn move_active_slide(&mut self, to: usize) -> bool {
        let from = self.active;
        if from >= self.slides.len() || to >= self.slides.len() || from == to {
            return false;
        }
        let slide = self.slides.remove(from);
        self.slides.insert(to, slide);
        self.active = to;
        self.record(Change::Move { from, to });
        true
    }

    pub fn delete_active_slide(&mut self) -> bool {
        if self.slides.len() <= 1 || self.active >= self.slides.len() {
            return false;
        }
        let index = self.active;
        let slide = self.slides.remove(index);
        self.active = index.min(self.slides.len() - 1);
        self.record(Change::Delete {
            index,
            slide,
            next: self.active,
        });
        true
    }

    pub fn apply_theme(&mut self, theme: Theme) {
        if self.theme == theme {
            return;
        }
        let before = std::mem::replace(&mut self.theme, theme.clone());
        self.record(Change::Theme {
            before,
            after: theme,
        });
    }
}

#[cfg(test)]
mod history_tests {
    use super::*;

    #[test]
    fn duplicate_move_delete_restore_content_selection_and_saved_revision() {
        let mut p = Presentation::demo();
        p.slides[0].notes = "日本語のノート".into();
        p.slides[0].title.x = 0.23;
        let original = p.slides.clone();
        assert!(p.duplicate_active_slide());
        assert_eq!(p.slides[1], original[0]);
        assert!(p.move_active_slide(3));
        p.mark_clean();
        assert!(p.delete_active_slide());
        assert_eq!(p.active, 2);
        assert!(p.undo());
        assert_eq!(p.active, 3);
        assert_eq!(p.slides[3], original[0]);
        assert!(!p.is_dirty());
        assert!(p.undo());
        assert_eq!(p.active, 1);
        assert!(p.is_dirty());
        assert!(p.redo());
        assert!(!p.is_dirty());
        assert!(p.redo());
        assert!(p.is_dirty());
    }

    #[test]
    fn typing_coalesces_but_save_and_branch_are_boundaries() {
        let mut p = Presentation::new();
        let original = p.active_slide().unwrap().clone();
        let mut slide = original.clone();
        slide.title.text = "a".into();
        p.update_active_slide(slide.clone(), false);
        slide.title.text = "ab".into();
        p.update_active_slide(slide.clone(), true);
        p.mark_clean();
        slide.title.text = "abc".into();
        p.update_active_slide(slide, true);
        p.undo();
        assert_eq!(p.active_slide().unwrap().title.text, "ab");
        assert!(!p.is_dirty());
        p.undo();
        assert_eq!(p.active_slide().unwrap(), &original);
        p.redo();
        assert!(!p.is_dirty());
        p.apply_theme(Theme::dark());
        assert!(!p.can_redo());
        p.undo();
        assert!(!p.is_dirty());
    }

    #[test]
    fn no_op_last_slide_and_invalid_moves_preserve_redo() {
        let mut p = Presentation::new();
        let original = p.slides.clone();
        assert!(!p.delete_active_slide());
        assert_eq!(p.slides, original);
        assert!(!p.is_dirty());
        p.add_slide();
        p.undo();
        assert!(!p.move_active_slide(0));
        assert!(!p.move_active_slide(100));
        assert!(!p.delete_active_slide());
        p.apply_theme(p.theme.clone());
        assert!(p.can_redo());
        assert!(p.redo());
    }

    #[test]
    fn bounded_history_and_serialization_exclude_runtime_state() {
        let mut p = Presentation::new();
        for _ in 0..110 {
            p.add_slide();
        }
        let json = serde_json::to_value(&p).unwrap();
        assert!(json.get("undo").is_none());
        assert!(json.get("revision").is_none());
        let loaded: Presentation = serde_json::from_value(json).unwrap();
        assert_eq!(loaded.slides, p.slides);
        assert!(!loaded.is_dirty());
        assert!(!loaded.can_undo());
        for _ in 0..100 {
            assert!(p.undo());
        }
        assert!(!p.undo());
        assert_eq!(p.slides.len(), 11);
    }

    #[test]
    fn theme_and_native_roundtrip_keep_edited_slide_order_and_notes() {
        let mut p = Presentation::demo();
        let mut slide = p.active_slide().unwrap().clone();
        slide.body.text = "更新本文".into();
        slide.notes = "発表ノート".into();
        p.update_active_slide(slide.clone(), false);
        p.apply_theme(Theme::dark());
        p.undo();
        assert_eq!(p.theme, Theme::ocean());
        p.redo();
        p.duplicate_active_slide();
        p.move_active_slide(3);
        let bytes = serde_json::to_vec(&p).unwrap();
        let loaded: Presentation = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(loaded.slides[3], slide);
        assert_eq!(loaded.theme, Theme::dark());
    }
}
