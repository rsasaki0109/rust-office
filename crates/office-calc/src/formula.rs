//! Minimal formula parser / evaluator.

use std::collections::{HashMap, HashSet};

use crate::addr::{parse_a1, parse_a1_range, CellAddr};
use crate::cell::{CalcError, Value};
use crate::sheet::Sheet;
use crate::{MAX_SHEET_COLS, MAX_SHEET_ROWS};

// Bound both recursive parsing and left-associated expression trees (including drop).
const MAX_DEPTH: usize = 64;
const MAX_PARSE_NODES: usize = 512;
const MAX_FORMULA_BYTES: usize = 32_768;
const MAX_WORK: usize = 100_000;

#[derive(Debug, Clone, PartialEq)]
enum Expr {
    Error(CalcError),
    Number(f64),
    Text(String),
    Ref(CellAddr),
    SheetRef(String, CellAddr),
    SheetRange(String, crate::CellRange),
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
    depth: usize,
    nodes: usize,
}

impl<'a> Parser<'a> {
    fn new(s: &'a str) -> Self {
        Self {
            src: s.as_bytes(),
            i: 0,
            depth: 0,
            nodes: 0,
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
        if self.src.len() > MAX_FORMULA_BYTES {
            return Err(CalcError::Num);
        }
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
                    self.node()?;
                    left = Expr::BinOp(BinOp::Add, Box::new(left), Box::new(right));
                }
                Some(b'-') => {
                    self.bump();
                    let right = self.parse_mul()?;
                    self.node()?;
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
                    self.node()?;
                    left = Expr::BinOp(BinOp::Mul, Box::new(left), Box::new(right));
                }
                Some(b'/') => {
                    self.bump();
                    let right = self.parse_unary()?;
                    self.node()?;
                    left = Expr::BinOp(BinOp::Div, Box::new(left), Box::new(right));
                }
                _ => break,
            }
        }
        Ok(left)
    }

    fn node(&mut self) -> Result<(), CalcError> {
        self.nodes += 1;
        if self.nodes > MAX_PARSE_NODES {
            Err(CalcError::Num)
        } else {
            Ok(())
        }
    }

    fn parse_unary(&mut self) -> Result<Expr, CalcError> {
        if self.depth >= MAX_DEPTH {
            return Err(CalcError::Num);
        }
        self.node()?;
        self.depth += 1;
        let result = self.parse_unary_inner();
        self.depth -= 1;
        result
    }

    fn parse_unary_inner(&mut self) -> Result<Expr, CalcError> {
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
            Some(b'\'') => self.parse_quoted_sheet(),
            Some(b'#') => self.parse_error(),
            Some(c) if c.is_ascii_digit() || c == b'.' => self.parse_number(),
            Some(c) if c.is_ascii_alphabetic() || c == b'$' || c >= 128 => self.parse_ident_or_ref(),
            _ => Err(CalcError::Value),
        }
    }

    fn parse_string(&mut self) -> Result<Expr, CalcError> {
        if self.bump() != Some(b'"') {
            return Err(CalcError::Value);
        }
        let mut out = Vec::new();
        while let Some(c) = self.bump() {
            if c == b'"' {
                if self.peek() == Some(b'"') {
                    self.bump();
                    out.push(b'"');
                    continue;
                }
                return String::from_utf8(out)
                    .map(Expr::Text)
                    .map_err(|_| CalcError::Value);
            }
            out.push(c);
        }
        Err(CalcError::Value)
    }

    fn parse_error(&mut self) -> Result<Expr, CalcError> {
        for error in [
            CalcError::Ref,
            CalcError::Value,
            CalcError::Div0,
            CalcError::Cycle,
            CalcError::Name,
            CalcError::Na,
            CalcError::Null,
            CalcError::Num,
            CalcError::GettingData,
        ] {
            let token = error.to_string();
            if self.src[self.i..].starts_with(token.as_bytes()) {
                self.i += token.len();
                return Ok(Expr::Error(error));
            }
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

    fn parse_quoted_sheet(&mut self) -> Result<Expr, CalcError> {
        self.bump();
        let mut name = Vec::new();
        loop {
            match self.bump() {
                Some(b'\'') if self.peek() == Some(b'\'') => { self.bump(); name.push(b'\''); }
                Some(b'\'') => break,
                Some(byte) => name.push(byte),
                None => return Err(CalcError::Value),
            }
        }
        self.skip_ws();
        self.parse_sheet_reference(String::from_utf8(name).map_err(|_| CalcError::Value)?)
    }
    fn parse_sheet_reference(&mut self, name: String) -> Result<Expr, CalcError> {
        if self.bump() != Some(b'!') || name.is_empty() || name.contains(['[', ']']) {
            return Err(CalcError::Ref);
        }
        self.skip_ws();
        let start = self.i;
        while matches!(self.peek(), Some(b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'$')) { self.i += 1; }
        let first = std::str::from_utf8(&self.src[start..self.i]).map_err(|_| CalcError::Ref)?;
        let addr = parse_a1(first).map_err(|_| CalcError::Ref)?;
        if !in_bounds(addr) { return Err(CalcError::Ref); }
        self.skip_ws();
        if self.peek() == Some(b':') {
            self.bump(); self.skip_ws(); let start = self.i;
            while matches!(self.peek(), Some(b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'$')) { self.i += 1; }
            let last = std::str::from_utf8(&self.src[start..self.i]).map_err(|_| CalcError::Ref)?;
            let range = parse_a1_range(&format!("{first}:{last}")).map_err(|_| CalcError::Ref)?;
            if !in_bounds(range.end) { return Err(CalcError::Ref); }
            Ok(Expr::SheetRange(name, range))
        } else { Ok(Expr::SheetRef(name, addr)) }
    }

    fn parse_ident_or_ref(&mut self) -> Result<Expr, CalcError> {
        let start = self.i;
        while matches!(
            self.peek(),
            Some(b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'$' | b'_' | b'.' | 128..=255)
        ) {
            self.i += 1;
        }
        let ident = std::str::from_utf8(&self.src[start..self.i]).map_err(|_| CalcError::Value)?;
        self.skip_ws();
        if self.peek() == Some(b'!') {
            return self.parse_sheet_reference(ident.to_owned());
        }
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
            let end =
                std::str::from_utf8(&self.src[end_start..self.i]).map_err(|_| CalcError::Value)?;
            let range = parse_a1_range(&format!("{ident}:{end}")).map_err(|_| CalcError::Ref)?;
            if !in_bounds(range.end) {
                return Err(CalcError::Ref);
            }
            return Ok(Expr::Range(range));
        }

        if let Ok(addr) = parse_a1(ident) {
            if !in_bounds(addr) {
                return Err(CalcError::Ref);
            }
            Ok(Expr::Ref(addr))
        } else {
            Err(CalcError::Name)
        }
    }
}

