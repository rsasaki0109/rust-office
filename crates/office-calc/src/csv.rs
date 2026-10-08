//! CSV import / export for the active sheet.

use std::path::Path;

use thiserror::Error;

use crate::addr::CellAddr;
use crate::sheet::Sheet;
use crate::workbook::Workbook;

#[derive(Debug, Error)]
pub enum CsvError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Load a workbook from a CSV file (one sheet).
pub fn load_csv_path(path: &Path) -> Result<Workbook, CsvError> {
    let text = std::fs::read_to_string(path)?;
    Ok(load_csv_str(&text))
}

pub fn load_csv_str(text: &str) -> Workbook {
    let mut sheet = Sheet::new("Sheet1");
    for (r, line) in text.lines().enumerate() {
        for (c, field) in parse_csv_line(line).into_iter().enumerate() {
            if !field.is_empty() {
                sheet.set_raw(CellAddr::new(c as u32, r as u32), field);
            }
        }
    }
    let mut wb = Workbook::new();
    wb.sheets = vec![sheet];
    wb.active = 0;
    wb.mark_clean();
    wb
}

pub fn write_csv_path(workbook: &Workbook, path: &Path) -> Result<(), CsvError> {
    let text = sheet_to_csv(workbook.active_sheet());
    office_core::storage::atomic_write(path, text.as_bytes())?;
    Ok(())
}

pub fn sheet_to_csv(sheet: &Sheet) -> String {
    let mut max_r = 0u32;
    let mut max_c = 0u32;
    let mut any = false;
    for (addr, _) in sheet.occupied() {
        any = true;
        max_r = max_r.max(addr.row);
        max_c = max_c.max(addr.col);
    }
    if !any {
        return String::new();
    }
    let mut lines = Vec::new();
    for r in 0..=max_r {
        let mut fields = Vec::new();
        for c in 0..=max_c {
            let raw = sheet.raw(CellAddr::new(c, r));
            fields.push(escape_csv_field(raw));
        }
        while fields.last().is_some_and(|f| f.is_empty()) {
            fields.pop();
        }
        lines.push(fields.join(","));
    }
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    let mut out = lines.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

fn parse_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut cur = String::new();
    let mut chars = line.chars().peekable();
    let mut in_quotes = false;
    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    cur.push('"');
                } else {
                    in_quotes = false;
                }
            } else {
                cur.push(c);
            }
        } else if c == '"' {
            in_quotes = true;
        } else if c == ',' {
            fields.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    fields.push(cur);
    fields
}

fn escape_csv_field(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addr::CellAddr;

    #[test]
    fn csv_round_trip() {
        let mut wb = Workbook::new();
        wb.set_cell(CellAddr::new(0, 0), "Name");
        wb.set_cell(CellAddr::new(1, 0), "Qty");
        wb.set_cell(CellAddr::new(0, 1), "Apples");
        wb.set_cell(CellAddr::new(1, 1), "3");
        wb.set_cell(CellAddr::new(0, 2), "Hello, world");
        let csv = sheet_to_csv(wb.active_sheet());
        let restored = load_csv_str(&csv);
        assert_eq!(restored.active_sheet().raw(CellAddr::new(0, 0)), "Name");
        assert_eq!(restored.active_sheet().raw(CellAddr::new(1, 1)), "3");
        assert_eq!(
            restored.active_sheet().raw(CellAddr::new(0, 2)),
            "Hello, world"
        );
    }
}
