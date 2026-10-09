//! Sparse prefix offsets: no widget or offset array per empty row/cell.
use office_calc::{DimensionAxis, Sheet};

pub(crate) struct GridAxis {
    count: u32,
    default: f64,
    // Cumulative deviation immediately after each overridden row/column.
    corrections: Vec<(u32, f64)>,
}
impl GridAxis {
    pub(crate) fn new(sheet: &Sheet, axis: DimensionAxis) -> Self {
        let scale = if axis == DimensionAxis::Rows {
            4.0 / 3.0
        } else {
            1.0
        };
        let default = sheet.default_dimension(axis) * scale;
        let count = if axis == DimensionAxis::Rows {
            sheet.rows
        } else {
            sheet.cols
        };
        let mut delta = 0.0;
        let corrections = sheet
            .dimension_overrides(axis)
            .filter(|(index, _)| *index < count)
            .map(|(index, size)| {
                delta += size * scale - default;
                (index, delta)
            })
            .collect();
        Self {
            count,
            default,
            corrections,
        }
    }
    pub(crate) fn position(&self, index: u32) -> f64 {
        let index = index.min(self.count);
        let before = self.corrections.partition_point(|(row, _)| *row < index);
        index as f64 * self.default
            + if before == 0 {
                0.0
            } else {
                self.corrections[before - 1].1
            }
    }
    pub(crate) fn size(&self, index: u32) -> f32 {
        (self.position(index + 1) - self.position(index)) as f32
    }
    pub(crate) fn total(&self) -> f32 {
        self.position(self.count) as f32
    }
    pub(crate) fn index_at(&self, offset: f64) -> u32 {
        if offset.is_nan() || offset <= 0.0 {
            return 0;
        }
        let mut first = 0;
        let mut last = self.count;
        while first < last {
            let middle = first + (last - first) / 2;
            if self.position(middle + 1) <= offset {
                first = middle + 1;
            } else {
                last = middle;
            }
        }
        first.min(self.count.saturating_sub(1))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use office_calc::{Workbook, MAX_SHEET_ROWS};
    #[test]
    fn variable_geometry_hits_boundaries_and_large_sparse_rows() {
        let mut book = Workbook::new();
        book.resize_range(DimensionAxis::Columns, 0, 0, Some(160.0))
            .unwrap();
        book.resize_range(DimensionAxis::Columns, 2, 2, Some(40.0))
            .unwrap();
        let cols = GridAxis::new(book.active_sheet(), DimensionAxis::Columns);
        for (pos, index) in [
            (-1.0, 0),
            (159.9, 0),
            (160.0, 1),
            (247.9, 1),
            (248.0, 2),
            (288.0, 3),
        ] {
            assert_eq!(cols.index_at(pos), index);
        }
        assert_eq!(cols.size(0), 160.0);
        book.resize_range(
            DimensionAxis::Rows,
            MAX_SHEET_ROWS - 1,
            MAX_SHEET_ROWS - 1,
            Some(36.0),
        )
        .unwrap();
        let rows = GridAxis::new(book.active_sheet(), DimensionAxis::Rows);
        assert_eq!(rows.corrections.len(), 1);
        assert_eq!(
            rows.index_at(rows.position(MAX_SHEET_ROWS - 1)),
            MAX_SHEET_ROWS - 1
        );
        assert_eq!(rows.size(MAX_SHEET_ROWS - 1), 48.0);
        assert_eq!(rows.index_at(f64::INFINITY), MAX_SHEET_ROWS - 1);
    }
}