fn in_bounds(addr: CellAddr) -> bool {
    addr.col < MAX_SHEET_COLS && addr.row < MAX_SHEET_ROWS
}

// A fresh context for each displayed cell keeps caching valid after edits/Undo.
struct EvalContext<'a> {
    workbook: Option<&'a crate::Workbook>,
    sheet_index: usize,
    visiting: HashSet<(usize, CellAddr)>,
    cache: HashMap<(usize, CellAddr), Value>,
    depth: usize,
    work: usize,
    limited: bool,
}

impl<'a> EvalContext<'a> {
    fn new() -> Self {
        Self {
            workbook: None,
            sheet_index: 0,
            visiting: HashSet::new(),
            cache: HashMap::new(),
            depth: 0,
            work: MAX_WORK,
            limited: false,
        }
    }

    fn spend(&mut self, amount: usize) -> Result<(), CalcError> {
        if self.limited || amount > self.work {
            self.limited = true;
            return Err(CalcError::Num);
        }
        self.work -= amount;
        Ok(())
    }

    fn enter(&mut self) -> Result<(), CalcError> {
        if self.depth >= MAX_DEPTH {
            self.limited = true;
            return Err(CalcError::Num);
        }
        self.spend(1)?;
        self.depth += 1;
        Ok(())
    }

    fn finish(&self, value: Value) -> Value {
        if self.limited {
            Value::Error(CalcError::Num)
        } else {
            value
        }
    }
}

/// Evaluate a formula body (without leading `=`).
pub fn evaluate_formula(sheet: &Sheet, body: &str, origin: CellAddr) -> Value {
    let mut context = EvalContext::new();
    context.visiting.insert((0, origin));
    let value = eval_body(sheet, body, &mut context);
    context.finish(value)
}

pub(crate) fn evaluate_cell(sheet: &Sheet, addr: CellAddr) -> Value {
    let mut context = EvalContext::new();
    let value = eval_cell(sheet, addr, &mut context);
    context.finish(value)
}

