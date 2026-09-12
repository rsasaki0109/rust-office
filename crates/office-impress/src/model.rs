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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Presentation {
    pub title: String,
    pub theme: Theme,
    pub slides: Vec<Slide>,
    #[serde(default)]
    pub active: usize,
    #[serde(skip)]
    dirty: bool,
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
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
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

    pub fn add_slide(&mut self) {
        self.slides.push(Slide::blank());
        self.active = self.slides.len() - 1;
        self.dirty = true;
    }

    pub fn insert_slide_after_current(&mut self) {
        let idx = self.active + 1;
        self.slides.insert(idx, Slide::blank());
        self.active = idx;
        self.dirty = true;
    }

    pub fn delete_active_slide(&mut self) {
        if self.slides.len() <= 1 {
            if let Some(s) = self.slides.get_mut(0) {
                *s = Slide::blank();
            }
            self.dirty = true;
            return;
        }
        self.slides.remove(self.active);
        if self.active >= self.slides.len() {
            self.active = self.slides.len() - 1;
        }
        self.dirty = true;
    }

    pub fn apply_theme(&mut self, theme: Theme) {
        self.theme = theme;
        self.dirty = true;
    }
}
