//! Spreadsheet address helpers (A1 notation).

use thiserror::Error;

/// Zero-based column / row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CellAddr {
    pub col: u32,
    pub row: u32,
}

impl CellAddr {
    pub fn new(col: u32, row: u32) -> Self {
        Self { col, row }
    }

    /// Format as A1 (no `$`).
    pub fn to_a1(self) -> String {
        format!("{}{}", col_to_letters(self.col), self.row + 1)
    }
}

/// Inclusive rectangular range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellRange {
    pub start: CellAddr,
    pub end: CellAddr,
}

impl CellRange {
    pub fn contains(self, addr: CellAddr) -> bool {
        let range = self.normalize();
        (range.start.col..=range.end.col).contains(&addr.col)
            && (range.start.row..=range.end.row).contains(&addr.row)
    }

    pub fn normalize(self) -> Self {
        Self {
            start: CellAddr::new(
                self.start.col.min(self.end.col),
                self.start.row.min(self.end.row),
            ),
            end: CellAddr::new(
                self.start.col.max(self.end.col),
                self.start.row.max(self.end.row),
            ),
        }
    }

    pub fn iter(self) -> impl Iterator<Item = CellAddr> {
        let r = self.normalize();
        (r.start.row..=r.end.row)
            .flat_map(move |row| (r.start.col..=r.end.col).map(move |col| CellAddr::new(col, row)))
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AddrError {
    #[error("invalid cell address")]
    Invalid,
}

/// Parse `A1`, `$A$1`, `AA10`.
pub fn parse_a1(input: &str) -> Result<CellAddr, AddrError> {
    let s = input.trim();
    if s.is_empty() {
        return Err(AddrError::Invalid);
    }
    let bytes = s.as_bytes();
    let mut i = 0usize;
    if bytes.get(i) == Some(&b'$') {
        i += 1;
    }
    let col_start = i;
    while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
        i += 1;
    }
    if i == col_start {
        return Err(AddrError::Invalid);
    }
    let col = letters_to_col(&s[col_start..i])?;
    if bytes.get(i) == Some(&b'$') {
        i += 1;
    }
    let row_start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == row_start || i != bytes.len() {
        return Err(AddrError::Invalid);
    }
    let row_num: u32 = s[row_start..i].parse().map_err(|_| AddrError::Invalid)?;
    if row_num == 0 {
        return Err(AddrError::Invalid);
    }
    Ok(CellAddr::new(col, row_num - 1))
}

/// Parse `A1:B2` or a single `A1` (degenerate range).
pub fn parse_a1_range(input: &str) -> Result<CellRange, AddrError> {
    let s = input.trim();
    if let Some((a, b)) = s.split_once(':') {
        Ok(CellRange {
            start: parse_a1(a)?,
            end: parse_a1(b)?,
        }
        .normalize())
    } else {
        let addr = parse_a1(s)?;
        Ok(CellRange {
            start: addr,
            end: addr,
        })
    }
}

pub fn col_to_letters(mut col: u32) -> String {
    let mut out = Vec::new();
    loop {
        let rem = (col % 26) as u8;
        out.push(b'A' + rem);
        if col < 26 {
            break;
        }
        col = col / 26 - 1;
    }
    out.reverse();
    String::from_utf8(out).unwrap()
}

fn letters_to_col(letters: &str) -> Result<u32, AddrError> {
    if letters.is_empty() {
        return Err(AddrError::Invalid);
    }
    let mut n: u32 = 0;
    for c in letters.chars() {
        if !c.is_ascii_alphabetic() {
            return Err(AddrError::Invalid);
        }
        let v = (c.to_ascii_uppercase() as u8 - b'A') as u32;
        n = n
            .checked_mul(26)
            .and_then(|x| x.checked_add(v + 1))
            .ok_or(AddrError::Invalid)?;
    }
    Ok(n - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a1_round_trip() {
        assert_eq!(parse_a1("A1").unwrap(), CellAddr::new(0, 0));
        assert_eq!(parse_a1("$B$2").unwrap(), CellAddr::new(1, 1));
        assert_eq!(parse_a1("AA10").unwrap(), CellAddr::new(26, 9));
        assert_eq!(CellAddr::new(0, 0).to_a1(), "A1");
        assert_eq!(CellAddr::new(26, 9).to_a1(), "AA10");
        assert_eq!(col_to_letters(0), "A");
        assert_eq!(col_to_letters(25), "Z");
        assert_eq!(col_to_letters(26), "AA");
    }

    #[test]
    fn range_parse() {
        let r = parse_a1_range("B2:A1").unwrap();
        assert_eq!(r.start, CellAddr::new(0, 0));
        assert_eq!(r.end, CellAddr::new(1, 1));
        assert_eq!(r.iter().count(), 4);
    }
}
