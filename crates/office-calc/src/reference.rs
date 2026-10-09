//! Translate A1 references without changing formula strings or identifier names.

use crate::{col_to_letters, parse_a1, CellAddr, MAX_SHEET_COLS, MAX_SHEET_ROWS};

/// Adjust relative A1 references when copying a formula between cells.
/// Fixed axes retain `$`. References moved outside XLSX bounds become `#REF!`.
/// Quoted strings, sheet names, structured references and function names remain
/// unchanged. The original spacing and operators are preserved.
pub fn translate_formula(raw: &str, source: CellAddr, destination: CellAddr) -> String {
    let dx = i64::from(destination.col) - i64::from(source.col);
    let dy = i64::from(destination.row) - i64::from(source.row);
    if !raw.trim_start().starts_with('=') || (dx == 0 && dy == 0) {
        return raw.to_owned();
    }
    let mut output = String::with_capacity(raw.len());
    let mut cursor = 0;
    while cursor < raw.len() {
        let ch = raw[cursor..].chars().next().unwrap();
        if ch == '"' || ch == '\'' {
            let end = quoted_end(raw, cursor, ch);
            output.push_str(&raw[cursor..end]);
            cursor = end;
        } else if ch == '[' {
            let mut depth = 0;
            let mut end = raw.len();
            for (offset, ch) in raw[cursor..].char_indices() {
                if ch == '[' {
                    depth += 1;
                }
                if ch == ']' {
                    depth -= 1;
                    if depth == 0 {
                        end = cursor + offset + 1;
                        break;
                    }
                }
            }
            output.push_str(&raw[cursor..end]);
            cursor = end;
        } else if ch == '#' {
            let end = token_end(raw, cursor, |ch| {
                ch.is_ascii_alphanumeric() || "#/!?_.".contains(ch)
            });
            output.push_str(&raw[cursor..end]);
            cursor = end;
        } else if identifier_char(ch) {
            let end = token_end(raw, cursor, identifier_char);
            let token = &raw[cursor..end];
            let next = skip_space(raw, end);
            let reference = Reference::parse(token);
            // A1-shaped names such as LOG10(...) or Sheet1! are not cell refs.
            if reference.is_none() || matches!(raw.as_bytes().get(next), Some(b'(' | b'!' | b'[')) {
                output.push_str(token);
                cursor = end;
                continue;
            }
            let shifted = reference.unwrap().shifted(token, dx, dy);
            if raw.as_bytes().get(next) == Some(&b':') {
                let second_start = skip_space(raw, next + 1);
                let second_end = token_end(raw, second_start, identifier_char);
                let second = &raw[second_start..second_end];
                if let Some(reference) = Reference::parse(second).filter(|_| {
                    !matches!(
                        raw.as_bytes().get(skip_space(raw, second_end)),
                        Some(b'(' | b'!' | b'[')
                    )
                }) {
                    match (shifted, reference.shifted(second, dx, dy)) {
                        (Some(first), Some(second)) => {
                            output.push_str(&first);
                            output.push_str(&raw[end..second_start]);
                            output.push_str(&second);
                        }
                        _ => output.push_str("#REF!"),
                    }
                    cursor = second_end;
                    continue;
                }
            }
            output.push_str(shifted.as_deref().unwrap_or("#REF!"));
            cursor = end;
        } else {
            output.push(ch);
            cursor += ch.len_utf8();
        }
    }
    output
}

fn identifier_char(ch: char) -> bool {
    ch.is_alphanumeric() || matches!(ch, '_' | '.' | '$' | '\\')
}

fn token_end(raw: &str, start: usize, accept: impl Fn(char) -> bool) -> usize {
    raw[start..]
        .char_indices()
        .find_map(|(offset, ch)| (!accept(ch)).then_some(start + offset))
        .unwrap_or(raw.len())
}

fn skip_space(raw: &str, start: usize) -> usize {
    token_end(raw, start, char::is_whitespace)
}

fn quoted_end(raw: &str, start: usize, quote: char) -> usize {
    let mut chars = raw[start + 1..].char_indices().peekable();
    while let Some((offset, ch)) = chars.next() {
        if ch == quote {
            if chars.peek().is_some_and(|(_, ch)| *ch == quote) {
                chars.next();
            } else {
                return start + offset + 2;
            }
        }
    }
    raw.len()
}

struct Reference {
    addr: CellAddr,
    fixed_col: bool,
    fixed_row: bool,
}

impl Reference {
    fn parse(token: &str) -> Option<Self> {
        let addr = parse_a1(token).ok()?;
        // Out-of-sheet A1-shaped tokens may be defined names in imported files.
        if addr.col >= MAX_SHEET_COLS || addr.row >= MAX_SHEET_ROWS {
            return None;
        }
        let fixed_col = token.starts_with('$');
        let fixed_row = token[usize::from(fixed_col)..].contains('$');
        Some(Self {
            addr,
            fixed_col,
            fixed_row,
        })
    }

