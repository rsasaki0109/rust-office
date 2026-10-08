//! Tab-separated clipboard interchange with Excel-style quoted fields.

use thiserror::Error;

use crate::addr::CellRange;

pub const MAX_SHEET_ROWS: u32 = 1_048_576;
pub const MAX_SHEET_COLS: u32 = 16_384;
const MAX_CLIPBOARD_CELLS: u64 = 1_000_000;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ClipboardError {
    #[error("The clipboard contains an unterminated quoted field")]
    UnterminatedQuote,
    #[error("Unexpected text after a quoted clipboard field")]
    InvalidQuote,
    #[error("The range exceeds XLSX worksheet limits")]
    OutOfBounds,
    #[error("Copy or paste at most 1,000,000 cells at a time")]
    TooLarge,
}

pub(crate) fn validate_range(range: CellRange) -> Result<CellRange, ClipboardError> {
    let range = range.normalize();
    if range.end.row >= MAX_SHEET_ROWS || range.end.col >= MAX_SHEET_COLS {
        return Err(ClipboardError::OutOfBounds);
    }
    let count = u64::from(range.end.row - range.start.row + 1)
        * u64::from(range.end.col - range.start.col + 1);
    if count > MAX_CLIPBOARD_CELLS {
        return Err(ClipboardError::TooLarge);
    }
    Ok(range)
}

pub(crate) fn parse_tsv(text: &str) -> Result<Vec<Vec<String>>, ClipboardError> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut after_quote = false;
    let mut field_start = true;
    let mut cell_count = 0u64;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if quoted {
            if ch == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    quoted = false;
                    after_quote = true;
                }
            } else {
                field.push(ch);
            }
            continue;
        }
        if ch == '"' && field_start {
            quoted = true;
            field_start = false;
        } else if ch == '\t' || ch == '\n' || ch == '\r' {
            cell_count += 1;
            if cell_count > MAX_CLIPBOARD_CELLS {
                return Err(ClipboardError::TooLarge);
            }
            row.push(std::mem::take(&mut field));
            field_start = true;
            after_quote = false;
            if ch != '\t' {
                if ch == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                rows.push(std::mem::take(&mut row));
            }
        } else if after_quote {
            return Err(ClipboardError::InvalidQuote);
        } else {
            field.push(ch);
            field_start = false;
        }
    }
    if quoted {
        return Err(ClipboardError::UnterminatedQuote);
    }
    // One final line delimiter is a terminator, not an additional empty row.
    if !field_start || !row.is_empty() || rows.is_empty() {
        if cell_count >= MAX_CLIPBOARD_CELLS {
            return Err(ClipboardError::TooLarge);
        }
        row.push(field);
        rows.push(row);
    }
    let width = rows.iter().map(Vec::len).max().unwrap_or(1);
    if (rows.len() as u64) * (width as u64) > MAX_CLIPBOARD_CELLS {
        return Err(ClipboardError::TooLarge);
    }
    for row in &mut rows {
        row.resize(width, String::new());
    }
    Ok(rows)
}

pub(crate) fn quote_field(text: &str) -> String {
    if text.contains(['\t', '\n', '\r', '"']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_table_preserves_blank_columns_and_pads_short_rows() {
        let rows = parse_tsv("Name\tQty\t\r\nりんご\t3\r\n\t\t\r\n").unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0], ["Name", "Qty", ""]);
        assert_eq!(rows[1], ["りんご", "3", ""]);
        assert_eq!(rows[2], ["", "", ""]);
    }

    #[test]
    fn quoted_tabs_newlines_and_quotes_round_trip() {
        for value in ["line 1\nline 2", "a\tb", "say \"hello\"", "日本語"] {
            assert_eq!(parse_tsv(&quote_field(value)).unwrap(), vec![vec![value]]);
        }
    }

    #[test]
    fn rejects_malformed_quotes() {
        assert_eq!(
            parse_tsv("\"unclosed"),
            Err(ClipboardError::UnterminatedQuote)
        );
        assert_eq!(
            parse_tsv("\"closed\"extra"),
            Err(ClipboardError::InvalidQuote)
        );
    }

    #[test]
    fn sparse_ragged_table_is_limited_before_padding() {
        let text = format!("{}\n{}", "\t".repeat(1_000), "x\n".repeat(1_000));
        assert_eq!(parse_tsv(&text), Err(ClipboardError::TooLarge));
    }
}