pub(crate) fn evaluate_workbook(book: &crate::Workbook, sheet_index: usize, addr: CellAddr) -> Value {
    let Some(sheet) = book.sheets.get(sheet_index) else { return Value::Error(CalcError::Ref); };
    let mut context = EvalContext::new();
    context.workbook = Some(book); context.sheet_index = sheet_index;
    let value = eval_cell(sheet, addr, &mut context);
    context.finish(value)
}
fn target_sheet<'a>(name: &str, context: &mut EvalContext<'a>) -> Result<(usize, &'a Sheet), CalcError> {
    let book = context.workbook.ok_or(CalcError::Ref)?;
    let name = name.to_lowercase();
    for (index, sheet) in book.sheets.iter().enumerate() {
        context.spend(1)?;
        if sheet.name.to_lowercase() == name {
            return Ok((index, sheet));
        }
    }
    Err(CalcError::Ref)
}
fn eval_sheet_reference(name: &str, addr: CellAddr, context: &mut EvalContext) -> Value {
    let (index, sheet) = match target_sheet(name, context) { Ok(target) => target, Err(e) => return Value::Error(e) };
    let previous = context.sheet_index; context.sheet_index = index;
    let value = eval_cell(sheet, addr, context); context.sheet_index = previous; value
}

fn eval_body(sheet: &Sheet, body: &str, context: &mut EvalContext) -> Value {
    if let Err(error) = context.spend(body.len()) {
        return Value::Error(error);
    }
    match Parser::new(body).parse() {
        Ok(expr) => eval_expr(sheet, &expr, context),
        Err(error) => {
            if error == CalcError::Num {
                context.limited = true;
            }
            Value::Error(error)
        }
    }
}

fn eval_expr(sheet: &Sheet, expr: &Expr, context: &mut EvalContext) -> Value {
    if let Err(error) = context.enter() {
        return Value::Error(error);
    }
    let value = eval_expr_inner(sheet, expr, context);
    context.depth -= 1;
    value
}

