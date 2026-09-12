//! Presentation theme (colors + fonts).

use serde::{Deserialize, Serialize};

/// Named theme applied to new slides / canvas chrome.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Theme {
    pub name: String,
    /// Background RGB 0–255.
    pub background: [u8; 3],
    pub title_color: [u8; 3],
    pub body_color: [u8; 3],
    pub accent: [u8; 3],
    pub title_font_pt: f32,
    pub body_font_pt: f32,
}

impl Theme {
    pub fn light() -> Self {
        Self {
            name: "Light".into(),
            background: [255, 255, 255],
            title_color: [20, 30, 50],
            body_color: [40, 45, 55],
            accent: [30, 90, 180],
            title_font_pt: 36.0,
            body_font_pt: 20.0,
        }
    }

    pub fn dark() -> Self {
        Self {
            name: "Dark".into(),
            background: [28, 32, 40],
            title_color: [240, 242, 248],
            body_color: [200, 205, 215],
            accent: [90, 160, 255],
            title_font_pt: 36.0,
            body_font_pt: 20.0,
        }
    }

    pub fn ocean() -> Self {
        Self {
            name: "Ocean".into(),
            background: [232, 244, 248],
            title_color: [10, 60, 90],
            body_color: [30, 50, 70],
            accent: [0, 140, 160],
            title_font_pt: 36.0,
            body_font_pt: 20.0,
        }
    }

    pub fn builtins() -> Vec<Self> {
        vec![Self::light(), Self::dark(), Self::ocean()]
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::light()
    }
}
