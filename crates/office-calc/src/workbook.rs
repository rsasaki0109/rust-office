//! Workbook container and dirty tracking.

use serde::{Deserialize, Serialize};

use std::collections::BTreeMap;

use crate::addr::{CellAddr, CellRange};
use crate::cell::Cell;
use crate::clipboard::{self, ClipboardError};
use crate::sheet::Sheet;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workbook {
    pub sheets: Vec<Sheet>,
    pub active: usize,
    #[serde(skip)]
    dirty: bool,
    #[serde(skip)]
    undo_stack: Vec<Edit>,
    #[serde(skip)]
    redo_stack: Vec<Edit>,
    #[serde(skip)]
    revision: u64,
    #[serde(skip)]
    saved_revision: u64,
    #[serde(skip)]
    next_revision: u64,
}

#[derive(Debug, Clone)]
struct CellChange {
    addr: CellAddr,
    before: Option<Cell>,
    after: Option<Cell>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SheetError {
    #[error("Worksheet does not exist")]
    Missing,
    #[error("Invalid worksheet name: {0}")]
    InvalidName(String),
    #[error("A worksheet with this name already exists")]
    DuplicateName,
    #[error("The last worksheet cannot be deleted")]
    LastSheet,
}

#[derive(Debug, Clone)]
enum Change {
    Dimensions {
        sheet: usize,
        axis: crate::DimensionAxis,
        sizes: Vec<(u32, f64, f64)>,
        before_bounds: (u32, u32),
        after_bounds: (u32, u32),
    },
    Formats {
        sheet: usize,
        cells: Vec<(CellAddr, crate::CellFormat, crate::CellFormat)>,
        before_bounds: (u32, u32),
        after_bounds: (u32, u32),
    },
    Cells {
        sheet: usize,
        cells: Vec<CellChange>,
        before_bounds: (u32, u32),
        after_bounds: (u32, u32),
    },
    RenameSheet {
        index: usize,
        before: String,
        after: String,
    },
    DeleteSheet {
        sheet: Sheet,
        index: usize,
        previous_active: usize,
        next_active: usize,
    },
    AddSheet {
        sheet: Sheet,
        index: usize,
        previous_active: usize,
    },
}

#[derive(Debug, Clone)]
struct Edit {
    change: Change,
    before_revision: u64,
    after_revision: u64,
}

impl Default for Workbook {
    fn default() -> Self {
        Self::new()
    }
}

impl Workbook {
    pub fn new() -> Self {
        Self {
            sheets: vec![Sheet::new("Sheet1")],
            active: 0,
            dirty: false,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            revision: 0,
            saved_revision: 0,
            next_revision: 0,
        }
    }

    pub fn active_sheet(&self) -> &Sheet {
        &self.sheets[self.active]
    }

    pub fn active_sheet_mut(&mut self) -> &mut Sheet {
        &mut self.sheets[self.active]
    }

    pub fn set_cell(&mut self, addr: CellAddr, raw: impl Into<String>) {
        self.set_cells([(addr, raw.into())]);
    }

    /// Apply an entire range operation as one undoable edit. Unchanged values
    /// neither dirty the workbook nor invalidate redo history.
    pub fn set_cells<S: Into<String>>(&mut self, cells: impl IntoIterator<Item = (CellAddr, S)>) {
        let mut updates = BTreeMap::new();
        for (addr, raw) in cells {
            updates.insert((addr.row, addr.col), raw.into());
        }
        let sheet = self.active_sheet();
        let before_bounds = (sheet.rows, sheet.cols);
        let changes: Vec<_> = updates
            .into_iter()
            .filter_map(|((row, col), raw)| {
                let addr = CellAddr::new(col, row);
                let before = sheet.get(addr).cloned();
                let after = (!raw.trim().is_empty()).then(|| Cell::new(raw));
                (before != after).then_some(CellChange {
                    addr,
                    before,
                    after,
                })
            })
            .collect();
        if changes.is_empty() {
            return;
        }
        for change in &changes {
            self.active_sheet_mut().set_raw(
                change.addr,
                change.after.as_ref().map_or("", |cell| cell.raw.as_str()),
            );
        }
        let sheet = self.active_sheet();
        let after_bounds = (sheet.rows, sheet.cols);
        self.record(Change::Cells {
            sheet: self.active,
            cells: changes,
            before_bounds,
            after_bounds,
        });
    }

