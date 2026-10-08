//! Single worksheet (sparse grid).

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::addr::CellAddr;
use crate::cell::{CalcError, Cell, Value};
use crate::formula;

/// Default visible grid size (not a hard storage limit).
pub const DEFAULT_ROWS: u32 = 50;
pub const DEFAULT_COLS: u32 = 26;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sheet {
    pub name: String,
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

    fn ensure_bounds(&mut self, addr: CellAddr) {
        self.cols = self.cols.max(addr.col + 1);
        self.rows = self.rows.max(addr.row + 1);
    }

    pub fn clear(&mut self) {
        self.cells.clear();
    }

    /// Evaluate cell for display (handles formulas and literals).
    pub fn evaluate(&self, addr: CellAddr) -> Value {
        let mut visiting = HashSet::new();
        self.evaluate_at(addr, &mut visiting)
    }

    pub(crate) fn evaluate_at(&self, addr: CellAddr, visiting: &mut HashSet<CellAddr>) -> Value {
        if !visiting.insert(addr) {
            return Value::Error(CalcError::Cycle);
        }
        let value = match self.get(addr) {
            None => Value::Empty,
            Some(cell) => {
                let trimmed = cell.raw.trim();
                if let Some(text) = cell.literal_text() {
                    Value::Text(text.to_owned())
                } else if trimmed.is_empty() {
                    Value::Empty
                } else if let Some(body) = trimmed.strip_prefix('=') {
                    formula::eval_body(self, body, visiting)
                } else if let Ok(n) = trimmed.parse::<f64>() {
                    Value::Number(n)
                } else {
                    Value::Text(cell.raw.clone())
                }
            }
        };
        visiting.remove(&addr);
        value
    }

    pub fn display(&self, addr: CellAddr) -> String {
        self.evaluate(addr).display()
    }

    pub fn occupied(&self) -> impl Iterator<Item = (CellAddr, &Cell)> {
        self.cells
            .iter()
            .map(|(&(col, row), cell)| (CellAddr::new(col, row), cell))
    }
}
