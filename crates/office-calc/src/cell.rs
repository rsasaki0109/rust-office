//! Cell values and raw input.

use serde::{Deserialize, Serialize};

/// What the user typed into a cell (formula starts with `=`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Cell {
    pub raw: String,
}

impl Cell {
    pub fn new(raw: impl Into<String>) -> Self {
        Self { raw: raw.into() }
    }

    pub fn is_empty(&self) -> bool {
        self.raw.trim().is_empty()
    }

    pub fn is_formula(&self) -> bool {
        self.raw.trim_start().starts_with('=')
    }
}

/// Evaluated display value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Empty,
    Number(f64),
    Text(String),
    Error(CalcError),
}

impl Value {
    pub fn as_number(&self) -> Result<f64, CalcError> {
        match self {
            Value::Empty => Ok(0.0),
            Value::Number(n) => Ok(*n),
            Value::Text(t) => {
                let t = t.trim();
                if t.is_empty() {
                    Ok(0.0)
                } else {
                    t.parse::<f64>().map_err(|_| CalcError::Value)
                }
            }
            Value::Error(e) => Err(*e),
        }
    }

    pub fn display(&self) -> String {
        match self {
            Value::Empty => String::new(),
            Value::Number(n) => format_number(*n),
            Value::Text(t) => t.clone(),
            Value::Error(e) => e.to_string(),
        }
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

/// Formula / evaluation errors (Excel-like tokens).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CalcError {
    Ref,
    Value,
    Div0,
    Cycle,
    Name,
}

impl std::fmt::Display for CalcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            CalcError::Ref => "#REF!",
            CalcError::Value => "#VALUE!",
            CalcError::Div0 => "#DIV/0!",
            CalcError::Cycle => "#CYCLE!",
            CalcError::Name => "#NAME?",
        })
    }
}
