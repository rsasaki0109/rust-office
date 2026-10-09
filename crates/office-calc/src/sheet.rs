//! Single worksheet (sparse grid).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::addr::CellAddr;
use crate::cell::{Cell, Value};
use crate::formula;

/// Default visible grid size (not a hard storage limit).
pub const DEFAULT_ROWS: u32 = 50;
pub const DEFAULT_COLS: u32 = 26;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sheet {
    pub name: String,
    #[serde(default)]
    pub(crate) dimensions: crate::dimensions::Dimensions,
    #[serde(default)]
    formats: HashMap<(u32, u32), crate::CellFormat>,
    #[serde(default)]
    cells: HashMap<(u32, u32), Cell>,
    #[serde(default = "default_rows")]
    pub rows: u32,
    #[serde(default = "default_cols")]
    pub cols: u32,
}

fn default_rows() -> u32 {
    DEFAULT_ROWS
}
fn default_cols() -> u32 {
    DEFAULT_COLS
}

/// JSON-safe presentation data for crash recovery, including styled blank cells.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SheetPresentation {
    formats: Vec<(u32, u32, crate::CellFormat)>,
    dimensions: crate::dimensions::Dimensions,
}

impl SheetPresentation {
    pub fn validate(&self, rows: u32, cols: u32) -> Result<(), String> {
        if rows == 0
            || rows > crate::MAX_SHEET_ROWS
            || cols == 0
            || cols > crate::MAX_SHEET_COLS
            || self.formats.len() > 100_000
        {
            return Err("Invalid presentation bounds or format count".into());
        }
        let mut seen = std::collections::HashSet::new();
        for (col, row, _) in &self.formats {
            if *col >= cols || *row >= rows || !seen.insert((*col, *row)) {
                return Err("Invalid or duplicate presentation cell".into());
            }
        }
        for (axis, limit) in [
            (crate::DimensionAxis::Rows, rows),
            (crate::DimensionAxis::Columns, cols),
        ] {
            axis.validate(self.dimensions.default_size(axis))
                .map_err(|e| e.to_string())?;
            for (index, size) in self.dimensions.iter(axis) {
                if index >= limit {
                    return Err("Presentation dimension exceeds sheet bounds".into());
                }
                axis.validate(size).map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }

    pub fn entry_count(&self) -> usize {
        self.formats.len()
            + self.dimensions.iter(crate::DimensionAxis::Rows).count()
            + self.dimensions.iter(crate::DimensionAxis::Columns).count()
    }
}

impl Sheet {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            dimensions: Default::default(),
            formats: HashMap::new(),
            cells: HashMap::new(),
            rows: DEFAULT_ROWS,
            cols: DEFAULT_COLS,
        }
    }

    pub fn presentation(&self) -> SheetPresentation {
        let mut formats: Vec<_> = self
            .formatted()
            .map(|(addr, format)| (addr.col, addr.row, format.clone()))
            .collect();
        formats.sort_by_key(|(col, row, _)| (*row, *col));
        SheetPresentation {
            formats,
            dimensions: self.dimensions.clone(),
        }
    }

    /// Restore validated presentation without creating editing history.
    pub fn restore_presentation(&mut self, presentation: SheetPresentation) -> Result<(), String> {
        presentation.validate(self.rows, self.cols)?;
        self.formats = presentation
            .formats
            .into_iter()
            .map(|(col, row, format)| ((col, row), format))
            .collect();
        self.dimensions = presentation.dimensions;
        Ok(())
    }

    pub fn get(&self, addr: CellAddr) -> Option<&Cell> {
        self.cells.get(&(addr.col, addr.row))
    }

    pub fn raw(&self, addr: CellAddr) -> &str {
        self.get(addr).map(|c| c.raw.as_str()).unwrap_or("")
    }

    pub fn set_raw(&mut self, addr: CellAddr, raw: impl Into<String>) {
        let raw = raw.into();
        if raw.trim().is_empty() {
            self.cells.remove(&(addr.col, addr.row));
        } else {
            self.ensure_bounds(addr);
            self.cells.insert((addr.col, addr.row), Cell::new(raw));
        }
    }

    pub(crate) fn ensure_bounds(&mut self, addr: CellAddr) {
        self.cols = self.cols.max(addr.col + 1);
        self.rows = self.rows.max(addr.row + 1);
    }

    pub fn clear(&mut self) {
        self.cells.clear();
        self.formats.clear();
    }

    /// Evaluate cell for display (handles formulas and literals).
    pub fn evaluate(&self, addr: CellAddr) -> Value {
        formula::evaluate_cell(self, addr)
    }

    pub fn display(&self, addr: CellAddr) -> String {
        self.format(addr).display(self.evaluate(addr))
    }

    pub fn format(&self, addr: CellAddr) -> crate::CellFormat {
        self.formats
            .get(&(addr.col, addr.row))
            .cloned()
            .unwrap_or_default()
    }

    pub(crate) fn set_format(&mut self, addr: CellAddr, format: crate::CellFormat) {
        if format == crate::CellFormat::default() {
            self.formats.remove(&(addr.col, addr.row));
        } else {
            self.ensure_bounds(addr);
            self.formats.insert((addr.col, addr.row), format);
        }
    }

    pub fn column_width(&self, index: u32) -> f64 {
        self.dimensions.get(crate::DimensionAxis::Columns, index)
    }
    pub fn row_height(&self, index: u32) -> f64 {
        self.dimensions.get(crate::DimensionAxis::Rows, index)
    }
    pub fn default_dimension(&self, axis: crate::DimensionAxis) -> f64 {
        self.dimensions.default_size(axis)
    }
    pub fn dimension_overrides(
        &self,
        axis: crate::DimensionAxis,
    ) -> impl Iterator<Item = (u32, f64)> + '_ {
        self.dimensions.iter(axis)
    }
    pub fn has_custom_dimensions(&self) -> bool {
        self.dimensions.is_custom()
    }
    pub(crate) fn set_dimension(&mut self, axis: crate::DimensionAxis, index: u32, size: f64) {
        self.dimensions.set(axis, index, size);
        if size != self.dimensions.default_size(axis) {
            match axis {
                crate::DimensionAxis::Columns => self.cols = self.cols.max(index + 1),
                crate::DimensionAxis::Rows => self.rows = self.rows.max(index + 1),
            }
        }
    }

    pub fn has_formatting(&self) -> bool {
        !self.formats.is_empty()
    }

    pub(crate) fn formatted(&self) -> impl Iterator<Item = (CellAddr, &crate::CellFormat)> {
        self.formats
            .iter()
            .map(|(&(col, row), format)| (CellAddr::new(col, row), format))
    }

    pub fn occupied(&self) -> impl Iterator<Item = (CellAddr, &Cell)> {
        self.cells
            .iter()
            .map(|(&(col, row), cell)| (CellAddr::new(col, row), cell))
    }
}
