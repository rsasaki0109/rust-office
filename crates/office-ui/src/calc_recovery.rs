//! Versioned sparse-cell recovery format; tuple-keyed sheet maps cannot be JSON objects.
use office_calc::{CellAddr, Workbook};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CalcSnapshot {
    version: u32,
    active: usize,
    sheets: Vec<SheetSnapshot>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SheetSnapshot {
    name: String,
    rows: u32,
    cols: u32,
    cells: Vec<(u32, u32, String)>,
}

impl CalcSnapshot {
    pub fn capture(book: &Workbook, draft: Option<(CellAddr, &str)>) -> Self {
        let sheets = book
            .sheets
            .iter()
            .enumerate()
            .map(|(index, sheet)| {
                let mut cells: Vec<_> = sheet
                    .occupied()
                    .map(|(a, c)| (a.col, a.row, c.raw.clone()))
                    .collect();
                let mut rows = sheet.rows;
                let mut cols = sheet.cols;
                if index == book.active {
                    if let Some((addr, text)) = draft {
                        cells.retain(|(col, row, _)| *col != addr.col || *row != addr.row);
                        if !text.trim().is_empty() {
                            cells.push((addr.col, addr.row, text.into()));
                            rows = rows.max(addr.row.saturating_add(1));
                            cols = cols.max(addr.col.saturating_add(1));
                        }
                    }
                }
                cells.sort_by_key(|(col, row, _)| (*row, *col));
                SheetSnapshot {
                    name: sheet.name.clone(),
                    rows,
                    cols,
                    cells,
                }
            })
            .collect();
        Self {
            version: 1,
            active: book.active,
            sheets,
        }
    }
    pub fn into_workbook(self) -> Workbook {
        let mut book = Workbook::new();
        book.sheets = self
            .sheets
            .into_iter()
            .map(|snapshot| {
                let mut sheet = office_calc::Sheet::new(snapshot.name);
                for (col, row, raw) in snapshot.cells {
                    sheet.set_raw(CellAddr::new(col, row), raw);
                }
                sheet.rows = snapshot.rows;
                sheet.cols = snapshot.cols;
                sheet
            })
            .collect();
        book.active = self.active;
        book.mark_recovered();
        book
    }
}
impl crate::recovery::RecoveryData for CalcSnapshot {
    const PREFIX: &'static str = "calc-";
    fn validate(&self) -> Result<(), String> {
        if self.version != 1
            || self.sheets.is_empty()
            || self.sheets.len() > 1000
            || self.active >= self.sheets.len()
        {
            return Err("Invalid Calc recovery version, sheet count or active sheet".into());
        }
        let mut total = 0usize;
        for sheet in &self.sheets {
            total = total.saturating_add(sheet.cells.len());
            if total > 100_000
                || sheet.rows == 0
                || sheet.rows > 1_048_576
                || sheet.cols == 0
                || sheet.cols > 16_384
            {
                return Err("Calc recovery exceeds cell or grid limits".into());
            }
            let mut seen = std::collections::HashSet::new();
            for (col, row, _) in &sheet.cells {
                if *col >= sheet.cols || *row >= sheet.rows || !seen.insert((*col, *row)) {
                    return Err("Invalid or duplicate Calc recovery cell address".into());
                }
            }
        }
        Ok(())
    }
    fn decode(bytes: &[u8]) -> Result<Self, String> {
        serde_json::from_slice(bytes).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recovery::{Recovery, RecoveryData};
    #[test]
    fn sparse_multisheet_formula_and_draft_survive_two_restarts() {
        let root = tempfile::tempdir().unwrap();
        let mut book = Workbook::new();
        book.set_cell(CellAddr::new(0, 0), "日本語");
        book.set_cell(CellAddr::new(1, 0), "'001");
        book.add_sheet("集計");
        book.active = 1;
        book.set_cell(CellAddr::new(0, 0), "2");
        book.set_cell(CellAddr::new(1, 0), "=A1*3");
        let snapshot = CalcSnapshot::capture(&book, Some((CellAddr::new(0, 0), "4")));
        let mut first = Recovery::new(root.path()).unwrap();
        first
            .tick(&snapshot, true, "source.xlsx", std::time::Instant::now())
            .unwrap();
        let second = Recovery::<CalcSnapshot>::new(root.path()).unwrap();
        assert!(second.entries.is_empty());
        drop(first);
        drop(second);
        let mut second = Recovery::<CalcSnapshot>::new(root.path()).unwrap();
        let restored = second.restore(0).unwrap().into_workbook();
        assert_eq!(restored.sheets[0].raw(CellAddr::new(0, 0)), "日本語");
        assert_eq!(restored.sheets[0].raw(CellAddr::new(1, 0)), "'001");
        assert_eq!(restored.active, 1);
        assert_eq!(restored.active_sheet().name, "集計");
        assert_eq!(restored.active_sheet().display(CellAddr::new(1, 0)), "12");
        assert!(restored.is_dirty() && !restored.can_undo());
        assert_eq!(book.active_sheet().raw(CellAddr::new(0, 0)), "2");
        drop(second);
        let mut third = Recovery::<CalcSnapshot>::new(root.path()).unwrap();
        assert_eq!(third.restore(0).unwrap().into_workbook().active, 1);
        third.clear().unwrap();
    }
    #[test]
    fn malformed_models_are_rejected_before_workbook_construction() {
        let valid = CalcSnapshot::capture(&Workbook::new(), None);
        let value = serde_json::to_value(&valid).unwrap();
        for mutation in [
            "active",
            "empty",
            "version",
            "bounds",
            "duplicate",
            "unknown",
        ] {
            let mut bad = value.clone();
            match mutation {
                "active" => bad["active"] = 9.into(),
                "empty" => bad["sheets"] = serde_json::json!([]),
                "version" => bad["version"] = 2.into(),
                "bounds" => bad["sheets"][0]["rows"] = 0.into(),
                "duplicate" => {
                    bad["sheets"][0]["cells"] = serde_json::json!([[0, 0, "1"], [0, 0, "2"]])
                }
                _ => bad["future_field"] = true.into(),
            }
            let decoded = CalcSnapshot::decode(&serde_json::to_vec(&bad).unwrap());
            assert!(decoded.and_then(|v| v.validate()).is_err(), "{mutation}");
        }
    }
    #[test]
    fn undo_to_recovered_baseline_stays_dirty_until_explicit_save() {
        let mut book = CalcSnapshot::capture(&Workbook::new(), None).into_workbook();
        book.set_cell(CellAddr::new(0, 0), "new");
        assert!(book.undo());
        assert!(book.is_dirty());
        book.mark_clean();
        assert!(!book.is_dirty());
        assert!(book.redo());
        assert!(book.is_dirty());
        assert!(book.undo());
        assert!(!book.is_dirty());
    }
}
