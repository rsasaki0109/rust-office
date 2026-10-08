//! Core document model and editing engine for rust-office.
//!
//! This crate is GUI-free: editing, undo/redo, and the document tree can be
//! unit-tested without egui or any windowing system.

mod document;
mod editor;
mod selection;
pub mod storage;
mod style;

pub use document::{
    extension_for_mime, mime_from_path, Block, Document, Image, ImageSource, Paragraph, Run,
    Section, Table, TableCell, TableRow,
};
pub use editor::{DocumentEditor, EditError};
pub use selection::{CellAddress, DocPosition, EditFocus, Selection};
pub use style::{
    apply_named_paragraph_defaults, Alignment, ListKind, ListStyle, NamedParagraphStyle, PageStyle,
    ParagraphStyle, TextStyle,
};
