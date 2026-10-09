//! Spreadsheet engine for rust-office Calc.
//!
//! Provides a sparse worksheet model, A1 addressing, a small formula language
//! (`+ - * /`, `SUM` / `AVERAGE` / `MIN` / `MAX` / `COUNT` / `IF`, cell refs and
//! ranges), CSV I/O, and XLSX import/export.

mod addr;
mod cell;
mod clipboard;
mod csv;
mod delimited;
mod dimensions;
mod format;
mod formula;
mod reference;
mod sheet;
mod workbook;
mod xlsx;
mod xlsx_format;

pub use addr::{col_to_letters, parse_a1, parse_a1_range, AddrError, CellAddr, CellRange};
pub use cell::{CalcError, Cell, Value};
pub use clipboard::{ClipboardError, MAX_SHEET_COLS, MAX_SHEET_ROWS};
pub use csv::{load_csv_path, load_csv_str, sheet_to_csv, write_csv_path, CsvError};
pub use dimensions::{DimensionAxis, DimensionError, DEFAULT_COLUMN_WIDTH, DEFAULT_ROW_HEIGHT};
pub use format::{CellFormat, FormatChange};
pub use formula::evaluate_formula;
pub use reference::translate_formula;
pub use sheet::{Sheet, SheetPresentation, DEFAULT_COLS, DEFAULT_ROWS};
pub use workbook::{SheetError, Workbook};
pub use xlsx::{load_xlsx_path, write_xlsx_bytes, write_xlsx_path, XlsxError};
