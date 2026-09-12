//! Minimal formula parser / evaluator.

use std::collections::HashSet;

use crate::addr::{parse_a1, parse_a1_range, CellAddr};
use crate::cell::{CalcError, Value};
use crate::sheet::Sheet;

#[derive(Debug, Clone, PartialEq)]
enum Expr {
    Number(f64),
    Text(String),
    Ref(CellAddr),
    Range(crate::addr::CellRange),
    UnaryNeg(Box<Expr>),
    BinOp(BinOp, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
}

struct Parser<'a> {
    src: &'a [u8],
    i: usize,
}

impl<'a> Parser<'a> {
    fn new(s: &'a str) -> Self {
        Self {
            src: s.as_bytes(),
            i: 0,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.i).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let c = self.peek()?;
        self.i += 1;
        Some(c)
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t')) {
            self.i += 1;
        }
    }

    fn parse(&mut self) -> Result<Expr, CalcError> {
        self.skip_ws();
        let expr = self.parse_add()?;
        self.skip_ws();
        if self.i != self.src.len() {
            return Err(CalcError::Value);
        }
        Ok(expr)
    }

    fn parse_add(&mut self) -> Result<Expr, CalcError> {
        let mut left = self.parse_mul()?;
        loop {
            self.skip_ws();
            match self.peek() {
                Some(b'+') => {
                    self.bump();
                    let right = self.parse_mul()?;
                    left = Expr::BinOp(BinOp::Add, Box::new(left), Box::new(right));
                }
                Some(b'-') => {
                    self.bump();
                    let right = self.parse_mul()?;
                    left = Expr::BinOp(BinOp::Sub, Box::new(left), Box::new(right));
                }
                _ => break,
            }
        }
        Ok(left)
    }

    fn parse_mul(&mut self) -> Result<Expr, CalcError> {
        let mut left = self.parse_unary()?;
        loop {
            self.skip_ws();
            match self.peek() {
                Some(b'*') => {
                    self.bump();
                    let right = self.parse_unary()?;
                    left = Expr::BinOp(BinOp::Mul, Box::new(left), Box::new(right));
                }
                Some(b'/') => {
                    self.bump();
                    let right = self.parse_unary()?;
                    left = Expr::BinOp(BinOp::Div, Box::new(left), Box::new(right));
                }
                _ => break,
            }
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, CalcError> {
        self.skip_ws();
        if self.peek() == Some(b'-') {
            self.bump();
            return Ok(Expr::UnaryNeg(Box::new(self.parse_unary()?)));
        }
        if self.peek() == Some(b'+') {
            self.bump();
            return self.parse_unary();
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<Expr, CalcError> {
        self.skip_ws();
        match self.peek() {
            Some(b'(') => {
                self.bump();
                let e = self.parse_add()?;
                self.skip_ws();
                if self.bump() != Some(b')') {
                    return Err(CalcError::Value);
                }
                Ok(e)
            }
            Some(b'"') => self.parse_string(),
            Some(c) if c.is_ascii_digit() || c == b'.' => self.parse_number(),
            Some(c) if c.is_ascii_alphabetic() || c == b'$' => self.parse_ident_or_ref(),
            _ => Err(CalcError::Value),
        }
    }

    fn parse_string(&mut self) -> Result<Expr, CalcError> {
        if self.bump() != Some(b'"') {
            return Err(CalcError::Value);
        }
        let mut out = String::new();
        while let Some(c) = self.bump() {
            if c == b'"' {
                return Ok(Expr::Text(out));
            }
            out.push(c as char);
        }
        Err(CalcError::Value)
    }

    fn parse_number(&mut self) -> Result<Expr, CalcError> {
        let start = self.i;
        while matches!(self.peek(), Some(b'0'..=b'9' | b'.')) {
            self.i += 1;
        }
        let s = std::str::from_utf8(&self.src[start..self.i]).map_err(|_| CalcError::Value)?;
        let n: f64 = s.parse().map_err(|_| CalcError::Value)?;
        Ok(Expr::Number(n))
    }

    fn parse_ident_or_ref(&mut self) -> Result<Expr, CalcError> {
        let start = self.i;
        while matches!(
            self.peek(),
            Some(b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'$' | b'_')
        ) {
            self.i += 1;
        }
        let ident = std::str::from_utf8(&self.src[start..self.i]).map_err(|_| CalcError::Value)?;
        self.skip_ws();
        if self.peek() == Some(b'(') {
            self.bump();
            let mut args = Vec::new();
            self.skip_ws();
            if self.peek() != Some(b')') {
                loop {
                    args.push(self.parse_add()?);
                    self.skip_ws();
                    match self.peek() {
                        Some(b',') => {
                            self.bump();
                        }
                        Some(b')') => break,
                        _ => return Err(CalcError::Value),
                    }
                }
            }
            if self.bump() != Some(b')') {
                return Err(CalcError::Value);
            }
            return Ok(Expr::Call(ident.to_ascii_uppercase(), args));
        }

        // Range A1:B2 may have been partially consumed — check for ':'
        if self.peek() == Some(b':') {
            self.bump();
            let end_start = self.i;
            while matches!(
                self.peek(),
                Some(b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'$')
            ) {
                self.i += 1;
            }
            let end = std::str::from_utf8(&self.src[end_start..self.i]).map_err(|_| CalcError::Value)?;
            let range = parse_a1_range(&format!("{ident}:{end}")).map_err(|_| CalcError::Ref)?;
            return Ok(Expr::Range(range));
        }

        if let Ok(addr) = parse_a1(ident) {
            Ok(Expr::Ref(addr))
        } else {
            Err(CalcError::Name)
        }
    }
}

/// Evaluate a formula body (without leading `=`).
pub fn evaluate_formula(sheet: &Sheet, body: &str, origin: CellAddr) -> Value {
    let mut visiting = HashSet::new();
    visiting.insert(origin);
    let value = eval_body(sheet, body, &mut visiting);
    visiting.remove(&origin);
    value
}

pub(crate) fn eval_body(
    sheet: &Sheet,
    body: &str,
    visiting: &mut HashSet<CellAddr>,
) -> Value {
    match Parser::new(body).parse() {
        Ok(expr) => eval_expr(sheet, &expr, visiting),
        Err(e) => Value::Error(e),
    }
}

fn eval_expr(sheet: &Sheet, expr: &Expr, visiting: &mut HashSet<CellAddr>) -> Value {
    match expr {
        Expr::Number(n) => Value::Number(*n),
        Expr::Text(t) => Value::Text(t.clone()),
        Expr::Ref(addr) => eval_cell(sheet, *addr, visiting),
        Expr::Range(_) => Value::Error(CalcError::Value),
        Expr::UnaryNeg(e) => match eval_expr(sheet, e, visiting).as_number() {
            Ok(n) => Value::Number(-n),
            Err(e) => Value::Error(e),
        },
        Expr::BinOp(op, a, b) => {
            let av = match eval_expr(sheet, a, visiting).as_number() {
                Ok(n) => n,
                Err(e) => return Value::Error(e),
            };
            let bv = match eval_expr(sheet, b, visiting).as_number() {
                Ok(n) => n,
                Err(e) => return Value::Error(e),
            };
            match op {
                BinOp::Add => Value::Number(av + bv),
                BinOp::Sub => Value::Number(av - bv),
                BinOp::Mul => Value::Number(av * bv),
                BinOp::Div => {
                    if bv == 0.0 {
                        Value::Error(CalcError::Div0)
                    } else {
                        Value::Number(av / bv)
                    }
                }
            }
        }
        Expr::Call(name, args) => eval_call(sheet, name, args, visiting),
    }
}

fn eval_call(
    sheet: &Sheet,
    name: &str,
    args: &[Expr],
    visiting: &mut HashSet<CellAddr>,
) -> Value {
    match name {
        "SUM" => {
            let mut sum = 0.0;
            for arg in args {
                match arg {
                    Expr::Range(r) => {
                        for addr in r.iter() {
                            match eval_cell(sheet, addr, visiting).as_number() {
                                Ok(n) => sum += n,
                                Err(e) => return Value::Error(e),
                            }
                        }
                    }
                    other => match eval_expr(sheet, other, visiting).as_number() {
                        Ok(n) => sum += n,
                        Err(e) => return Value::Error(e),
                    },
                }
            }
            Value::Number(sum)
        }
        "AVERAGE" | "AVG" => {
            let mut sum = 0.0;
            let mut count = 0.0;
            for arg in args {
                match arg {
                    Expr::Range(r) => {
                        for addr in r.iter() {
                            match eval_cell(sheet, addr, visiting).as_number() {
                                Ok(n) => {
                                    sum += n;
                                    count += 1.0;
                                }
                                Err(e) => return Value::Error(e),
                            }
                        }
                    }
                    other => match eval_expr(sheet, other, visiting).as_number() {
                        Ok(n) => {
                            sum += n;
                            count += 1.0;
                        }
                        Err(e) => return Value::Error(e),
                    },
                }
            }
            if count == 0.0 {
                Value::Error(CalcError::Div0)
            } else {
                Value::Number(sum / count)
            }
        }
        "MIN" => fold_range(sheet, args, visiting, f64::INFINITY, |a, b| a.min(b)),
        "MAX" => fold_range(sheet, args, visiting, f64::NEG_INFINITY, |a, b| a.max(b)),
        "COUNT" => {
            let mut count = 0.0;
            for arg in args {
                match arg {
                    Expr::Range(r) => {
                        for addr in r.iter() {
                            match eval_cell(sheet, addr, visiting) {
                                Value::Empty => {}
                                Value::Error(e) => return Value::Error(e),
                                _ => count += 1.0,
                            }
                        }
                    }
                    other => match eval_expr(sheet, other, visiting) {
                        Value::Empty => {}
                        Value::Error(e) => return Value::Error(e),
                        _ => count += 1.0,
                    },
                }
            }
            Value::Number(count)
        }
        "IF" => {
            if args.len() < 2 {
                return Value::Error(CalcError::Value);
            }
            let cond = match eval_expr(sheet, &args[0], visiting).as_number() {
                Ok(n) => n != 0.0,
                Err(e) => return Value::Error(e),
            };
            if cond {
                eval_expr(sheet, &args[1], visiting)
            } else if let Some(else_expr) = args.get(2) {
                eval_expr(sheet, else_expr, visiting)
            } else {
                Value::Number(0.0)
            }
        }
        _ => Value::Error(CalcError::Name),
    }
}

fn fold_range(
    sheet: &Sheet,
    args: &[Expr],
    visiting: &mut HashSet<CellAddr>,
    init: f64,
    f: fn(f64, f64) -> f64,
) -> Value {
    let mut acc = init;
    let mut any = false;
    for arg in args {
        let values: Vec<CellAddr> = match arg {
            Expr::Range(r) => r.iter().collect(),
            Expr::Ref(a) => vec![*a],
            _ => {
                return match eval_expr(sheet, arg, visiting).as_number() {
                    Ok(n) => Value::Number(n),
                    Err(e) => Value::Error(e),
                };
            }
        };
        for addr in values {
            match eval_cell(sheet, addr, visiting).as_number() {
                Ok(n) => {
                    acc = f(acc, n);
                    any = true;
                }
                Err(e) => return Value::Error(e),
            }
        }
    }
    if any {
        Value::Number(acc)
    } else {
        Value::Error(CalcError::Value)
    }
}

fn eval_cell(sheet: &Sheet, addr: CellAddr, visiting: &mut HashSet<CellAddr>) -> Value {
    sheet.evaluate_at(addr, visiting)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sheet::Sheet;

    #[test]
    fn arithmetic_and_sum() {
        let mut sheet = Sheet::new("Sheet1");
        sheet.set_raw(CellAddr::new(0, 0), "10");
        sheet.set_raw(CellAddr::new(0, 1), "20");
        sheet.set_raw(CellAddr::new(0, 2), "=A1+A2");
        sheet.set_raw(CellAddr::new(0, 3), "=SUM(A1:A2)");
        assert_eq!(sheet.display(CellAddr::new(0, 2)), "30");
        assert_eq!(sheet.display(CellAddr::new(0, 3)), "30");
    }

    #[test]
    fn if_and_count() {
        let mut sheet = Sheet::new("Sheet1");
        sheet.set_raw(CellAddr::new(0, 0), "1");
        sheet.set_raw(CellAddr::new(0, 1), "");
        sheet.set_raw(CellAddr::new(0, 2), "=IF(A1,\"yes\",\"no\")");
        sheet.set_raw(CellAddr::new(0, 3), "=COUNT(A1:A2)");
        assert_eq!(sheet.display(CellAddr::new(0, 2)), "yes");
        assert_eq!(sheet.display(CellAddr::new(0, 3)), "1");
    }

    #[test]
    fn cycle_detected() {
        let mut sheet = Sheet::new("Sheet1");
        sheet.set_raw(CellAddr::new(0, 0), "=A2");
        sheet.set_raw(CellAddr::new(0, 1), "=A1");
        assert_eq!(sheet.display(CellAddr::new(0, 0)), "#CYCLE!");
    }
}
