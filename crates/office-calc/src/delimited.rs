//! Shared quoted-field parser for CSV files and tabular clipboard input.

use thiserror::Error;

pub(crate) struct Format {
    pub delimiter: char,
    pub max_fields: u64,
    pub allow_unquoted_quotes: bool,
}

#[derive(Debug, Error)]
pub(crate) enum ParseError {
    #[error("Unterminated quoted field in record {record}, field {field}")]
    UnterminatedQuote { record: usize, field: usize },
    #[error("Invalid quote in record {record}, field {field}")]
    InvalidQuote { record: usize, field: usize },
    #[error("Too many fields")]
    TooLarge,
}

pub(crate) fn parse_records(text: &str, format: Format) -> Result<Vec<Vec<String>>, ParseError> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut after_quote = false;
    let mut field_start = true;
    let mut field_count = 0u64;
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
        } else if ch == format.delimiter || ch == '\n' || ch == '\r' {
            field_count += 1;
            if field_count > format.max_fields {
                return Err(ParseError::TooLarge);
            }
            row.push(std::mem::take(&mut field));
            field_start = true;
            after_quote = false;
            if ch != format.delimiter {
                if ch == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                rows.push(std::mem::take(&mut row));
            }
        } else if after_quote || (ch == '"' && !format.allow_unquoted_quotes) {
            return Err(ParseError::InvalidQuote {
                record: rows.len() + 1,
                field: row.len() + 1,
            });
        } else {
            field.push(ch);
            field_start = false;
        }
    }
    if quoted {
        return Err(ParseError::UnterminatedQuote {
            record: rows.len() + 1,
            field: row.len() + 1,
        });
    }
    // One final line delimiter is a terminator, not an additional empty row.
    if !field_start || !row.is_empty() || rows.is_empty() {
        if field_count >= format.max_fields {
            return Err(ParseError::TooLarge);
        }
        row.push(field);
        rows.push(row);
    }
    Ok(rows)
}