fn eval_expr_inner(sheet: &Sheet, expr: &Expr, context: &mut EvalContext) -> Value {
    match expr {
        Expr::Error(error) => Value::Error(*error),
        Expr::Number(n) => Value::Number(*n),
        Expr::Text(t) => Value::Text(t.clone()),
        Expr::Ref(addr) => eval_cell(sheet, *addr, context),
        Expr::SheetRef(name, addr) => eval_sheet_reference(name, *addr, context),
        Expr::Range(_) | Expr::SheetRange(_, _) => Value::Error(CalcError::Value),
        Expr::UnaryNeg(e) => match eval_expr(sheet, e, context).as_number() {
            Ok(n) => Value::Number(-n),
            Err(e) => Value::Error(e),
        },
        Expr::BinOp(op, a, b) => {
            let av = match eval_expr(sheet, a, context).as_number() {
                Ok(n) => n,
                Err(e) => return Value::Error(e),
            };
            let bv = match eval_expr(sheet, b, context).as_number() {
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
        Expr::Call(name, args) => eval_call(sheet, name, args, context),
    }
}

fn eval_call(sheet: &Sheet, name: &str, args: &[Expr], context: &mut EvalContext) -> Value {
    match name {
        "SUM" | "AVERAGE" | "AVG" | "MIN" | "MAX" | "COUNT" => {
            match aggregate(sheet, name, args, context) {
                Ok(value) => Value::Number(value),
                Err(error) => Value::Error(error),
            }
        }
        "IF" => {
            if args.len() < 2 {
                return Value::Error(CalcError::Value);
            }
            let cond = match eval_expr(sheet, &args[0], context).as_number() {
                Ok(n) => n != 0.0,
                Err(e) => return Value::Error(e),
            };
            if cond {
                eval_expr(sheet, &args[1], context)
            } else if let Some(else_expr) = args.get(2) {
                eval_expr(sheet, else_expr, context)
            } else {
                Value::Number(0.0)
            }
        }
        _ => Value::Error(CalcError::Name),
    }
}

fn aggregate(
    sheet: &Sheet,
    name: &str,
    args: &[Expr],
    context: &mut EvalContext,
) -> Result<f64, CalcError> {
    let mut sum = 0.0;
    let mut count = 0usize;
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    let mut add = |value: Value, referenced: bool| -> Result<(), CalcError> {
        let number = match value {
            Value::Number(n) => Some(n),
            Value::Empty => None,
            Value::Text(_) if referenced => None,
            Value::Error(CalcError::Cycle) => return Err(CalcError::Cycle),
            Value::Error(_) if referenced && name == "COUNT" => None,
            Value::Text(text) if name == "COUNT" => text.trim().parse::<f64>().ok(),
            Value::Text(text) => Some(text.trim().parse::<f64>().map_err(|_| CalcError::Value)?),
            other => Some(other.as_number()?),
        };
        if let Some(n) = number {
            sum += n;
            count += 1;
            min = min.min(n);
            max = max.max(n);
        }
        Ok(())
    };
    for arg in args {
        match arg {
            Expr::Range(range) => {
                for addr in range_addresses(sheet, *range, context)? { add(eval_cell(sheet, addr, context), true)?; context.spend(0)?; }
            }
            Expr::SheetRange(name, range) => {
                let (index, target) = target_sheet(name, context)?;
                let previous = context.sheet_index; context.sheet_index = index;
                let result = (|| {
                    for addr in range_addresses(target, *range, context)? { add(eval_cell(target, addr, context), true)?; context.spend(0)?; }
                    Ok::<(), CalcError>(())
                })();
                context.sheet_index = previous;
                result?;
            }
            Expr::Ref(addr) => add(eval_cell(sheet, *addr, context), true)?,
            Expr::SheetRef(name, addr) => add(eval_sheet_reference(name, *addr, context), true)?,
            other => add(eval_expr(sheet, other, context), false)?,
        }
        // COUNT may ignore spreadsheet error values, but must not swallow limits.
        if context.limited {
            return Err(CalcError::Num);
        }
    }
    Ok(match name {
        "COUNT" => count as f64,
        "AVERAGE" | "AVG" => {
            if count == 0 {
                return Err(CalcError::Div0);
            }
            sum / count as f64
        }
        "MIN" if count > 0 => min,
        "MAX" if count > 0 => max,
        "MIN" | "MAX" => 0.0,
        _ => sum,
    })
}

fn range_addresses(sheet: &Sheet, range: crate::CellRange, context: &mut EvalContext) -> Result<Vec<CellAddr>, CalcError> {
    let mut addresses = Vec::new();
    for (addr, _) in sheet.occupied() {
        context.spend(1)?;
        if range.contains(addr) { addresses.push(addr); }
    }
    for (index, addr) in &context.visiting {
        if *index == context.sheet_index && range.contains(*addr) && sheet.get(*addr).is_none() { addresses.push(*addr); }
    }
    addresses.sort_unstable_by_key(|addr| (addr.row, addr.col));
    Ok(addresses)
}

fn eval_cell(sheet: &Sheet, addr: CellAddr, context: &mut EvalContext) -> Value {
    if !in_bounds(addr) {
        return Value::Error(CalcError::Ref);
    }
    if let Err(error) = context.enter() {
        return Value::Error(error);
    }
    let value = eval_cell_inner(sheet, addr, context);
    context.depth -= 1;
    value
}

fn eval_cell_inner(sheet: &Sheet, addr: CellAddr, context: &mut EvalContext) -> Value {
    let key = (context.sheet_index, addr);
    if context.visiting.contains(&key) {
        return Value::Error(CalcError::Cycle);
    }
    if let Some(value) = context.cache.get(&key) {
        return value.clone();
    }
    context.visiting.insert(key);
    let value = match sheet.get(addr) {
        None => Value::Empty,
        Some(cell) => {
            let trimmed = cell.raw.trim();
            if let Some(body) = trimmed
                .strip_prefix('=')
                .filter(|_| cell.literal_text().is_none())
            {
                eval_body(sheet, body, context)
            } else if context.depth > 1 && context.spend(cell.raw.len()).is_err() {
                Value::Error(CalcError::Num)
            } else if let Some(text) = cell.literal_text() {
                Value::Text(text.to_owned())
            } else if trimmed.is_empty() {
                Value::Empty
            } else if let Ok(n) = trimmed.parse::<f64>() {
                Value::Number(n)
            } else {
                Value::Text(cell.raw.clone())
            }
        }
    };
    context.visiting.remove(&key);
    // Avoid retaining/duplicating large strings in the memoization table.
    if !matches!(value, Value::Text(_)) {
        context.cache.insert(key, value.clone());
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sheet::Sheet;

    #[test]
    fn large_worksheet_ranges_use_sparse_cells_and_keep_row_major_order() {
        let mut sheet = Sheet::new("Sparse");
        sheet.set_raw(CellAddr::new(0, 1), "10");
        sheet.set_raw(CellAddr::new(MAX_SHEET_COLS - 1, MAX_SHEET_ROWS - 1), "20");
        sheet.set_raw(CellAddr::new(5, 500_000), "'ignored");
        for (name, expected) in [
            ("SUM", 30.0),
            ("AVERAGE", 15.0),
            ("COUNT", 2.0),
            ("MIN", 10.0),
            ("MAX", 20.0),
        ] {
            assert_eq!(
                evaluate_formula(
                    &sheet,
                    &format!("{name}(A2:XFD1048576)"),
                    CellAddr::new(1, 0)
                ),
                Value::Number(expected)
            );
        }
        // HashMap iteration order must not change rounding or the first error.
        sheet.set_raw(CellAddr::new(0, 0), "10000000000000000");
        sheet.set_raw(CellAddr::new(0, 1), "-10000000000000000");
        sheet.set_raw(CellAddr::new(0, 2), "1");
        assert_eq!(
            evaluate_formula(&sheet, "SUM(A1:A3)", CellAddr::new(1, 0)),
            Value::Number(1.0)
        );
        sheet.set_raw(CellAddr::new(0, 0), "=#REF!");
        sheet.set_raw(CellAddr::new(0, 1), "=#DIV/0!");
        assert_eq!(
            evaluate_formula(&sheet, "SUM(A1:A3)", CellAddr::new(1, 0)),
            Value::Error(CalcError::Ref)
        );
    }

    #[test]
    fn oversized_and_deep_syntax_return_num_without_panicking() {
        let sheet = Sheet::new("Syntax");
        let origin = CellAddr::new(0, 0);
        for body in [
            format!("{}1{}", "(".repeat(10_000), ")".repeat(10_000)),
            format!("{}1", "-".repeat(10_000)),
            format!("{}1", "+".repeat(10_000)),
            format!("{}1{}", "SUM(".repeat(1000), ")".repeat(1000)),
            vec!["1"; 10_000].join("+"),
            format!("SUM({})", vec!["1"; 10_000].join(",")),
            " ".repeat(MAX_FORMULA_BYTES + 1),
        ] {
            assert_eq!(
                evaluate_formula(&sheet, &body, origin),
                Value::Error(CalcError::Num)
            );
        }
        let normal = format!("{}1{}", "(".repeat(20), ")".repeat(20));
        assert_eq!(
            evaluate_formula(&sheet, &normal, origin),
            Value::Number(1.0)
        );
        // A modest flat tree reaches evaluation's depth bound, not the parser's node bound.
        assert_eq!(
            evaluate_formula(&sheet, &vec!["1"; 100].join("+"), origin),
            Value::Error(CalcError::Num)
        );
    }

    #[test]
    fn deep_cell_dependencies_are_bounded_and_count_cannot_hide_limits() {
        let mut sheet = Sheet::new("Chain");
        for row in 0..1000 {
            sheet.set_raw(CellAddr::new(0, row), format!("=A{}", row + 2));
        }
        sheet.set_raw(CellAddr::new(0, 1000), "7");
        assert_eq!(
            sheet.evaluate(CellAddr::new(0, 0)),
            Value::Error(CalcError::Num)
        );
        assert_eq!(
            evaluate_formula(&sheet, "COUNT(A1)", CellAddr::new(1, 0)),
            Value::Error(CalcError::Num)
        );
        sheet.set_raw(CellAddr::new(0, 1), "5");
        assert_eq!(sheet.evaluate(CellAddr::new(0, 0)), Value::Number(5.0));
        sheet.set_raw(CellAddr::new(0, 1), "=A1");
        assert_eq!(
            sheet.evaluate(CellAddr::new(0, 0)),
            Value::Error(CalcError::Cycle)
        );
        sheet.set_raw(CellAddr::new(0, 0), "=SUM(A1:A1048576)");
        assert_eq!(
            sheet.evaluate(CellAddr::new(0, 0)),
            Value::Error(CalcError::Cycle)
        );
        sheet.set_raw(CellAddr::new(0, 0), "=COUNT(A1:A1048576)");
        assert_eq!(
            sheet.evaluate(CellAddr::new(0, 0)),
            Value::Error(CalcError::Cycle)
        );
    }

    #[test]
    fn repeated_dependencies_are_memoized_only_within_one_evaluation() {
        let mut book = crate::Workbook::new();
        book.set_cell(CellAddr::new(0, 0), "1");
        for row in 1..22 {
            book.set_cell(CellAddr::new(0, row), format!("=A{row}+A{row}"));
        }
        let result = CellAddr::new(0, 21);
        assert_eq!(
            book.active_sheet().evaluate(result),
            Value::Number(2_f64.powi(21))
        );
        book.set_cell(CellAddr::new(0, 0), "3");
        assert_eq!(
            book.active_sheet().evaluate(result),
            Value::Number(3.0 * 2_f64.powi(21))
        );
        book.undo();
        assert_eq!(
            book.active_sheet().evaluate(result),
            Value::Number(2_f64.powi(21))
        );
        book.redo();
        assert_eq!(
            book.active_sheet().evaluate(result),
            Value::Number(3.0 * 2_f64.powi(21))
        );
    }

    #[test]
    fn occupied_cell_scans_and_repeated_large_formulas_share_a_work_budget() {
        let mut sheet = Sheet::new("Budget");
        for row in 0..1000 {
            sheet.set_raw(CellAddr::new(0, row), "1");
        }
        let body = format!("SUM({})", vec!["A1:A1000"; 100].join(","));
        assert_eq!(
            evaluate_formula(&sheet, &body, CellAddr::new(1, 0)),
            Value::Error(CalcError::Num)
        );
        let body = format!("IF(0,{body},7)");
        assert_eq!(
            evaluate_formula(&sheet, &body, CellAddr::new(1, 0)),
            Value::Number(7.0)
        );
        // Repeated parsing/string clones also consume work even with few stored cells.
        sheet.set_raw(CellAddr::new(1, 0), format!("=\"{}\"", "text".repeat(8000)));
        assert_eq!(
            evaluate_formula(&sheet, "COUNT(B1,B1,B1,B1)", CellAddr::new(2, 0)),
            Value::Error(CalcError::Num)
        );
        assert_eq!(
            evaluate_formula(&sheet, "SUM(A1:A2)", CellAddr::new(2, 0)),
            Value::Number(2.0)
        );
    }

    #[test]
    fn references_outside_xlsx_bounds_are_ref_errors() {
        let sheet = Sheet::new("Bounds");
        for body in ["XFE1", "A1048577", "SUM(A1:XFE1)", "COUNT(A1:A1048577)"] {
            assert_eq!(
                evaluate_formula(&sheet, body, CellAddr::new(0, 0)),
                Value::Error(CalcError::Ref),
                "{body}"
            );
        }
        assert_eq!(
            evaluate_formula(&sheet, "SUM(XFD1048576)", CellAddr::new(0, 0)),
            Value::Number(0.0)
        );
    }

    #[test]
    fn xlsx_round_trip_preserves_aggregate_results_and_limited_formula_source() {
        let mut book = crate::Workbook::new();
        let values = ["10", "", "20", "label", "'0042"];
        for (row, raw) in values.into_iter().enumerate() {
            book.set_cell(CellAddr::new(0, row as u32), raw);
        }
        book.set_cell(CellAddr::new(1, 0), "=AVERAGE(A1:A1048576)");
        book.set_cell(CellAddr::new(1, 1), "=COUNT(A1:A1048576)");
        let limited = format!("={}1{}", "(".repeat(100), ")".repeat(100));
        book.set_cell(CellAddr::new(1, 2), &limited);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("aggregates.xlsx");
        crate::write_xlsx_path(&book, &path).unwrap();
        let restored = crate::load_xlsx_path(&path).unwrap();
        assert_eq!(
            restored.active_sheet().evaluate(CellAddr::new(1, 0)),
            Value::Number(15.0)
        );
        assert_eq!(
            restored.active_sheet().evaluate(CellAddr::new(1, 1)),
            Value::Number(2.0)
        );
        assert_eq!(restored.active_sheet().raw(CellAddr::new(1, 2)), limited);
        assert_eq!(
            restored.active_sheet().evaluate(CellAddr::new(1, 2)),
            Value::Error(CalcError::Num)
        );
    }

    #[test]
    fn aggregates_ignore_blank_and_text_references() {
        let mut sheet = Sheet::new("Data");
        for (row, raw) in [
            (0, "10"),
            (2, "20"),
            (3, "label"),
            (4, "'0042"),
            (5, "=IF(1,\"7\",0)"),
        ] {
            sheet.set_raw(CellAddr::new(0, row), raw);
        }
        sheet.set_raw(CellAddr::new(2, 0), "0");
        sheet.set_raw(CellAddr::new(2, 1), "-5");
        sheet.set_raw(CellAddr::new(2, 2), "'");
        for (body, expected) in [
            ("SUM(A1:A6)", 30.0),
            ("AVERAGE(A1:A6)", 15.0),
            ("COUNT(A1:A6)", 2.0),
            ("MIN(A1:A6)", 10.0),
            ("MAX(A1:A6)", 20.0),
            ("SUM(A4,A5,A6)", 0.0),
            ("COUNT(A4,A5,A6)", 0.0),
            ("MIN(8,A1,3)", 3.0),
            ("MAX(8,A1,3)", 10.0),
            ("SUM(\"7\",A1)", 17.0),
            ("COUNT(\"7\",\"label\",A1)", 2.0),
            ("MIN(B1:B9)", 0.0),
            ("MAX(B1:B9)", 0.0),
            ("SUM(A1:A3,A1)", 40.0),
            ("AVERAGE(C1:C3)", -2.5),
            ("COUNT(C1:C3)", 2.0),
            ("MIN(C1:C3)", -5.0),
            ("MAX(C1:C3)", 0.0),
        ] {
            assert_eq!(
                evaluate_formula(&sheet, body, CellAddr::new(1, 10)),
                Value::Number(expected),
                "{body}"
            );
        }
        assert_eq!(
            evaluate_formula(&sheet, "AVERAGE(B1:B9)", CellAddr::new(1, 10)),
            Value::Error(CalcError::Div0)
        );
    }

    #[test]
    fn count_ignores_errors_in_references_but_direct_errors_propagate() {
        let mut sheet = Sheet::new("Data");
        sheet.set_raw(CellAddr::new(0, 0), "=#N/A");
        sheet.set_raw(CellAddr::new(0, 1), "4");
        let origin = CellAddr::new(1, 0);
        assert_eq!(
            evaluate_formula(&sheet, "COUNT(A1:A2,A1)", origin),
            Value::Number(1.0)
        );
        assert_eq!(
            evaluate_formula(&sheet, "COUNT(#N/A)", origin),
            Value::Error(CalcError::Na)
        );
        for name in ["SUM", "AVERAGE", "MIN", "MAX"] {
            assert_eq!(
                evaluate_formula(&sheet, &format!("{name}(A1:A2)"), origin),
                Value::Error(CalcError::Na)
            );
        }
    }

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
        let empty = Sheet::new("Empty");
        for body in ["SUM(A1:XFD1048576)", "COUNT(A1:A5)"] {
            assert_eq!(
                evaluate_formula(&empty, body, CellAddr::new(0, 0)),
                Value::Error(CalcError::Cycle)
            );
        }
    }
}

#[cfg(test)]
mod literal_tests {
    use super::*;

    #[test]
    fn ref_errors_propagate_but_do_not_evaluate_unused_if_branches() {
        let sheet = Sheet::new("Sheet1");
        let origin = CellAddr::new(0, 0);
        for body in ["#REF!+1", "SUM(#REF!)", "IF(1,#REF!,3)"] {
            assert_eq!(
                evaluate_formula(&sheet, body, origin),
                Value::Error(CalcError::Ref)
            );
        }
        assert_eq!(
            evaluate_formula(&sheet, "IF(0,#REF!,\"日本語 \"\"A1\"\"\")", origin),
            Value::Text("日本語 \"A1\"".into())
        );
    }

    #[test]
    fn imported_error_constants_propagate_and_unused_if_branches_stay_lazy() {
        let sheet = Sheet::new("Sheet1");
        let origin = CellAddr::new(0, 0);
        for error in [
            CalcError::Na,
            CalcError::Null,
            CalcError::Num,
            CalcError::GettingData,
        ] {
            let token = error.to_string();
            for body in [
                token.clone(),
                format!("SUM({token})"),
                format!("IF(1,{token},3)"),
            ] {
                assert_eq!(evaluate_formula(&sheet, &body, origin), Value::Error(error));
            }
            assert_eq!(
                evaluate_formula(&sheet, &format!("IF(0,{token},3)"), origin),
                Value::Number(3.0)
            );
        }
    }
}
