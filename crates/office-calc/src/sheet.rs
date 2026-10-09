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

impl Sheet {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            formats: HashMap::new(),
            cells: HashMap::new(),
            rows: DEFAULT_ROWS,
            cols: DEFAULT_COLS,
        }
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
