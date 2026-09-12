//! Workbook container and dirty tracking.

use serde::{Deserialize, Serialize};

use crate::addr::CellAddr;
use crate::sheet::Sheet;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workbook {
    pub sheets: Vec<Sheet>,
    pub active: usize,
    #[serde(skip)]
    dirty: bool,
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
        }
    }

    pub fn active_sheet(&self) -> &Sheet {
        &self.sheets[self.active]
    }

    pub fn active_sheet_mut(&mut self) -> &mut Sheet {
        &mut self.sheets[self.active]
    }

    pub fn set_cell(&mut self, addr: CellAddr, raw: impl Into<String>) {
        self.active_sheet_mut().set_raw(addr, raw);
        self.dirty = true;
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn mark_clean(&mut self) {
        self.dirty = false;
    }

    pub fn set_active_sheet(&mut self, index: usize) {
        if index < self.sheets.len() {
            self.active = index;
        }
    }

    pub fn sheet_count(&self) -> usize {
        self.sheets.len()
    }

    pub fn add_sheet(&mut self, name: impl Into<String>) {
        self.sheets.push(Sheet::new(name));
        self.active = self.sheets.len() - 1;
        self.dirty = true;
    }
}
