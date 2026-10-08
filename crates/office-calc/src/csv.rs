//! CSV import / export for the active sheet.

use std::path::Path;

use thiserror::Error;

use crate::addr::CellAddr;
use crate::clipboard::{MAX_SHEET_COLS, MAX_SHEET_ROWS};
use crate::delimited::{self, Format};
use crate::sheet::Sheet;
use crate::workbook::Workbook;

#[derive(Debug, Error)]
pub enum CsvError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("CSV read error: {0}")]
    Parse(String),
    #[error("CSV exceeds XLSX worksheet limits ({MAX_SHEET_ROWS} rows, {MAX_SHEET_COLS} columns)")]
    OutOfBounds,
}

/// Load a workbook from a CSV file (one sheet).
pub fn load_csv_path(path: &Path) -> Result<Workbook, CsvError> {
    let text = std::fs::read_to_string(path)?;
    load_csv_str(&text)
}

/// Read comma-separated records, including quoted multiline fields.
/// Quotes must enclose the complete field; errors reject the entire import.
pub fn load_csv_str(text: &str) -> Result<Workbook, CsvError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let rows = delimited::parse_records(
        text,
        Format {
            delimiter: ',',
            max_fields: u64::MAX,
            allow_unquoted_quotes: false,
        },
    )
    .map_err(|error| CsvError::Parse(error.to_string()))?;
    let mut sheet = Sheet::new("Sheet1");
    for (r, fields) in rows.into_iter().enumerate() {
        if r >= MAX_SHEET_ROWS as usize || fields.len() > MAX_SHEET_COLS as usize {
            return Err(CsvError::OutOfBounds);
        }
        for (c, field) in fields.into_iter().enumerate() {
            if !field.is_empty() {
                // CSV values retain literal apostrophes and whitespace-only text.
                let input = if field.starts_with('\'') || field.trim().is_empty() {
                    format!("'{field}")
                } else {
                    field
                };
                sheet.set_raw(CellAddr::new(c as u32, r as u32), input);
            }
        }
    }
    let mut wb = Workbook::new();
    wb.sheets = vec![sheet];
    wb.active = 0;
    wb.mark_clean();
    Ok(wb)
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
            let raw = sheet
                .get(CellAddr::new(c, r))
                .map_or("", |cell| cell.literal_text().unwrap_or(&cell.raw));
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
        let restored = load_csv_str(&csv).unwrap();
        assert_eq!(restored.active_sheet().raw(CellAddr::new(0, 0)), "Name");
        assert_eq!(restored.active_sheet().raw(CellAddr::new(1, 1)), "3");
        assert_eq!(
            restored.active_sheet().raw(CellAddr::new(0, 2)),
            "Hello, world"
        );
    }

    #[test]
    fn csv_exports_literal_text_without_the_input_marker() {
        let mut book = Workbook::new();
        for (col, input) in ["'00123", "'=A1", "''quoted", "'hello, world"]
            .iter()
            .enumerate()
        {
            book.set_cell(CellAddr::new(col as u32, 0), *input);
        }
        assert_eq!(
            sheet_to_csv(book.active_sheet()),
            "00123,=A1,'quoted,\"hello, world\"\n"
        );
    }

    #[test]
    fn csv_apostrophes_belong_to_the_data_and_survive_round_trip() {
        let csv = "'quoted,''two,plain\n";
        let book = load_csv_str(csv).unwrap();
        assert_eq!(book.active_sheet().display(CellAddr::new(0, 0)), "'quoted");
        assert_eq!(book.active_sheet().display(CellAddr::new(1, 0)), "''two");
        assert_eq!(sheet_to_csv(book.active_sheet()), csv);
    }

    #[test]
    fn csv_round_trip_preserves_multiline_cells_quotes_and_formulas() {
        let mut book = Workbook::new();
        let values = [
            (CellAddr::new(0, 0), "日本語\nsecond line"),
            (CellAddr::new(1, 0), "say \"hello\", then\nbye"),
            (CellAddr::new(2, 0), "1"),
            (CellAddr::new(0, 1), "CRLF\r\ninside\rreturn"),
            (CellAddr::new(3, 1), "=IF(1,\"日本語\",\"none\")"),
            (CellAddr::new(2, 2), "=C1+1"),
        ];
        for (address, value) in values {
            book.set_cell(address, value);
        }
        let restored = load_csv_str(&sheet_to_csv(book.active_sheet())).unwrap();
        for (address, value) in values {
            assert_eq!(restored.active_sheet().raw(address), value);
        }
        assert_eq!(restored.active_sheet().display(CellAddr::new(2, 2)), "2");
        assert!(!restored.is_dirty() && !restored.can_undo());
    }

    #[test]
    fn windows_csv_preserves_blank_records_ragged_rows_and_embedded_crlf() {
        let csv = "Name,Note,Qty\r\nりんご,\"first\r\nsecond\",3\r\n\r\nみかん\r\n,\"say \"\"hi\"\"\",5,\r\n";
        let book = load_csv_str(csv).unwrap();
        assert_eq!(
            book.active_sheet().raw(CellAddr::new(1, 1)),
            "first\r\nsecond"
        );
        assert_eq!(book.active_sheet().raw(CellAddr::new(2, 1)), "3");
        assert_eq!(book.active_sheet().raw(CellAddr::new(0, 2)), "");
        assert_eq!(book.active_sheet().raw(CellAddr::new(0, 3)), "みかん");
        assert_eq!(book.active_sheet().raw(CellAddr::new(1, 4)), "say \"hi\"");
        assert_eq!(book.active_sheet().raw(CellAddr::new(2, 4)), "5");
        assert_eq!(book.active_sheet().raw(CellAddr::new(3, 4)), "");
    }

    #[test]
    fn utf8_bom_is_not_part_of_the_first_header_or_quoted_cell() {
        for csv in [
            "\u{feff}Name,Qty\r\nりんご,3\r\n",
            "\u{feff}\"Name\",Qty\nりんご,3\n",
        ] {
            let book = load_csv_str(csv).unwrap();
            assert_eq!(book.active_sheet().raw(CellAddr::new(0, 0)), "Name");
            assert_eq!(book.active_sheet().raw(CellAddr::new(0, 1)), "りんご");
        }
        let book = load_csv_str("Name\n\u{feff}data\n").unwrap();
        assert_eq!(book.active_sheet().raw(CellAddr::new(0, 1)), "\u{feff}data");
    }

    #[test]
    fn malformed_csv_reports_the_logical_record_and_field() {
        for csv in [
            "Name,Note\nitem,\"first\nsecond",
            "Name,Note\nitem,\"closed\"extra\n",
            "Name,Note\nitem,unquoted\"quote\n",
        ] {
            let error = load_csv_str(csv).expect_err("malformed CSV must not return partial data");
            assert!(matches!(error, CsvError::Parse(_)));
            assert!(error.to_string().contains("record 2, field 2"));
        }
    }

    #[test]
    fn whitespace_only_values_are_preserved_as_text() {
        let csv = "Name,Note\nitem,\" \t \"\n";
        let book = load_csv_str(csv).unwrap();
        assert_eq!(book.active_sheet().display(CellAddr::new(1, 1)), " \t ");
        let restored = load_csv_str(&sheet_to_csv(book.active_sheet())).unwrap();
        assert_eq!(restored.active_sheet().display(CellAddr::new(1, 1)), " \t ");
    }

    #[test]
    fn csv_columns_are_limited_to_the_xlsx_grid() {
        let valid = format!("{}last", ",".repeat((MAX_SHEET_COLS - 1) as usize));
        let book = load_csv_str(&valid).unwrap();
        assert_eq!(
            book.active_sheet()
                .raw(CellAddr::new(MAX_SHEET_COLS - 1, 0)),
            "last"
        );
        let invalid = format!("{valid},extra");
        assert!(matches!(load_csv_str(&invalid), Err(CsvError::OutOfBounds)));
    }

    #[test]
    fn csv_rows_are_limited_to_logical_records() {
        let valid = format!("{}last", "\n".repeat((MAX_SHEET_ROWS - 1) as usize));
        let book = load_csv_str(&valid).unwrap();
        assert_eq!(
            book.active_sheet()
                .raw(CellAddr::new(0, MAX_SHEET_ROWS - 1)),
            "last"
        );
        let invalid = format!("{valid}\nextra");
        assert!(matches!(load_csv_str(&invalid), Err(CsvError::OutOfBounds)));
    }

    #[test]
    fn csv_file_io_preserves_multiline_values_and_propagates_parse_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("multiline.csv");
        let mut book = Workbook::new();
        book.set_cell(CellAddr::new(0, 0), "日本語\nsecond line");
        write_csv_path(&book, &path).unwrap();
        let restored = load_csv_path(&path).unwrap();
        assert_eq!(
            restored.active_sheet().raw(CellAddr::new(0, 0)),
            "日本語\nsecond line"
        );
        std::fs::write(&path, "Good,1\n\"unfinished").unwrap();
        assert!(matches!(load_csv_path(&path), Err(CsvError::Parse(_))));
    }
}
