//! Cell presentation is separate from raw values and formula evaluation.
use crate::Value;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellFormat {
    pub number_format: String,
    /// Retain locale-dependent builtin codes that have no portable format string.
    #[serde(default)]
    pub builtin_number_format: Option<u8>,
    pub bold: bool,
    pub italic: bool,
}

impl Default for CellFormat {
    fn default() -> Self {
        Self {
            number_format: "General".into(),
            builtin_number_format: None,
            bold: false,
            italic: false,
        }
    }
}

#[derive(Debug, Clone)]
pub enum FormatChange {
    Number(String),
    Bold(bool),
    Italic(bool),
    Clear,
}

impl FormatChange {
    pub(crate) fn apply(&self, mut format: CellFormat) -> CellFormat {
        match self {
            Self::Number(code) => {
                format.number_format = code.clone();
                format.builtin_number_format = None;
            }
            Self::Bold(value) => format.bold = *value,
            Self::Italic(value) => format.italic = *value,
            Self::Clear => format = CellFormat::default(),
        }
        format
    }
}

impl CellFormat {
    pub(crate) fn xlsx_format(&self) -> rust_xlsxwriter::Format {
        let mut format = rust_xlsxwriter::Format::new();
        format = if let Some(index) = self.builtin_number_format {
            format.set_num_format_index(index)
        } else {
            format.set_num_format(&self.number_format)
        };
        if self.bold {
            format = format.set_bold();
        }
        if self.italic {
            format = format.set_italic();
        }
        format
    }

    /// Supported presentation codes; other imported codes remain exportable and
    /// use General display. Text, empty cells and errors are never reinterpreted.
    pub fn display(&self, value: Value) -> String {
        let Value::Number(n) = value else {
            return value.display();
        };
        match self.number_format.as_str() {
            "0" => format!("{n:.0}"),
            "0.00" => format!("{n:.2}"),
            "0%" => format!("{:.0}%", n * 100.0),
            "0.00%" => format!("{:.2}%", n * 100.0),
            "yyyy-mm-dd" => excel_date(n).unwrap_or_else(|| Value::Number(n).display()),
            _ => Value::Number(n).display(),
        }
    }
}

fn excel_date(serial: f64) -> Option<String> {
    if !serial.is_finite() || !(0.0..2_958_466.0).contains(&serial) {
        return None;
    }
    let serial = serial.floor() as i64;
    // Excel's fictitious leap day is part of its 1900 serial convention.
    if serial == 60 {
        return Some("1900-02-29".into());
    }
    let unix_days = serial - 25_568 - i64::from(serial > 60);
    let z = unix_days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + i64::from(month <= 2);
    Some(format!("{year:04}-{month:02}-{day:02}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn presentation_preserves_numeric_meaning_text_errors_and_excel_dates() {
        let mut format = CellFormat {
            number_format: "0.00%".into(),
            ..Default::default()
        };
        assert_eq!(format.display(Value::Number(0.125)), "12.50%");
        assert_eq!(format.display(Value::Text("0.125".into())), "0.125");
        assert_eq!(
            format.display(Value::Error(crate::CalcError::Div0)),
            "#DIV/0!"
        );
        assert_eq!(format.display(Value::Empty), "");
        format.number_format = "0.00".into();
        assert_eq!(format.display(Value::Number(1.235)), "1.24");
        format.number_format = "yyyy-mm-dd".into();
        for (serial, date) in [
            (0., "1899-12-31"),
            (1., "1900-01-01"),
            (59., "1900-02-28"),
            (60., "1900-02-29"),
            (61., "1900-03-01"),
            (25569.75, "1970-01-01"),
            (2958465., "9999-12-31"),
        ] {
            assert_eq!(format.display(Value::Number(serial)), date);
        }
        assert_eq!(format.display(Value::Number(-1.)), "-1");
    }
}
