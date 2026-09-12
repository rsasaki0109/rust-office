//! Character styles and paragraph styles.

use serde::{Deserialize, Serialize};

/// Horizontal alignment of a paragraph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Alignment {
    #[default]
    Left,
    Center,
    Right,
    Justify,
}

/// List marker kind for a paragraph that participates in a list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListKind {
    Bullet,
    Numbered,
}

/// Paragraph list membership (flat consecutive paragraphs form a list).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ListStyle {
    pub kind: ListKind,
    /// Nesting level (0 = top). Indent/outdent adjusts this (max 8).
    pub level: u8,
}

impl ListStyle {
    pub const MAX_LEVEL: u8 = 8;

    pub fn bullet() -> Self {
        Self {
            kind: ListKind::Bullet,
            level: 0,
        }
    }

    pub fn numbered() -> Self {
        Self {
            kind: ListKind::Numbered,
            level: 0,
        }
    }

    pub fn with_level(mut self, level: u8) -> Self {
        self.level = level.min(Self::MAX_LEVEL);
        self
    }
}

/// Built-in named paragraph styles (Word/LibreOffice-style shortcuts).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum NamedParagraphStyle {
    #[default]
    Normal,
    Heading1,
    Heading2,
    Heading3,
}

impl NamedParagraphStyle {
    pub const ALL: &[NamedParagraphStyle] = &[
        NamedParagraphStyle::Normal,
        NamedParagraphStyle::Heading1,
        NamedParagraphStyle::Heading2,
        NamedParagraphStyle::Heading3,
    ];

    pub fn display_name(self) -> &'static str {
        match self {
            NamedParagraphStyle::Normal => "Normal",
            NamedParagraphStyle::Heading1 => "Heading 1",
            NamedParagraphStyle::Heading2 => "Heading 2",
            NamedParagraphStyle::Heading3 => "Heading 3",
        }
    }

    /// Default paragraph metrics for this named style (list/alignment left to caller).
    pub fn builtin_paragraph(self) -> ParagraphStyle {
        match self {
            NamedParagraphStyle::Normal => ParagraphStyle {
                named: self,
                alignment: Alignment::Left,
                space_after: 6.0,
                line_spacing: 1.15,
                list: None,
            },
            NamedParagraphStyle::Heading1 => ParagraphStyle {
                named: self,
                alignment: Alignment::Left,
                space_after: 14.0,
                line_spacing: 1.2,
                list: None,
            },
            NamedParagraphStyle::Heading2 => ParagraphStyle {
                named: self,
                alignment: Alignment::Left,
                space_after: 10.0,
                line_spacing: 1.2,
                list: None,
            },
            NamedParagraphStyle::Heading3 => ParagraphStyle {
                named: self,
                alignment: Alignment::Left,
                space_after: 8.0,
                line_spacing: 1.2,
                list: None,
            },
        }
    }

    /// Default character formatting applied when assigning this named style.
    pub fn builtin_text(self) -> TextStyle {
        match self {
            NamedParagraphStyle::Normal => TextStyle::default(),
            NamedParagraphStyle::Heading1 => TextStyle {
                bold: true,
                italic: false,
                underline: false,
                font_size: 20.0,
                font_family: default_font_family(),
            },
            NamedParagraphStyle::Heading2 => TextStyle {
                bold: true,
                italic: false,
                underline: false,
                font_size: 16.0,
                font_family: default_font_family(),
            },
            NamedParagraphStyle::Heading3 => TextStyle {
                bold: true,
                italic: false,
                underline: false,
                font_size: 14.0,
                font_family: default_font_family(),
            },
        }
    }
}

/// Character-level formatting applied to a [`crate::Run`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextStyle {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    /// Font size in points.
    pub font_size: f32,
    /// Font family name (logical; renderer maps to a concrete face).
    #[serde(default = "default_font_family")]
    pub font_family: String,
}

fn default_font_family() -> String {
    "Sans".to_string()
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            bold: false,
            italic: false,
            underline: false,
            font_size: 12.0,
            font_family: "Sans".to_string(),
        }
    }
}

/// Paragraph-level formatting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParagraphStyle {
    /// Named style identity (Normal / Heading N). Formatting is also stored inline.
    #[serde(default, skip_serializing_if = "is_named_normal")]
    pub named: NamedParagraphStyle,
    pub alignment: Alignment,
    /// Space after the paragraph in points.
    pub space_after: f32,
    /// Line spacing multiplier (1.0 = single).
    pub line_spacing: f32,
    /// When set, this paragraph is a list item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list: Option<ListStyle>,
}

fn is_named_normal(named: &NamedParagraphStyle) -> bool {
    *named == NamedParagraphStyle::Normal
}

impl Default for ParagraphStyle {
    fn default() -> Self {
        NamedParagraphStyle::Normal.builtin_paragraph()
    }
}

/// Apply named-style defaults onto `style`, preserving alignment and list.
pub fn apply_named_paragraph_defaults(style: &mut ParagraphStyle, named: NamedParagraphStyle) {
    let list = style.list;
    let alignment = style.alignment;
    *style = named.builtin_paragraph();
    style.alignment = alignment;
    style.list = list;
}

/// Page geometry and margins. v0.1 uses a fixed A4 page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageStyle {
    /// Page width in points (1 point = 1/72 inch).
    pub width: f32,
    /// Page height in points.
    pub height: f32,
    pub margin_top: f32,
    pub margin_bottom: f32,
    pub margin_left: f32,
    pub margin_right: f32,
}

impl Default for PageStyle {
    fn default() -> Self {
        // A4 at 72 dpi: 210mm × 297mm ≈ 595.28 × 841.89 pt
        Self {
            width: 595.28,
            height: 841.89,
            margin_top: 72.0,
            margin_bottom: 72.0,
            margin_left: 72.0,
            margin_right: 72.0,
        }
    }
}

impl PageStyle {
    pub fn content_width(&self) -> f32 {
        (self.width - self.margin_left - self.margin_right).max(1.0)
    }

    pub fn content_height(&self) -> f32 {
        (self.height - self.margin_top - self.margin_bottom).max(1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heading_builtins_are_larger_than_normal() {
        assert!(NamedParagraphStyle::Heading1.builtin_text().font_size > 12.0);
        assert!(NamedParagraphStyle::Heading1.builtin_text().bold);
        assert_eq!(
            NamedParagraphStyle::Normal.builtin_text().font_size,
            12.0
        );
    }
}
