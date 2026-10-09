use office_calc::{load_xlsx_path, write_xlsx_path, CalcError, CellAddr, Sheet, Value, Workbook};
fn addr(col: u32, row: u32) -> CellAddr {
    CellAddr::new(col, row)
}
fn book() -> Workbook {
    let mut book = Workbook::new();
    book.sheets[0].name = "Summary".into();
    let mut sheet = Sheet::new("Q1 日本語's");
    sheet.set_raw(addr(0, 0), "10");
    sheet.set_raw(addr(0, 1), "20");
    sheet.set_raw(addr(0, 2), "'ignore");
    book.sheets.push(sheet);
    book
}
#[test]
fn quoted_ranges_anchors_and_nested_local_refs_recalculate() {
    let mut book = book();
    for (col, formula, expected) in [
        (0, "=SUM('Q1 日本語''s'!$A$1:A3)", 30.0),
        (1, "=AVERAGE('Q1 日本語''s'!A1:A3)", 15.0),
        (2, "=COUNT('Q1 日本語''s'!A1:A3)", 2.0),
    ] {
        book.sheets[0].set_raw(addr(col, 0), formula);
        assert_eq!(book.evaluate(0, addr(col, 0)), Value::Number(expected));
    }
    book.sheets[1].set_raw(addr(0, 1), "=A1*3");
    assert_eq!(book.display(0, addr(0, 0)), "40");
    book.sheets[0].set_raw(addr(3, 0), "=sUmMaRy!A1+1");
    assert_eq!(book.evaluate(0, addr(3, 0)), Value::Number(41.0));
}
#[test]
fn cycles_missing_sheets_and_limits_span_all_sheets() {
    let mut book = book();
    book.sheets[0].set_raw(addr(0, 0), "='Q1 日本語''s'!A1");
    book.sheets[1].set_raw(addr(0, 0), "=Summary!A1");
    assert_eq!(book.evaluate(0, addr(0, 0)), Value::Error(CalcError::Cycle));
    book.sheets[0].set_raw(addr(0, 0), "=Missing!A1");
    assert_eq!(book.evaluate(0, addr(0, 0)), Value::Error(CalcError::Ref));
    for row in 0..100 {
        let name = if row % 2 == 0 {
            "'Q1 日本語''s'"
        } else {
            "Summary"
        };
        book.sheets[(row % 2) as usize].set_raw(addr(0, row), format!("={name}!A{}", row + 2));
    }
    assert_eq!(book.evaluate(0, addr(0, 0)), Value::Error(CalcError::Num));
}
#[test]
fn rename_delete_and_history_preserve_references_and_string_literals() {
    let mut book = book();
    book.sheets[0].set_raw(addr(0, 0), "=SUM('Q1 日本語''s'!A1:A3)");
    book.sheets[0].set_raw(addr(1, 0), "=IF(1,\"Q1 日本語's!A1\",0)");
    let formula = book.sheets[0].raw(addr(0, 0)).to_string();
    book.rename_sheet(1, "新しい名前").unwrap();
    assert_eq!(book.display(0, addr(0, 0)), "30");
    assert_eq!(
        book.sheets[0].raw(addr(1, 0)),
        "=IF(1,\"Q1 日本語's!A1\",0)"
    );
    assert!(book.undo());
    assert_eq!(book.sheets[0].raw(addr(0, 0)), formula);
    assert!(book.redo());
    book.delete_sheet(1).unwrap();
    assert_eq!(book.sheets[0].raw(addr(0, 0)), "=SUM(#REF!)");
    assert_eq!(book.evaluate(0, addr(0, 0)), Value::Error(CalcError::Ref));
    assert!(book.undo());
    assert_eq!(book.display(0, addr(0, 0)), "30");
    assert!(book.redo());
    book.add_sheet("新しい名前");
    assert_eq!(book.evaluate(0, addr(0, 0)), Value::Error(CalcError::Ref));
}
#[test]
fn xlsx_roundtrip_preserves_cross_sheet_formulas() {
    let mut book = book();
    book.sheets[0].set_raw(addr(0, 0), "=SUM('Q1 日本語''s'!$A$1:A3)");
    let path = std::env::temp_dir().join(format!("cross-sheet-{}.xlsx", std::process::id()));
    write_xlsx_path(&book, &path).unwrap();
    let restored = load_xlsx_path(&path).unwrap();
    assert_eq!(
        restored.sheets[0].raw(addr(0, 0)),
        book.sheets[0].raw(addr(0, 0))
    );
    assert_eq!(restored.display(0, addr(0, 0)), "30");
    std::fs::remove_file(path).unwrap();
}
