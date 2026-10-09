//! Caret / selection positions inside a document.

use serde::{Deserialize, Serialize};

/// A caret position identified by paragraph index and character offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct DocPosition {
    pub paragraph: usize,
    /// Offset in Unicode scalar values (chars), not bytes.
    pub offset: usize,
}

impl DocPosition {
    pub fn new(paragraph: usize, offset: usize) -> Self {
        Self { paragraph, offset }
    }

    pub fn zero() -> Self {
        Self {
            paragraph: 0,
            offset: 0,
        }
    }
}

/// Address of a table cell in the document block list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CellAddress {
    /// Index into [`crate::Document::sections`].
    #[serde(default)]
    pub section: usize,
    /// Index into that section's `blocks`.
    pub block: usize,
    pub row: usize,
    pub col: usize,
}

impl CellAddress {
    /// Cell in section 0 (legacy helper).
    pub fn new(block: usize, row: usize, col: usize) -> Self {
        Self {
            section: 0,
            block,
            row,
            col,
        }
    }

    pub fn in_section(section: usize, block: usize, row: usize, col: usize) -> Self {
        Self {
            section,
            block,
            row,
            col,
        }
    }
}

/// Where editing currently targets: body flow, a table cell, or page margins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum EditFocus {
    #[default]
    Body,
    Cell(CellAddress),
    Header(usize),
    Footer(usize),
}

impl EditFocus {
    pub fn is_body(self) -> bool {
        matches!(self, Self::Body)
    }

    pub fn is_margin(self) -> bool {
        matches!(self, Self::Header(_) | Self::Footer(_))
    }

    /// Single-paragraph target (cell or margin) — Enter does not split paragraphs.
    pub fn is_single_paragraph(self) -> bool {
        matches!(self, Self::Cell(_) | Self::Header(_) | Self::Footer(_))
    }

    pub fn cell(self) -> Option<CellAddress> {
        match self {
            Self::Cell(a) => Some(a),
            _ => None,
        }
    }
}

/// A text selection. When `anchor == focus` the selection is a caret.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selection {
    pub anchor: DocPosition,
    pub focus: DocPosition,
}

impl Default for Selection {
    fn default() -> Self {
        Self::caret(DocPosition::zero())
    }
}

impl Selection {
    pub fn caret(pos: DocPosition) -> Self {
        Self {
            anchor: pos,
            focus: pos,
        }
    }

    pub fn is_collapsed(&self) -> bool {
        self.anchor == self.focus
    }

    pub fn start(&self) -> DocPosition {
        if self.anchor <= self.focus {
            self.anchor
        } else {
            self.focus
        }
    }

    pub fn end(&self) -> DocPosition {
        if self.anchor <= self.focus {
            self.focus
        } else {
            self.anchor
        }
    }

    pub fn with_focus(self, focus: DocPosition) -> Self {
        Self {
            anchor: self.anchor,
            focus,
        }
    }
}
