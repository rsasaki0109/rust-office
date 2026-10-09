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
    #[serde(default)]
    presentation: Option<office_calc::SheetPresentation>,
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
                    presentation: Some(sheet.presentation()),
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
                if let Some(presentation) = snapshot.presentation {
                    sheet
                        .restore_presentation(presentation)
                        .expect("recovery data must be validated before restoration");
                }
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
            if let Some(presentation) = &sheet.presentation {
                presentation.validate(sheet.rows, sheet.cols)?;
                total = total.saturating_add(presentation.entry_count());
                if total > 100_000 {
                    return Err("Calc recovery exceeds presentation limits".into());
                }
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
    fn recovery_preserves_formats_blank_cells_dimensions_and_xlsx_export() {
        use office_calc::{CellRange, DimensionAxis, FormatChange};
        let mut book = Workbook::new();
        let cell = CellAddr::new(0, 0);
        let blank = CellAddr::new(2, 3);
        book.set_cell(cell, "0.125");
        book.format_range(
            CellRange {
                start: cell,
                end: cell,
            },
            FormatChange::Number("0.00%".into()),
        )
        .unwrap();
        book.format_range(
            CellRange {
                start: blank,
                end: blank,
            },
            FormatChange::Bold(true),
        )
        .unwrap();
        book.resize_range(DimensionAxis::Columns, 0, 0, Some(140.0))
            .unwrap();
        book.resize_range(DimensionAxis::Rows, 3, 3, Some(32.5))
            .unwrap();
        // Imported worksheet defaults must survive along with sparse overrides.
        let mut presentation = serde_json::to_value(book.active_sheet().presentation()).unwrap();
        presentation["dimensions"]["default_column"] = 120.0.into();
        presentation["dimensions"]["default_row"] = 23.5.into();
        book.active_sheet_mut()
            .restore_presentation(serde_json::from_value(presentation).unwrap())
            .unwrap();
        book.add_sheet("Second");
        book.active = 0;
        let snapshot = CalcSnapshot::capture(&book, Some((cell, "0.25")));
        let decoded = CalcSnapshot::decode(&serde_json::to_vec(&snapshot).unwrap()).unwrap();
        decoded.validate().unwrap();
        let restored = decoded.into_workbook();
        let sheet = restored.active_sheet();
        assert_eq!(sheet.display(cell), "25.00%");
        assert_eq!(sheet.raw(blank), "");
        assert!(sheet.format(blank).bold);
        assert_eq!(sheet.column_width(0), 140.0);
        assert_eq!(sheet.row_height(3), 32.5);
        assert_eq!(sheet.column_width(1), 120.0);
        assert_eq!(sheet.row_height(1), 23.5);
        assert_eq!(restored.sheets.len(), 2);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("restored.xlsx");
        office_calc::write_xlsx_path(&restored, &path).unwrap();
        let reopened = office_calc::load_xlsx_path(&path).unwrap();
        assert_eq!(reopened.sheets[0].format(cell), sheet.format(cell));
        assert_eq!(reopened.sheets[0].format(blank), sheet.format(blank));
        assert_eq!(reopened.sheets[0].column_width(0), 140.0);
        assert_eq!(reopened.sheets[0].row_height(3), 32.5);
        assert_eq!(reopened.sheets[0].column_width(1), 120.0);
        assert_eq!(reopened.sheets[0].row_height(1), 23.5);
    }

    #[test]
    fn legacy_snapshot_without_presentation_still_restores() {
        let mut value =
            serde_json::to_value(CalcSnapshot::capture(&Workbook::new(), None)).unwrap();
        value["sheets"][0]
            .as_object_mut()
            .unwrap()
            .remove("presentation");
        let snapshot = CalcSnapshot::decode(&serde_json::to_vec(&value).unwrap()).unwrap();
        snapshot.validate().unwrap();
        assert!(!snapshot.into_workbook().active_sheet().has_formatting());
    }

    #[test]
    fn invalid_recovery_presentation_is_rejected() {
        let value = serde_json::to_value(CalcSnapshot::capture(&Workbook::new(), None)).unwrap();
        for mutation in ["address", "duplicate", "width", "height", "row", "unknown"] {
            let mut bad = value.clone();
            let p = &mut bad["sheets"][0]["presentation"];
            match mutation {
                "address" => {
                    p["formats"] =
                        serde_json::json!([[16384, 0, office_calc::CellFormat::default()]])
                }
                "duplicate" => {
                    p["formats"] = serde_json::json!([
                        [0, 0, office_calc::CellFormat::default()],
                        [0, 0, office_calc::CellFormat::default()]
                    ])
                }
                "width" => p["dimensions"]["default_column"] = (-1).into(),
                "height" => p["dimensions"]["default_row"] = 1000.into(),
                "row" => p["dimensions"]["rows"] = serde_json::json!({"1048576": 32.0}),
                _ => p["dimensions"]["unknown"] = true.into(),
            }
            assert!(
                CalcSnapshot::decode(&serde_json::to_vec(&bad).unwrap())
                    .and_then(|s| s.validate())
                    .is_err(),
                "{mutation}"
            );
        }
    }

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