    /// Apply a presentation change without touching raw values or formulas.
    pub fn format_range(
        &mut self,
        range: CellRange,
        update: crate::FormatChange,
    ) -> Result<bool, ClipboardError> {
        let range = clipboard::validate_range(range)?;
        let sheet = self.active_sheet();
        let before_bounds = (sheet.rows, sheet.cols);
        let cells: Vec<_> = range
            .iter()
            .filter_map(|addr| {
                let before = sheet.format(addr);
                let after = update.apply(before.clone());
                (before != after).then_some((addr, before, after))
            })
            .collect();
        if cells.is_empty() {
            return Ok(false);
        }
        for (addr, _, after) in &cells {
            self.active_sheet_mut().set_format(*addr, after.clone());
        }
        let sheet = self.active_sheet();
        let after_bounds = (sheet.rows, sheet.cols);
        self.record(Change::Formats {
            sheet: self.active,
            cells,
            before_bounds,
            after_bounds,
        });
        Ok(true)
    }

    /// Resize a row/column range as one undoable edit; None restores its sheet default.
    pub fn resize_range(
        &mut self,
        axis: crate::DimensionAxis,
        first: u32,
        last: u32,
        size: Option<f64>,
    ) -> Result<bool, crate::DimensionError> {
        let limit = match axis {
            crate::DimensionAxis::Columns => crate::MAX_SHEET_COLS,
            crate::DimensionAxis::Rows => crate::MAX_SHEET_ROWS,
        };
        if first > last || last >= limit {
            return Err(crate::DimensionError::OutOfBounds);
        }
        let sheet = self.active_sheet();
        let size = size.unwrap_or(sheet.default_dimension(axis));
        axis.validate(size)?;
        let before_bounds = (sheet.rows, sheet.cols);
        let sizes: Vec<_> = (first..=last)
            .filter_map(|index| {
                let before = sheet.dimensions.get(axis, index);
                (before != size).then_some((index, before, size))
            })
            .collect();
        if sizes.is_empty() {
            return Ok(false);
        }
        for &(index, _, after) in &sizes {
            self.active_sheet_mut().set_dimension(axis, index, after);
        }
        let sheet = self.active_sheet();
        let after_bounds = (sheet.rows, sheet.cols);
        self.record(Change::Dimensions {
            sheet: self.active,
            axis,
            sizes,
            before_bounds,
            after_bounds,
        });
        Ok(true)
    }

    pub fn copy_tsv(&self, range: CellRange) -> Result<String, ClipboardError> {
        let range = clipboard::validate_range(range)?;
        let sheet = self.active_sheet();
        let mut rows = Vec::new();
        for row in range.start.row..=range.end.row {
            rows.push(
                (range.start.col..=range.end.col)
                    .map(|col| clipboard::quote_field(sheet.raw(CellAddr::new(col, row))))
                    .collect::<Vec<_>>()
                    .join("\t"),
            );
        }
        // Include a row terminator so a final empty single-column row survives
        // interchange with applications that treat the final newline as optional.
        Ok(format!("{}\n", rows.join("\n")))
    }

    pub fn paste_tsv(&mut self, start: CellAddr, text: &str) -> Result<CellRange, ClipboardError> {
        self.paste_tsv_with_origin(start, text, None)
    }

    /// Paste a captured table as one edit. A known copy origin translates formula
    /// references; external text and cut operations pass `None` to preserve them.
    pub fn paste_tsv_with_origin(
        &mut self,
        start: CellAddr,
        text: &str,
        source: Option<CellAddr>,
    ) -> Result<CellRange, ClipboardError> {
        if let Some(source) = source {
            clipboard::validate_range(CellRange {
                start: source,
                end: source,
            })?;
        }
        let rows = clipboard::parse_tsv(text)?;
        let end = CellAddr::new(
            start
                .col
                .checked_add(rows[0].len() as u32 - 1)
                .ok_or(ClipboardError::OutOfBounds)?,
            start
                .row
                .checked_add(rows.len() as u32 - 1)
                .ok_or(ClipboardError::OutOfBounds)?,
        );
        let range = clipboard::validate_range(CellRange { start, end })?;
        self.set_cells(rows.into_iter().enumerate().flat_map(|(row, values)| {
            values.into_iter().enumerate().map(move |(col, value)| {
                (
                    CellAddr::new(start.col + col as u32, start.row + row as u32),
                    match source {
                        Some(source) => crate::translate_formula(&value, source, start),
                        None => value,
                    },
                )
            })
        }));
        Ok(range)
    }

