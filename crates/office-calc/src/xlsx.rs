//! XLSX import / export via calamine + rust_xlsxwriter.

use std::path::Path;

use calamine::{open_workbook_auto, CellErrorType, Data, Reader};
use rust_xlsxwriter::Workbook as XlsxWriter;
use thiserror::Error;

use crate::addr::CellAddr;
use crate::cell::CalcError;
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
/// A values or formulas read error on any sheet rejects the entire import.
pub fn load_xlsx_path(path: &Path) -> Result<Workbook, XlsxError> {
    let mut excel = open_workbook_auto(path).map_err(|e| XlsxError::Read(e.to_string()))?;
    let names = excel.sheet_names().to_vec();
    if names.is_empty() {
        return Ok(Workbook::new());
    }

    let mut sheets = Vec::new();
    for name in names {
        let mut sheet = Sheet::new(name.clone());
        let range = excel.worksheet_range(&name).map_err(|error| {
            XlsxError::Read(format!("cannot read values of worksheet {name:?}: {error}"))
        })?;
        let (sr, sc) = range.start().unwrap_or((0, 0));
        for (r, c, value) in range.used_cells() {
            let raw = data_to_raw(value);
            if !raw.is_empty() {
                sheet.set_raw(CellAddr::new(sc + c as u32, sr + r as u32), raw);
            }
        }
        let formulas = excel.worksheet_formula(&name).map_err(|error| {
            XlsxError::Read(format!(
                "cannot read formulas of worksheet {name:?}: {error}"
            ))
        })?;
        let (sr, sc) = formulas.start().unwrap_or((0, 0));
        for (r, c, formula) in formulas.used_cells() {
            let f = formula.trim();
            if !f.is_empty() {
                let text = if f.starts_with('=') {
                    f.to_string()
                } else {
                    format!("={f}")
                };
                sheet.set_raw(CellAddr::new(sc + c as u32, sr + r as u32), text);
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
        Data::String(s) => text_to_input(s),
        // Use the shortest representation that parses back to the same f64.
        // Display rounding must not change stored values or later calculations.
        Data::Float(f) => f.to_string(),
        Data::Int(i) => i.to_string(),
        Data::Bool(b) => {
            if *b {
                "TRUE".into()
            } else {
                "FALSE".into()
            }
        }
        // The raw-input model represents error values as constant formulas.
        Data::Error(e) => format!(
            "={}",
            match e {
                CellErrorType::Div0 => CalcError::Div0,
                CellErrorType::NA => CalcError::Na,
                CellErrorType::Name => CalcError::Name,
                CellErrorType::Null => CalcError::Null,
                CellErrorType::Num => CalcError::Num,
                CellErrorType::Ref => CalcError::Ref,
                CellErrorType::Value => CalcError::Value,
                CellErrorType::GettingData => CalcError::GettingData,
            }
        ),
        Data::DateTime(dt) => dt.to_string(),
        Data::DateTimeIso(s) | Data::DurationIso(s) => s.clone(),
    }
}

/// Escape text that would otherwise be reinterpreted by evaluation or export.
fn text_to_input(text: &str) -> String {
    let trimmed = text.trim();
    if text.starts_with('\'')
        || trimmed.starts_with('=')
        || trimmed.parse::<f64>().is_ok()
        || trimmed.eq_ignore_ascii_case("TRUE")
        || trimmed.eq_ignore_ascii_case("FALSE")
        || (!text.is_empty() && trimmed.is_empty())
    {
        format!("'{text}")
    } else {
        text.to_owned()
    }
}

/// Write workbook sheets to an `.xlsx` path (formulas preserved as formulas).
pub fn write_xlsx_path(workbook: &Workbook, path: &Path) -> Result<(), XlsxError> {
    let bytes = write_xlsx_bytes(workbook)?;
    office_core::storage::atomic_write(path, &bytes)?;
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
            if let Some(text) = cell.literal_text() {
                worksheet
                    .write_string(row, col, text)
                    .map_err(|e| XlsxError::Write(e.to_string()))?;
            } else if let Some(body) = raw.trim().strip_prefix('=') {
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
    use crate::{CellRange, Value};
    use std::io::{Cursor, Read, Write};

    fn reload(bytes: &[u8]) -> Result<Workbook, XlsxError> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workbook.xlsx");
        std::fs::write(&path, bytes).unwrap();
        load_xlsx_path(&path)
    }

    fn replace_second_sheet(xml: Option<&str>) -> Vec<u8> {
        let mut book = Workbook::new();
        book.active_sheet_mut().name = "Good".into();
        book.set_cell(CellAddr::new(0, 0), "keep me");
        let mut broken = Sheet::new("壊れたシート");
        broken.set_raw(CellAddr::new(0, 0), "=1+1");
        book.sheets.push(broken);
        let bytes = write_xlsx_bytes(&book).unwrap();
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut output = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for index in 0..archive.len() {
            let mut part = archive.by_index(index).unwrap();
            if part.name() == "xl/worksheets/sheet2.xml" {
                if let Some(xml) = xml {
                    output
                        .start_file(part.name(), zip::write::FileOptions::default())
                        .unwrap();
                    output.write_all(xml.as_bytes()).unwrap();
                }
            } else {
                let mut contents = Vec::new();
                part.read_to_end(&mut contents).unwrap();
                output
                    .start_file(part.name(), zip::write::FileOptions::default())
                    .unwrap();
                output.write_all(&contents).unwrap();
            }
        }
        output.finish().unwrap().into_inner()
    }

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

        let restored = reload(&bytes).unwrap();

        assert_eq!(restored.sheets.len(), 2);
        assert_eq!(restored.sheets[0].name, "Data");
        assert_eq!(restored.sheets[0].raw(CellAddr::new(0, 0)), "10");
        assert_eq!(restored.sheets[0].raw(CellAddr::new(0, 1)), "20");
        assert_eq!(restored.sheets[0].raw(CellAddr::new(0, 2)), "=SUM(A1:A2)");
        assert_eq!(restored.sheets[0].display(CellAddr::new(0, 2)), "30");
        assert_eq!(restored.sheets[1].raw(CellAddr::new(0, 0)), "hi");
        assert!(!restored.is_dirty() && !restored.can_undo() && !restored.can_redo());
    }

    #[test]
    fn formulas_preserve_absolute_mixed_range_and_error_references() {
        let mut book = Workbook::new();
        let formulas = [
            "=A1+$A$1+$A1+A$1",
            "=SUM($A$1:B2)+SUM(A$1:$B2)",
            "=SUM(#REF!)+$A$1",
            "=IF(1,\"日本語 A1 \"\"B2\"\"\",#REF!)",
            "='売上 データ'!$A1+Sheet2!B$2",
        ];
        book.set_cell(CellAddr::new(0, 0), "3");
        book.set_cell(CellAddr::new(0, 1), "4");
        for (row, formula) in formulas.iter().enumerate() {
            book.set_cell(CellAddr::new(2, row as u32), *formula);
        }
        let restored = reload(&write_xlsx_bytes(&book).unwrap()).unwrap();
        for (row, formula) in formulas.iter().enumerate() {
            assert_eq!(
                restored.active_sheet().raw(CellAddr::new(2, row as u32)),
                *formula
            );
        }
        assert_eq!(restored.active_sheet().display(CellAddr::new(2, 0)), "12");
        assert_eq!(restored.active_sheet().display(CellAddr::new(2, 1)), "14");
        assert_eq!(
            restored.active_sheet().display(CellAddr::new(2, 2)),
            "#REF!"
        );
        assert_eq!(
            restored.active_sheet().display(CellAddr::new(2, 3)),
            "日本語 A1 \"B2\""
        );
    }

    #[test]
    fn numeric_values_keep_their_f64_precision_after_reload() {
        let values: [f64; 10] = [
            0.0,
            1.0,
            -123.0,
            1.2345678901234567,
            -9.876543210987654,
            1e-12,
            -1e-12,
            1e100,
            1e-100,
            1234567890123456.0,
        ];
        let mut book = Workbook::new();
        for (row, value) in values.iter().enumerate() {
            book.set_cell(CellAddr::new(0, row as u32), value.to_string());
        }
        book.set_cell(CellAddr::new(1, 0), "=A6*1000000000000");
        let restored = reload(&write_xlsx_bytes(&book).unwrap()).unwrap();
        for (row, value) in values.iter().enumerate() {
            let raw = restored.active_sheet().raw(CellAddr::new(0, row as u32));
            assert_eq!(
                raw.parse::<f64>().unwrap().to_bits(),
                value.to_bits(),
                "{value} reloaded as {raw}"
            );
        }
        assert_eq!(restored.active_sheet().display(CellAddr::new(1, 0)), "1");
    }

    #[test]
    fn empty_and_sparse_unicode_sheets_survive_reload() {
        let mut book = Workbook::new();
        book.active_sheet_mut().name = "空のシート".into();
        let mut sparse = Sheet::new("売上 データ");
        sparse.set_raw(CellAddr::new(702, 1000), "日本語\nsecond line");
        sparse.set_raw(CellAddr::new(703, 1001), "=SUM(AAA1001:AAA1002)");
        book.sheets.push(sparse);
        let restored = reload(&write_xlsx_bytes(&book).unwrap()).unwrap();
        assert_eq!(restored.sheets.len(), 2);
        assert_eq!(restored.sheets[0].name, "空のシート");
        assert_eq!(restored.sheets[0].occupied().count(), 0);
        assert_eq!(restored.sheets[1].name, "売上 データ");
        assert_eq!(restored.sheets[1].occupied().count(), 2);
        assert_eq!(
            restored.sheets[1].raw(CellAddr::new(702, 1000)),
            "日本語\nsecond line"
        );
        assert_eq!(
            restored.sheets[1].raw(CellAddr::new(703, 1001)),
            "=SUM(AAA1001:AAA1002)"
        );
        assert!(!restored.is_dirty() && !restored.can_undo() && !restored.can_redo());
    }

    #[test]
    fn invalid_sheet_values_fail_the_whole_workbook_load() {
        let bytes = replace_second_sheet(Some(
            r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><dimension ref="A1"/><sheetData><row r="1"><c r="A1" t="n"><v>not-a-number</v></c></row></sheetData></worksheet>"#,
        ));
        let mut reader = calamine::open_workbook_auto_from_rs(Cursor::new(bytes.clone())).unwrap();
        assert!(reader.worksheet_range("壊れたシート").is_err());
        let error = reload(&bytes).expect_err("a broken sheet must not become an empty sheet");
        assert!(matches!(error, XlsxError::Read(_)));
        assert!(error.to_string().contains("壊れたシート"));
        assert!(error.to_string().contains("values"));
    }

    #[test]
    fn invalid_sheet_formulas_fail_instead_of_importing_cached_values() {
        let bytes = replace_second_sheet(Some(
            r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><dimension ref="A1"/><sheetData><row r="1"><c r="A1"><f t="shared">1+1</f><v>2</v></c></row></sheetData></worksheet>"#,
        ));
        let mut reader = calamine::open_workbook_auto_from_rs(Cursor::new(bytes.clone())).unwrap();
        assert_eq!(
            reader
                .worksheet_range("壊れたシート")
                .unwrap()
                .get_value((0, 0)),
            Some(&Data::Float(2.0))
        );
        assert!(reader.worksheet_formula("壊れたシート").is_err());
        let error = reload(&bytes).expect_err("cached values must not hide a formula read error");
        assert!(matches!(error, XlsxError::Read(_)));
        assert!(error.to_string().contains("壊れたシート"));
        assert!(error.to_string().contains("formulas"));
    }

    #[test]
    fn missing_sheet_xml_fails_instead_of_returning_a_partial_workbook() {
        let bytes = replace_second_sheet(None);
        let error = reload(&bytes).expect_err("missing sheets must not be silently skipped");
        assert!(matches!(error, XlsxError::Read(_)));
        assert!(error.to_string().contains("壊れたシート"));
    }

    #[test]
    fn invalid_workbook_reports_a_read_error() {
        assert!(matches!(
            reload(b"not an XLSX file"),
            Err(XlsxError::Read(_))
        ));
    }

    #[test]
    fn imported_text_cells_stay_literal_after_save_and_reload() {
        let texts = [
            "00123",
            "1e12",
            "=1+1",
            " =A1 ",
            "TRUE",
            "false",
            "'apostrophe",
            "日本語",
            " \t ",
            " NaN ",
            "#REF!",
        ];
        let mut external = XlsxWriter::new();
        let sheet = external.add_worksheet();
        for (row, text) in texts.iter().enumerate() {
            sheet.write_string(row as u32, 0, *text).unwrap();
        }
        sheet.write_number(0, 1, 123.0).unwrap();
        sheet.write_boolean(1, 1, true).unwrap();
        sheet.write_formula(2, 1, "1+1").unwrap();
        let mut book = reload(&external.save_to_buffer().unwrap()).unwrap();
        for _ in 0..2 {
            for (row, text) in texts.iter().enumerate() {
                let address = CellAddr::new(0, row as u32);
                assert_eq!(
                    book.active_sheet().evaluate(address),
                    Value::Text((*text).to_owned())
                );
                assert!(!book.active_sheet().get(address).unwrap().is_formula());
            }
            assert_eq!(
                book.active_sheet().evaluate(CellAddr::new(1, 0)),
                Value::Number(123.0)
            );
            assert_eq!(book.active_sheet().raw(CellAddr::new(1, 1)), "TRUE");
            assert_eq!(book.active_sheet().raw(CellAddr::new(1, 2)), "=1+1");
            let bytes = write_xlsx_bytes(&book).unwrap();
            let mut reader =
                calamine::open_workbook_auto_from_rs(Cursor::new(bytes.clone())).unwrap();
            let values = reader.worksheet_range("Sheet1").unwrap();
            let formulas = reader.worksheet_formula("Sheet1").unwrap();
            for (row, text) in texts.iter().enumerate() {
                let position = (row as u32, 0);
                assert_eq!(
                    values.get_value(position),
                    Some(&Data::String((*text).to_owned()))
                );
                assert!(formulas.get_value(position).is_none_or(String::is_empty));
            }
            assert_eq!(values.get_value((0, 1)), Some(&Data::Float(123.0)));
            assert_eq!(values.get_value((1, 1)), Some(&Data::Bool(true)));
            assert_eq!(formulas.get_value((2, 1)).map(String::as_str), Some("1+1"));
            book = reload(&bytes).unwrap();
        }
    }

    #[test]
    fn apostrophe_input_survives_copy_history_and_xlsx() {
        let mut book = Workbook::new();
        let inputs = ["'00123", "'=A1", "''quoted", "' \t "];
        let texts = ["00123", "=A1", "'quoted", " \t "];
        for (row, input) in inputs.iter().enumerate() {
            book.set_cell(CellAddr::new(0, row as u32), *input);
            assert_eq!(
                book.active_sheet().display(CellAddr::new(0, row as u32)),
                texts[row]
            );
        }
        book.mark_clean();
        let range = CellRange {
            start: CellAddr::new(0, 0),
            end: CellAddr::new(0, 3),
        };
        let copied = book.copy_tsv(range).unwrap();
        book.paste_tsv_with_origin(CellAddr::new(1, 5), &copied, Some(CellAddr::new(0, 0)))
            .unwrap();
        for (row, text) in texts.iter().enumerate() {
            assert_eq!(
                book.active_sheet()
                    .display(CellAddr::new(1, 5 + row as u32)),
                *text
            );
        }
        assert!(book.undo());
        assert!(!book.is_dirty());
        assert_eq!(book.active_sheet().raw(CellAddr::new(1, 5)), "");
        assert!(book.redo());
        let restored = reload(&write_xlsx_bytes(&book).unwrap()).unwrap();
        for (row, text) in texts.iter().enumerate() {
            assert_eq!(
                restored
                    .active_sheet()
                    .display(CellAddr::new(1, 5 + row as u32)),
                *text
            );
        }
    }

    #[test]
    fn imported_error_cells_remain_errors_after_save_and_reload() {
        let tokens = [
            "#DIV/0!", "#N/A", "#NAME?", "#NULL!", "#NUM!", "#REF!", "#VALUE!",
        ];
        let rows = tokens
            .iter()
            .enumerate()
            .map(|(row, token)| {
                format!(
                    "<row r=\"{}\"><c r=\"A{}\" t=\"e\"><v>{token}</v></c></row>",
                    row + 1,
                    row + 1
                )
            })
            .collect::<String>();
        let bytes = replace_second_sheet(Some(&format!("<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><dimension ref=\"A1:A7\"/><sheetData>{rows}</sheetData></worksheet>")));
        let mut book = reload(&bytes).unwrap();
        book.active = 1;
        for (row, token) in tokens.iter().enumerate() {
            let address = CellAddr::new(0, row as u32);
            assert_eq!(book.active_sheet().display(address), *token);
            assert!(matches!(
                book.active_sheet().evaluate(address),
                Value::Error(_)
            ));
            book.set_cell(CellAddr::new(1, row as u32), format!("=A{}+1", row + 1));
        }
        let restored = reload(&write_xlsx_bytes(&book).unwrap()).unwrap();
        for (row, token) in tokens.iter().enumerate() {
            assert_eq!(
                restored.sheets[1].display(CellAddr::new(0, row as u32)),
                *token
            );
            assert_eq!(
                restored.sheets[1].display(CellAddr::new(1, row as u32)),
                *token
            );
        }
    }
}
