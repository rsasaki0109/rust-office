//! Sparse worksheet sizes, in pixels and points.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const DEFAULT_COLUMN_WIDTH: f64 = 88.0;
pub const DEFAULT_ROW_HEIGHT: f64 = 18.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DimensionAxis {
    Columns,
    Rows,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DimensionError {
    #[error("The row/column range exceeds XLSX worksheet limits")]
    OutOfBounds,
    #[error("Column width must be finite and an integer between 1 and 1790 pixels")]
    ColumnWidth,
    #[error("Row height must be finite and between 1 and 409.5 points")]
    RowHeight,
}

impl DimensionAxis {
    pub(crate) fn validate(self, size: f64) -> Result<(), DimensionError> {
        match self {
            Self::Columns
                if size.is_finite() && (1.0..=1790.0).contains(&size) && size.fract() == 0.0 =>
            {
                Ok(())
            }
            Self::Rows if size.is_finite() && (1.0..=409.5).contains(&size) => Ok(()),
            Self::Columns => Err(DimensionError::ColumnWidth),
            Self::Rows => Err(DimensionError::RowHeight),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Dimensions {
    pub(crate) default_column: f64,
    pub(crate) default_row: f64,
    columns: BTreeMap<u32, f64>,
    rows: BTreeMap<u32, f64>,
}
impl Default for Dimensions {
    fn default() -> Self {
        Self {
            default_column: DEFAULT_COLUMN_WIDTH,
            default_row: DEFAULT_ROW_HEIGHT,
            columns: BTreeMap::new(),
            rows: BTreeMap::new(),
        }
    }
}
impl Dimensions {
    pub(crate) fn default_size(&self, axis: DimensionAxis) -> f64 {
        match axis {
            DimensionAxis::Columns => self.default_column,
            DimensionAxis::Rows => self.default_row,
        }
    }
    fn sizes(&self, axis: DimensionAxis) -> &BTreeMap<u32, f64> {
        match axis {
            DimensionAxis::Columns => &self.columns,
            DimensionAxis::Rows => &self.rows,
        }
    }
    pub(crate) fn get(&self, axis: DimensionAxis, index: u32) -> f64 {
        self.sizes(axis)
            .get(&index)
            .copied()
            .unwrap_or(self.default_size(axis))
    }
    pub(crate) fn set(&mut self, axis: DimensionAxis, index: u32, value: f64) {
        let default = self.default_size(axis);
        let sizes = match axis {
            DimensionAxis::Columns => &mut self.columns,
            DimensionAxis::Rows => &mut self.rows,
        };
        if value == default {
            sizes.remove(&index);
        } else {
            sizes.insert(index, value);
        }
    }
    pub(crate) fn iter(&self, axis: DimensionAxis) -> impl Iterator<Item = (u32, f64)> + '_ {
        self.sizes(axis).iter().map(|(&index, &size)| (index, size))
    }
    pub(crate) fn is_custom(&self) -> bool {
        !self.rows.is_empty()
            || !self.columns.is_empty()
            || self.default_column != DEFAULT_COLUMN_WIDTH
            || self.default_row != DEFAULT_ROW_HEIGHT
    }
}