    pub fn clear_range(&mut self, range: CellRange) {
        // Visit stored cells, not every empty cell in a large selected range.
        let cells: Vec<_> = self
            .active_sheet()
            .occupied()
            .filter(|(addr, _)| range.contains(*addr))
            .map(|(addr, _)| (addr, String::new()))
            .collect();
        self.set_cells(cells);
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn mark_clean(&mut self) {
        self.dirty = false;
        self.saved_revision = self.revision;
    }

    /// Drop demo/import history without changing the current save state.
    pub fn clear_history(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    pub fn undo(&mut self) -> bool {
        let Some(edit) = self.undo_stack.pop() else {
            return false;
        };
        self.apply(&edit.change, false);
        self.revision = edit.before_revision;
        self.dirty = self.revision != self.saved_revision;
        self.redo_stack.push(edit);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(edit) = self.redo_stack.pop() else {
            return false;
        };
        self.apply(&edit.change, true);
        self.revision = edit.after_revision;
        self.dirty = self.revision != self.saved_revision;
        self.undo_stack.push(edit);
        true
    }

    fn record(&mut self, change: Change) {
        self.next_revision += 1;
        self.undo_stack.push(Edit {
            change,
            before_revision: self.revision,
            after_revision: self.next_revision,
        });
        // Bound retained operations; cell deltas avoid whole-workbook snapshots.
        if self.undo_stack.len() > 100 {
            self.undo_stack.remove(0);
        }
        self.revision = self.next_revision;
        self.redo_stack.clear();
        self.dirty = true;
    }

    fn apply(&mut self, change: &Change, forward: bool) {
        match change {
            Change::Dimensions {
                sheet,
                axis,
                sizes,
                before_bounds,
                after_bounds,
            } => {
                self.active = *sheet;
                for &(index, before, after) in sizes {
                    self.active_sheet_mut().set_dimension(
                        *axis,
                        index,
                        if forward { after } else { before },
                    );
                }
                let (rows, cols) = if forward {
                    *after_bounds
                } else {
                    *before_bounds
                };
                self.active_sheet_mut().rows = rows;
                self.active_sheet_mut().cols = cols;
            }
            Change::Formats {
                sheet,
                cells,
                before_bounds,
                after_bounds,
            } => {
                self.active = *sheet;
                for (addr, before, after) in cells {
                    self.active_sheet_mut()
                        .set_format(*addr, if forward { after } else { before }.clone());
                }
                let (rows, cols) = if forward {
                    *after_bounds
                } else {
                    *before_bounds
                };
                self.active_sheet_mut().rows = rows;
                self.active_sheet_mut().cols = cols;
            }
            Change::Cells {
                sheet,
                cells,
                before_bounds,
                after_bounds,
            } => {
                self.active = *sheet;
                for change in cells {
                    let cell = if forward {
                        &change.after
                    } else {
                        &change.before
                    };
                    self.active_sheet_mut().set_raw(
                        change.addr,
                        cell.as_ref().map_or("", |cell| cell.raw.as_str()),
                    );
                }
                let (rows, cols) = if forward {
                    *after_bounds
                } else {
                    *before_bounds
                };
                self.active_sheet_mut().rows = rows;
                self.active_sheet_mut().cols = cols;
            }
            Change::RenameSheet {
                index,
                before,
                after,
            } => {
                self.sheets[*index].name = if forward { after } else { before }.clone();
                self.active = *index;
            }
            Change::DeleteSheet {
                sheet,
                index,
                previous_active,
                next_active,
            } => {
                if forward {
                    self.sheets.remove(*index);
                    self.active = *next_active;
                } else {
                    self.sheets.insert(*index, sheet.clone());
                    self.active = *previous_active;
                }
            }
            Change::AddSheet {
                sheet,
                index,
                previous_active,
            } => {
                if forward {
                    self.sheets.insert(*index, sheet.clone());
                    self.active = *index;
                } else {
                    self.sheets.remove(*index);
                    self.active = *previous_active;
                }
            }
        }
    }

    pub fn set_active_sheet(&mut self, index: usize) {
        if index < self.sheets.len() {
            self.active = index;
        }
    }

    pub fn sheet_count(&self) -> usize {
        self.sheets.len()
    }

    /// Generate a name that remains unique after deletions and custom renames.
    pub fn next_sheet_name(&self) -> String {
        for n in 1.. {
            let name = format!("Sheet{n}");
            if !self
                .sheets
                .iter()
                .any(|sheet| sheet.name.eq_ignore_ascii_case(&name))
            {
                return name;
            }
        }
        unreachable!()
    }

    /// Rename without changing values, formulas, or history on failure/no-op.
    pub fn rename_sheet(
        &mut self,
        index: usize,
        name: impl Into<String>,
    ) -> Result<bool, SheetError> {
        let before = self
            .sheets
            .get(index)
            .ok_or(SheetError::Missing)?
            .name
            .clone();
        let name = name.into();
        if name.trim().is_empty() {
            return Err(SheetError::InvalidName("Name cannot be blank".into()));
        }
        rust_xlsxwriter::Worksheet::new()
            .set_name(&name)
            .map_err(|error| SheetError::InvalidName(error.to_string()))?;
        if self
            .sheets
            .iter()
            .enumerate()
            .any(|(i, sheet)| i != index && sheet.name.to_lowercase() == name.to_lowercase())
        {
            return Err(SheetError::DuplicateName);
        }
        if before == name {
            return Ok(false);
        }
        self.sheets[index].name = name.clone();
        self.record(Change::RenameSheet {
            index,
            before,
            after: name,
        });
        Ok(true)
    }

    /// Retain the deleted sheet and its position in history for lossless undo.
    pub fn delete_sheet(&mut self, index: usize) -> Result<(), SheetError> {
        if index >= self.sheets.len() {
            return Err(SheetError::Missing);
        }
        if self.sheets.len() == 1 {
            return Err(SheetError::LastSheet);
        }
        let previous_active = self.active;
        let sheet = self.sheets.remove(index);
        self.active = if previous_active > index {
            previous_active - 1
        } else if previous_active == index {
            index.min(self.sheets.len() - 1)
        } else {
            previous_active
        };
        self.record(Change::DeleteSheet {
            sheet,
            index,
            previous_active,
            next_active: self.active,
        });
        Ok(())
    }

    pub fn add_sheet(&mut self, name: impl Into<String>) {
        let sheet = Sheet::new(name);
        let index = self.sheets.len();
        let previous_active = self.active;
        self.sheets.push(sheet.clone());
        self.active = self.sheets.len() - 1;
        self.record(Change::AddSheet {
            sheet,
            index,
            previous_active,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MAX_SHEET_COLS, MAX_SHEET_ROWS};

    #[test]
    fn resizing_ranges_tracks_saved_revisions_without_changing_cell_data() {
        use crate::DimensionAxis::*;
        let mut book = Workbook::new();
        book.set_cell(CellAddr::new(0, 0), "=1+2");
        book.mark_clean();
        book.clear_history();
        book.resize_range(Columns, 0, 2, Some(160.0)).unwrap();
        assert_eq!(book.active_sheet().column_width(2), 160.0);
        assert_eq!(book.active_sheet().raw(CellAddr::new(0, 0)), "=1+2");
        book.undo();
        assert!(!book.is_dirty() && !book.can_undo());
        book.redo();
        book.mark_clean();
        book.resize_range(Rows, 0, 1, Some(31.5)).unwrap();
        assert_eq!(book.active_sheet().row_height(1), 31.5);
        book.undo();
        assert!(!book.is_dirty());
        book.redo();
        book.resize_range(Rows, 0, 1, None).unwrap();
        assert_eq!(book.active_sheet().row_height(0), crate::DEFAULT_ROW_HEIGHT);
        book.undo();
        assert_eq!(book.active_sheet().row_height(0), 31.5);
    }

    #[test]
    fn invalid_or_unchanged_sizes_preserve_redo_and_clean_state() {
        use crate::DimensionAxis::*;
        let mut book = Workbook::new();
        book.resize_range(Columns, 0, 0, Some(100.0)).unwrap();
        book.undo();
        for (axis, value) in [
            (Columns, f64::NAN),
            (Columns, f64::INFINITY),
            (Columns, 0.0),
            (Columns, 1791.0),
            (Columns, 1.5),
            (Rows, 0.0),
            (Rows, 410.0),
            (Rows, f64::NEG_INFINITY),
        ] {
            assert!(book.resize_range(axis, 0, 0, Some(value)).is_err());
        }
        assert!(book.resize_range(Columns, 1, 0, Some(100.0)).is_err());
        assert!(book
            .resize_range(Columns, 0, MAX_SHEET_COLS, Some(100.0))
            .is_err());
        assert!(book
            .resize_range(Rows, 0, MAX_SHEET_ROWS, Some(25.0))
            .is_err());
        assert!(!book.resize_range(Columns, 0, 0, None).unwrap());
        assert!(!book.is_dirty() && book.can_redo() && !book.can_undo());
    }

    #[test]
    fn resized_empty_rows_restore_bounds_and_survive_sheet_deletion_undo() {
        use crate::DimensionAxis::*;
        let mut book = Workbook::new();
        let before = (book.active_sheet().rows, book.active_sheet().cols);
        book.resize_range(Rows, 100, 100, Some(36.0)).unwrap();
        assert_eq!(book.active_sheet().rows, 101);
        book.undo();
        assert_eq!((book.active_sheet().rows, book.active_sheet().cols), before);
        book.redo();
        book.add_sheet("Other");
        book.delete_sheet(0).unwrap();
        book.undo();
        assert_eq!(book.sheets[0].row_height(100), 36.0);
        assert_eq!(book.sheets[0].rows, 101);
    }

    #[test]
    fn formatting_is_one_edit_and_never_changes_values_or_formula_results() {
        let mut book = Workbook::new();
        book.paste_tsv(CellAddr::new(0, 0), "0.125\t=A1*2").unwrap();
        book.mark_clean();
        book.clear_history();
        let range = CellRange {
            start: CellAddr::new(0, 0),
            end: CellAddr::new(1, 0),
        };
        book.format_range(range, crate::FormatChange::Number("0.00%".into()))
            .unwrap();
        assert_eq!(book.active_sheet().display(range.start), "12.50%");
        assert_eq!(book.active_sheet().display(range.end), "25.00%");
        assert_eq!(
            book.active_sheet().evaluate(range.end),
            crate::Value::Number(0.25)
        );
        assert_eq!(book.copy_tsv(range).unwrap(), "0.125\t=A1*2\n");
        assert!(book.undo());
        assert!(!book.is_dirty() && !book.can_undo());
        assert_eq!(book.active_sheet().display(range.end), "0.25");
        book.redo();
        book.format_range(range, crate::FormatChange::Bold(true))
            .unwrap();
        book.set_cell(range.start, "0.5");
        assert_eq!(book.active_sheet().display(range.start), "50.00%");
        assert!(book.active_sheet().format(range.start).bold);
        book.clear_range(range);
        assert!(book.active_sheet().format(range.start).bold);
        assert!(book.active_sheet().has_formatting());
        book.undo();
        assert_eq!(book.active_sheet().display(range.start), "50.00%");
    }

    #[test]
    fn formatting_blank_cells_restores_bounds_and_survives_sheet_deletion_undo() {
        let mut book = Workbook::new();
        let before = (book.active_sheet().rows, book.active_sheet().cols);
        let addr = CellAddr::new(40, 100);
        let range = CellRange {
            start: addr,
            end: addr,
        };
        book.format_range(range, crate::FormatChange::Italic(true))
            .unwrap();
        assert_eq!(
            (book.active_sheet().rows, book.active_sheet().cols),
            (101, 41)
        );
        book.undo();
        assert_eq!((book.active_sheet().rows, book.active_sheet().cols), before);
        assert!(!book.active_sheet().has_formatting());
        book.redo();
        book.add_sheet("Other");
        book.delete_sheet(0).unwrap();
        book.undo();
        assert!(book.sheets[0].format(addr).italic);
    }

    #[test]
    fn invalid_or_unchanged_formatting_preserves_redo_and_clean_state() {
        let mut book = Workbook::new();
        let addr = CellAddr::new(0, 0);
        let range = CellRange {
            start: addr,
            end: addr,
        };
        book.format_range(range, crate::FormatChange::Bold(true))
            .unwrap();
        book.undo();
        assert!(!book
            .format_range(range, crate::FormatChange::Bold(false))
            .unwrap());
        for end in [
            CellAddr::new(MAX_SHEET_COLS, 0),
            CellAddr::new(0, MAX_SHEET_ROWS),
            CellAddr::new(1000, 1000),
        ] {
            assert!(book
                .format_range(
                    CellRange { start: addr, end },
                    crate::FormatChange::Bold(true)
                )
                .is_err());
        }
        assert!(!book.is_dirty() && book.can_redo() && !book.can_undo());
        book.redo();
        book.format_range(range, crate::FormatChange::Clear)
            .unwrap();
        assert!(!book.active_sheet().has_formatting());
        book.undo();
        assert!(book.active_sheet().format(addr).bold);
    }

    #[test]
    fn rename_validation_preserves_redo_and_saved_state() {
        let mut book = Workbook::new();
        book.add_sheet("Résumé");
        book.mark_clean();
        book.clear_history();
        book.rename_sheet(0, "売上 & <集計>").unwrap();
        book.undo();
        assert!(!book.is_dirty() && book.can_redo());
        for name in [
            "",
            "   ",
            "bad/name",
            "bad:name",
            "bad?name",
            "bad*name",
            "bad[name]",
            "bad\\name",
            "'bad",
            "bad'",
            "RÉSUMÉ",
            "abcdefghijklmnopqrstuvwxyz123456",
        ] {
            assert!(book.rename_sheet(0, name).is_err(), "{name:?}");
            assert_eq!(book.sheets[0].name, "Sheet1");
            assert!(!book.is_dirty() && book.can_redo() && !book.can_undo());
        }
        assert!(!book.rename_sheet(0, "Sheet1").unwrap());
        assert_eq!(book.rename_sheet(99, "Valid"), Err(SheetError::Missing));
        assert!(book.redo());
        assert_eq!(book.sheets[0].name, "売上 & <集計>");
        book.rename_sheet(0, "日".repeat(31)).unwrap();
        assert!(book.rename_sheet(0, "日".repeat(32)).is_err());
    }

    #[test]
    fn deletion_restores_position_contents_formulas_and_saved_revision() {
        let mut book = Workbook::new();
        book.set_cell(CellAddr::new(0, 0), "first");
        book.add_sheet("売上");
        book.set_cell(CellAddr::new(0, 80), "=1+2");
        book.add_sheet("Last");
        book.set_cell(CellAddr::new(0, 0), "last");
        book.mark_clean();
        book.clear_history();
        book.set_active_sheet(1);
        book.delete_sheet(1).unwrap();
        assert_eq!(book.active_sheet().name, "Last");
        assert!(book.is_dirty());
        assert!(book.undo());
        assert_eq!(book.active, 1);
        assert_eq!(book.active_sheet().raw(CellAddr::new(0, 80)), "=1+2");
        assert_eq!(book.active_sheet().rows, 81);
        assert!(!book.is_dirty());
        assert!(book.redo());
        assert_eq!(book.sheet_count(), 2);
        book.mark_clean();
        book.undo();
        assert!(book.is_dirty());
        book.redo();
        assert!(!book.is_dirty());
        book.delete_sheet(1).unwrap();
        assert_eq!(book.active, 0);
        assert_eq!(book.delete_sheet(0), Err(SheetError::LastSheet));
        assert_eq!(book.delete_sheet(1), Err(SheetError::Missing));
        book.undo();
        assert_eq!(book.active_sheet().raw(CellAddr::new(0, 0)), "last");
    }

    #[test]
    fn deleting_inactive_sheets_tracks_active_indices_and_prior_edits() {
        let mut book = Workbook::new();
        book.add_sheet("Sheet2");
        book.add_sheet("Sheet3");
        book.set_cell(CellAddr::new(0, 0), "third");
        book.delete_sheet(0).unwrap();
        assert_eq!(book.active, 1);
        assert_eq!(book.active_sheet().raw(CellAddr::new(0, 0)), "third");
        book.undo();
        assert_eq!(book.active, 2);
        book.undo();
        assert_eq!(book.active_sheet().raw(CellAddr::new(0, 0)), "");
        book.redo();
        book.redo();
        book.set_active_sheet(0);
        book.delete_sheet(1).unwrap();
        assert_eq!(book.active, 0);
        book.undo();
        assert_eq!(book.active, 0);
        assert_eq!(book.sheets[1].raw(CellAddr::new(0, 0)), "third");
        assert_eq!(book.next_sheet_name(), "Sheet1");
        book.add_sheet(book.next_sheet_name());
        assert_eq!(book.next_sheet_name(), "Sheet4");
    }

    #[test]
    fn paste_and_clear_are_single_undoable_operations_with_formulas() {
        let mut book = Workbook::new();
        book.set_cell(CellAddr::new(0, 0), "original");
        book.mark_clean();
        book.clear_history();
        let range = book
            .paste_tsv(CellAddr::new(0, 0), "10\t20\t=SUM(A1:B1)\r\n30\t\t40\r\n")
            .unwrap();
        assert_eq!(book.active_sheet().display(CellAddr::new(2, 0)), "30");
        assert!(book.undo());
        assert_eq!(book.active_sheet().raw(CellAddr::new(0, 0)), "original");
        assert_eq!(book.active_sheet().raw(CellAddr::new(2, 1)), "");
        assert!(!book.is_dirty() && !book.can_undo());
        assert!(book.redo());
        assert_eq!(
            book.copy_tsv(range).unwrap(),
            "10\t20\t=SUM(A1:B1)\n30\t\t40\n"
        );
        book.clear_range(range);
        assert_eq!(book.active_sheet().raw(CellAddr::new(0, 0)), "");
        assert!(book.undo());
        assert_eq!(book.active_sheet().display(CellAddr::new(2, 0)), "30");
    }

    #[test]
    fn undo_and_redo_track_saved_revision_and_branching() {
        let mut book = Workbook::new();
        let a1 = CellAddr::new(0, 0);
        book.set_cell(a1, "saved");
        book.mark_clean();
        book.set_cell(a1, "later");
        assert!(book.undo());
        assert!(!book.is_dirty());
        assert!(book.undo());
        assert!(book.is_dirty());
        assert!(book.redo());
        assert!(!book.is_dirty());
        // An unchanged edit preserves redo; a different edit starts a new branch.
        book.set_cell(a1, "saved");
        assert!(book.can_redo() && !book.is_dirty());
        book.set_cell(a1, "different");
        assert!(book.is_dirty() && !book.can_redo());
        assert!(book.undo());
        assert!(!book.is_dirty());
    }

    #[test]
    fn malformed_and_out_of_bounds_pastes_leave_data_and_history_untouched() {
        let mut book = Workbook::new();
        book.set_cell(CellAddr::new(0, 0), "original");
        book.mark_clean();
        book.clear_history();
        for (start, text) in [
            (CellAddr::new(0, 0), "\"unclosed"),
            (CellAddr::new(MAX_SHEET_COLS - 1, 0), "a\tb"),
            (CellAddr::new(0, MAX_SHEET_ROWS - 1), "a\nb"),
            (CellAddr::new(u32::MAX, 0), "a\tb"),
        ] {
            assert!(book.paste_tsv(start, text).is_err());
            assert_eq!(book.active_sheet().raw(CellAddr::new(0, 0)), "original");
            assert!(!book.is_dirty() && !book.can_undo());
        }
    }

    #[test]
    fn undo_restores_sheet_bounds_after_large_paste() {
        let mut book = Workbook::new();
        let bounds = (book.active_sheet().rows, book.active_sheet().cols);
        book.paste_tsv(CellAddr::new(50, 100), "value").unwrap();
        assert_eq!(
            (book.active_sheet().rows, book.active_sheet().cols),
            (101, 51)
        );
        book.undo();
        assert_eq!((book.active_sheet().rows, book.active_sheet().cols), bounds);
        assert!(!book.is_dirty());
        book.redo();
        assert_eq!(book.active_sheet().raw(CellAddr::new(50, 100)), "value");
    }

    #[test]
    fn copy_and_paste_preserve_quoted_text_and_clear_blank_cells() {
        let mut source = Workbook::new();
        source
            .paste_tsv(
                CellAddr::new(0, 0),
                "\"a\tb\"\t\"line 1\nline 2\"\n\"say \"\"hi\"\"\"\t",
            )
            .unwrap();
        let range = CellRange {
            start: CellAddr::new(0, 0),
            end: CellAddr::new(1, 1),
        };
        let text = source.copy_tsv(range).unwrap();
        let mut target = Workbook::new();
        target.set_cell(CellAddr::new(1, 1), "overwrite this");
        target.paste_tsv(CellAddr::new(0, 0), &text).unwrap();
        for addr in range.iter() {
            assert_eq!(
                target.active_sheet().raw(addr),
                source.active_sheet().raw(addr)
            );
        }
        assert_eq!(target.active_sheet().raw(CellAddr::new(1, 1)), "");
    }

    #[test]
    fn sheet_additions_and_edits_undo_in_the_correct_sheet() {
        let mut book = Workbook::new();
        let a1 = CellAddr::new(0, 0);
        book.set_cell(a1, "first sheet");
        book.mark_clean();
        book.clear_history();
        book.add_sheet("Second");
        book.set_cell(a1, "second sheet");
        book.set_active_sheet(0);
        book.undo();
        assert_eq!(book.active, 1);
        assert_eq!(book.active_sheet().raw(a1), "");
        book.undo();
        assert_eq!(book.sheet_count(), 1);
        assert_eq!(book.active_sheet().raw(a1), "first sheet");
        assert!(!book.is_dirty());
        book.redo();
        book.redo();
        assert_eq!(book.active_sheet().raw(a1), "second sheet");
    }

    #[test]
    fn copy_preserves_trailing_empty_single_column_rows() {
        let mut source = Workbook::new();
        source.set_cell(CellAddr::new(0, 0), "one");
        let range = CellRange {
            start: CellAddr::new(0, 0),
            end: CellAddr::new(0, 2),
        };
        let text = source.copy_tsv(range).unwrap();
        let mut target = Workbook::new();
        target.set_cell(CellAddr::new(0, 2), "must be cleared");
        assert_eq!(target.paste_tsv(CellAddr::new(0, 0), &text).unwrap(), range);
        assert_eq!(target.active_sheet().raw(CellAddr::new(0, 2)), "");
    }

    #[test]
    fn history_is_bounded_and_does_not_persist_in_saved_workbooks() {
        let mut book = Workbook::new();
        for value in 0..105 {
            book.set_cell(CellAddr::new(0, 0), value.to_string());
        }
        // The sparse tuple-key map is not a JSON file format. An empty sheet
        // still lets us verify serde excludes retained cell values in history.
        let mut empty = Workbook::new();
        empty.set_cell(CellAddr::new(0, 0), "previous value");
        empty.set_cell(CellAddr::new(0, 0), "");
        let restored: Workbook =
            serde_json::from_str(&serde_json::to_string(&empty).unwrap()).unwrap();
        assert!(!restored.can_undo() && !restored.can_redo() && !restored.is_dirty());
        for _ in 0..100 {
            assert!(book.undo());
        }
        assert!(!book.undo());
        assert_eq!(book.active_sheet().raw(CellAddr::new(0, 0)), "4");
        for _ in 0..100 {
            assert!(book.redo());
        }
        assert!(!book.redo());
        assert_eq!(book.active_sheet().raw(CellAddr::new(0, 0)), "104");
    }
}

#[cfg(test)]
mod copy_reference_tests {
    use super::*;
    use crate::{MAX_SHEET_COLS, MAX_SHEET_ROWS};

    #[test]
    fn copied_table_translates_each_formula_as_one_undoable_operation() {
        let mut book = Workbook::new();
        let source = CellAddr::new(3, 1);
        book.paste_tsv(source, "=B2*C2\t=$B$2+$B2+B$2\n=SUM(B2:C3)\ttext A1")
            .unwrap();
        book.mark_clean();
        book.clear_history();
        let text = book
            .copy_tsv(CellRange {
                start: source,
                end: CellAddr::new(4, 2),
            })
            .unwrap();
        let dest = CellAddr::new(5, 4);
        book.paste_tsv_with_origin(dest, &text, Some(source))
            .unwrap();
        assert_eq!(book.active_sheet().raw(dest), "=D5*E5");
        assert_eq!(
            book.active_sheet().raw(CellAddr::new(6, 4)),
            "=$B$2+$B5+D$2"
        );
        assert_eq!(book.active_sheet().raw(CellAddr::new(5, 5)), "=SUM(D5:E6)");
        assert_eq!(book.active_sheet().raw(CellAddr::new(6, 5)), "text A1");
        assert!(book.undo());
        assert!(!book.is_dirty() && !book.can_undo());
        assert_eq!(book.active_sheet().raw(dest), "");
        assert!(book.redo());
        assert_eq!(book.active_sheet().raw(dest), "=D5*E5");
    }

    #[test]
    fn copying_beyond_reference_bounds_propagates_ref_errors_and_can_be_undone() {
        let mut book = Workbook::new();
        let source = CellAddr::new(1, 1);
        book.set_cell(source, "=SUM(A1:B2)+$A$1");
        book.mark_clean();
        book.clear_history();
        book.paste_tsv_with_origin(CellAddr::new(0, 0), "=SUM(A1:B2)+$A$1", Some(source))
            .unwrap();
        assert_eq!(
            book.active_sheet().raw(CellAddr::new(0, 0)),
            "=SUM(#REF!)+$A$1"
        );
        assert_eq!(book.active_sheet().display(CellAddr::new(0, 0)), "#REF!");
        book.undo();
        assert_eq!(book.active_sheet().raw(CellAddr::new(0, 0)), "");
        assert!(!book.is_dirty());
    }

    #[test]
    fn invalid_copy_paste_does_not_change_the_workbook_or_history() {
        let mut book = Workbook::new();
        for (destination, text, source) in [
            (CellAddr::new(0, 0), "=A1", CellAddr::new(MAX_SHEET_COLS, 0)),
            (CellAddr::new(0, 0), "=A1", CellAddr::new(0, MAX_SHEET_ROWS)),
            (
                CellAddr::new(MAX_SHEET_COLS - 1, 0),
                "=A1\t=B1",
                CellAddr::new(0, 0),
            ),
            (CellAddr::new(0, 0), "\"unfinished", CellAddr::new(1, 1)),
        ] {
            assert!(book
                .paste_tsv_with_origin(destination, text, Some(source))
                .is_err());
            assert!(!book.is_dirty() && !book.can_undo());
            assert_eq!(book.active_sheet().occupied().count(), 0);
        }
    }
}