    fn shifted(&self, original: &str, dx: i64, dy: i64) -> Option<String> {
        let col = i64::from(self.addr.col) + if self.fixed_col { 0 } else { dx };
        let row = i64::from(self.addr.row) + if self.fixed_row { 0 } else { dy };
        if !(0..i64::from(MAX_SHEET_COLS)).contains(&col)
            || !(0..i64::from(MAX_SHEET_ROWS)).contains(&row)
        {
            return None;
        }
        if col == i64::from(self.addr.col) && row == i64::from(self.addr.row) {
            return Some(original.to_owned());
        }
        let mut letters = col_to_letters(col as u32);
        if original
            .chars()
            .filter(char::is_ascii_alphabetic)
            .all(|ch| ch.is_ascii_lowercase())
        {
            letters.make_ascii_lowercase();
        }
        Some(format!(
            "{}{}{}{}",
            if self.fixed_col { "$" } else { "" },
            letters,
            if self.fixed_row { "$" } else { "" },
            row + 1
        ))
    }
}

/// Rewrite only worksheet qualifiers, retaining strings, local refs and anchors.
pub(crate) fn rename_sheet_references(raw: &str, old: &str, new: &str) -> String {
    rewrite_sheet_references(raw, old, Some(new))
}
pub(crate) fn rewrite_sheet_references(raw: &str, old: &str, new: Option<&str>) -> String {
    if !raw.trim_start().starts_with('=') { return raw.to_owned(); }
    let mut output=String::new();let mut cursor=0;
    while cursor<raw.len() {
        let ch=raw[cursor..].chars().next().unwrap();
        if ch=='"' {
            let end=quoted_end(raw,cursor,ch);output.push_str(&raw[cursor..end]);cursor=end;continue;
        }
        if ch=='\'' || identifier_char(ch) {
            let end=if ch=='\'' {quoted_end(raw,cursor,ch)} else {token_end(raw,cursor,identifier_char)};
            let token=&raw[cursor..end];let next=skip_space(raw,end);
            let name=if ch=='\'' && token.ends_with('\'') { token[1..token.len()-1].replace("''","'") } else {token.to_string()};
            if raw.as_bytes().get(next)==Some(&b'!') && name.to_lowercase()==old.to_lowercase()
                && raw.as_bytes().get(cursor.wrapping_sub(1))!=Some(&b']') {
                if let Some(new) = new {
                    output.push('\'');output.push_str(&new.replace('\'',"''"));output.push('\'');
                } else {
                    let first=skip_space(raw,next+1);
                    let first_end=token_end(raw,first,identifier_char);
                    if Reference::parse(&raw[first..first_end]).is_some() {
                        let mut end=first_end;let colon=skip_space(raw,end);
                        if raw.as_bytes().get(colon)==Some(&b':') {
                            let last=skip_space(raw,colon+1);let last_end=token_end(raw,last,identifier_char);
                            if Reference::parse(&raw[last..last_end]).is_some() {end=last_end;}
                        }
                        output.push_str("#REF!");cursor=end;continue;
                    }
                    output.push_str(token);
                }
            } else {output.push_str(token);}
            cursor=end;
        } else {output.push(ch);cursor+=ch.len_utf8();}
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_fixed_and_mixed_axes_shift_in_both_directions() {
        let from = CellAddr::new(3, 1);
        let to = CellAddr::new(5, 4);
        let original = "  = B2*C2 + $B$2 + $B2 + B$2 + aa10";
        let moved = "  = D5*E5 + $B$2 + $B5 + D$2 + ac13";
        assert_eq!(translate_formula(original, from, to), moved);
        assert_eq!(translate_formula(moved, to, from), original);
        assert_eq!(translate_formula(original, from, from), original);
        assert_eq!(translate_formula("text A1", from, to), "text A1");
    }

    #[test]
    fn ranges_preserve_endpoint_order_spacing_and_fixed_axes() {
        assert_eq!(
            translate_formula(
                "=SUM( A1 : $B$3 , C$3:$D4, B3:A1)",
                CellAddr::new(0, 0),
                CellAddr::new(1, 2)
            ),
            "=SUM( B3 : $B$3 , D$3:$D6, C5:B3)"
        );
    }

    #[test]
    fn strings_names_functions_and_scientific_numbers_are_not_references() {
        let raw =
            "=IF(A1,\"日本語 A1 \"\"B2\"\"\",LOG10(B1))+1E10+1e-3+A1_name+日本A1+XFE1+Table1[A1]";
        assert_eq!(
            translate_formula(raw, CellAddr::new(0, 0), CellAddr::new(1, 1)),
            "=IF(B2,\"日本語 A1 \"\"B2\"\"\",LOG10(C2))+1E10+1e-3+A1_name+日本A1+XFE1+Table1[A1]"
        );
    }

    #[test]
    fn sheet_qualifiers_remain_unchanged_while_their_cell_refs_move() {
        assert_eq!(
            translate_formula(
                "=Sheet1!A1+'Sheet A1''s'!$B2+[Book1.xlsx]Sheet2!C$3+#REF!",
                CellAddr::new(0, 0),
                CellAddr::new(1, 1)
            ),
            "=Sheet1!B2+'Sheet A1''s'!$B3+[Book1.xlsx]Sheet2!D$3+#REF!"
        );
    }

    #[test]
    fn out_of_bounds_refs_and_ranges_become_ref_errors() {
        assert_eq!(
            translate_formula(
                "=A1+$A$1+SUM(A1:B2)+B$2",
                CellAddr::new(1, 1),
                CellAddr::new(0, 0)
            ),
            "=#REF!+$A$1+SUM(#REF!)+A$2"
        );
        assert_eq!(
            translate_formula(
                "=XFD1048576+$XFD$1048576",
                CellAddr::new(0, 0),
                CellAddr::new(1, 1)
            ),
            "=#REF!+$XFD$1048576"
        );
    }
}
