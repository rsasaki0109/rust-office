//! XLSX import / export via calamine + rust_xlsxwriter.

use std::path::Path;

use calamine::{open_workbook_auto, Data, Reader};
use rust_xlsxwriter::Workbook as XlsxWriter;
use thiserror::Error;

use crate::addr::CellAddr;
use crate::sheet::Sheet;
use crate::workbook::Workbook;

#[derive(Debug, Error)]
pub enum XlsxError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("xlsx read error: {0}")]
    Read(String),
    #[error("xlsx write error: {0}")]
    Write(String),
}

/// Load a workbook from an `.xlsx` / `.xlsm` / `.xls` / `.ods` path.
pub fn load_xlsx_path(path: &Path) -> Result<Workbook, XlsxError> {
    let mut excel = open_workbook_auto(path).map_err(|e| XlsxError::Read(e.to_string()))?;
    let names = excel.sheet_names().to_vec();
    if names.is_empty() {
        return Ok(Workbook::new());
    }

    let mut sheets = Vec::new();
    for name in names {
        let mut sheet = Sheet::new(name.clone());
        if let Ok(range) = excel.worksheet_range(&name) {
            let (sr, sc) = range.start().unwrap_or((0, 0));
            for (r, c, value) in range.used_cells() {
                let raw = data_to_raw(value);
                if !raw.is_empty() {
                    sheet.set_raw(
                        CellAddr::new(sc + c as u32, sr + r as u32),
                        raw,
                    );
                }
            }
        }
        if let Ok(formulas) = excel.worksheet_formula(&name) {
            let (sr, sc) = formulas.start().unwrap_or((0, 0));
            for (r, c, formula) in formulas.used_cells() {
                let f = formula.trim();
                if !f.is_empty() {
                    let text = if f.starts_with('=') {
                        f.to_string()
                    } else {
                        format!("={f}")
                    };
                    sheet.set_raw(
                        CellAddr::new(sc + c as u32, sr + r as u32),
                        text,
                    );
                }
            }
        }
        sheets.push(sheet);
    }

    let mut wb = Workbook::new();
    wb.sheets = sheets;
    wb.active = 0;
    wb.mark_clean();
    Ok(wb)
}

fn data_to_raw(value: &Data) -> String {
    match value {
        Data::Empty => String::new(),
        Data::String(s) => s.clone(),
        Data::Float(f) => format_number(*f),
        Data::Int(i) => i.to_string(),
        Data::Bool(b) => {
            if *b {
                "TRUE".into()
            } else {
                "FALSE".into()
            }
        }
        Data::Error(e) => format!("{e:?}"),
        Data::DateTime(dt) => dt.to_string(),
        Data::DateTimeIso(s) | Data::DurationIso(s) => s.clone(),
    }
}

fn format_number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{n:.0}")
    } else {
        let s = format!("{n:.10}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// Write workbook sheets to an `.xlsx` path (formulas preserved as formulas).
pub fn write_xlsx_path(workbook: &Workbook, path: &Path) -> Result<(), XlsxError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let bytes = write_xlsx_bytes(workbook)?;
    std::fs::write(path, bytes)?;
    Ok(())
}

pub fn write_xlsx_bytes(workbook: &Workbook) -> Result<Vec<u8>, XlsxError> {
    let mut xlsx = XlsxWriter::new();
    for (i, sheet) in workbook.sheets.iter().enumerate() {
        let name = if sheet.name.trim().is_empty() {
            format!("Sheet{}", i + 1)
        } else {
            sheet.name.clone()
        };

        xlsx.add_worksheet();
        let worksheet = xlsx
            .worksheet_from_index(i)
            .map_err(|e| XlsxError::Write(e.to_string()))?;
        worksheet
            .set_name(&name)
            .map_err(|e| XlsxError::Write(e.to_string()))?;

        for (addr, cell) in sheet.occupied() {
            let row = addr.row;
            let col = addr.col as u16;
            let raw = cell.raw.as_str();
            if let Some(body) = raw.trim().strip_prefix('=') {
                worksheet
                    .write_formula(row, col, body)
                    .map_err(|e| XlsxError::Write(e.to_string()))?;
            } else if let Ok(n) = raw.trim().parse::<f64>() {
                worksheet
                    .write_number(row, col, n)
                    .map_err(|e| XlsxError::Write(e.to_string()))?;
            } else if raw.eq_ignore_ascii_case("TRUE") {
                worksheet
                    .write_boolean(row, col, true)
                    .map_err(|e| XlsxError::Write(e.to_string()))?;
            } else if raw.eq_ignore_ascii_case("FALSE") {
                worksheet
                    .write_boolean(row, col, false)
                    .map_err(|e| XlsxError::Write(e.to_string()))?;
            } else {
                worksheet
                    .write_string(row, col, raw)
                    .map_err(|e| XlsxError::Write(e.to_string()))?;
            }
        }
    }
    xlsx.save_to_buffer()
        .map_err(|e| XlsxError::Write(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addr::CellAddr;

    #[test]
    fn xlsx_round_trip_values_and_formula() {
        let mut wb = Workbook::new();
        wb.active_sheet_mut().name = "Data".into();
        wb.set_cell(CellAddr::new(0, 0), "10");
        wb.set_cell(CellAddr::new(0, 1), "20");
        wb.set_cell(CellAddr::new(0, 2), "=SUM(A1:A2)");
        wb.sheets.push(Sheet::new("Extra"));
        wb.sheets[1].set_raw(CellAddr::new(0, 0), "hi");

        let bytes = write_xlsx_bytes(&wb).unwrap();
        assert!(bytes.starts_with(b"PK"));

        let tmp = std::env::temp_dir().join("rust-office-calc-roundtrip.xlsx");
        std::fs::write(&tmp, &bytes).unwrap();
        let restored = load_xlsx_path(&tmp).unwrap();
        let _ = std::fs::remove_file(&tmp);

        assert!(restored.sheets.len() >= 2);
        assert_eq!(restored.sheets[0].name, "Data");
        assert_eq!(restored.sheets[0].raw(CellAddr::new(0, 0)), "10");
        assert_eq!(restored.sheets[0].raw(CellAddr::new(0, 1)), "20");
        let f = restored.sheets[0].raw(CellAddr::new(0, 2));
        assert!(
            f.contains("SUM") || f == "30",
            "expected formula or cached sum, got {f:?}"
        );
        assert_eq!(restored.sheets[1].raw(CellAddr::new(0, 0)), "hi");
    }
}
