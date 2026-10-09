//! Read supported XLSX presentation without relying on worksheet ZIP ordering.
use crate::{parse_a1, CellFormat, Workbook, MAX_SHEET_COLS, MAX_SHEET_ROWS};
use quick_xml::{events::Event, name::ResolveResult, reader::NsReader};
use std::{collections::HashMap, io::{Cursor, Read}};

const MAIN: &[u8] = b"http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const RELS: &[u8] = b"http://schemas.openxmlformats.org/package/2006/relationships";
const DOCUMENT_RELS: &[u8] = b"http://schemas.openxmlformats.org/officeDocument/2006/relationships";
type Attributes = HashMap<String, String>;
type Archive<'a> = zip2::ZipArchive<Cursor<&'a [u8]>>;

fn part(archive: &mut Archive<'_>, path: &str) -> Result<String, String> {
    let mut file = archive.by_name(path).map_err(|e| format!("{path}: {e}"))?;
    if file.size() > 8 * 1024 * 1024 {
        return Err(format!("{path}: presentation XML exceeds 8 MiB"));
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(8 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err(format!("{path}: presentation XML exceeds 8 MiB"));
    }
    String::from_utf8(bytes).map_err(|e| format!("{path}: {e}"))
}

fn scan(
    xml: &str,
    namespace: &[u8],
    root: &str,
    mut visit: impl FnMut(&str, &str, &Attributes) -> Result<(), String>,
) -> Result<(), String> {
    let mut reader = NsReader::from_str(xml);
    let mut stack = Vec::<String>::new();
    let mut seen_root = false;
    loop {
        let (ns, event) = reader.read_resolved_event().map_err(|e| e.to_string())?;
        match event {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let matched = matches!(ns, ResolveResult::Bound(n) if n.as_ref() == namespace);
                if matches!(ns, ResolveResult::Unknown(_)) {
                    return Err("Undeclared XML namespace".into());
                }
                let name = std::str::from_utf8(e.local_name().as_ref())
                    .map_err(|e| e.to_string())?
                    .to_owned();
                if stack.is_empty() {
                    if seen_root || !matched || name != root {
                        return Err(format!("Expected one {root} root"));
                    }
                    seen_root = true;
                }
                let mut attrs = Attributes::new();
                for attr in e.attributes() {
                    let attr = attr.map_err(|e| e.to_string())?;
                    let (ns, key) = reader.resolve_attribute(attr.key);
                    let key = match ns {
                        ResolveResult::Unbound => std::str::from_utf8(key.as_ref())
                            .map_err(|e| e.to_string())?
                            .to_owned(),
                        ResolveResult::Bound(n)
                            if n.as_ref() == DOCUMENT_RELS && key.as_ref() == b"id" =>
                        {
                            "relationship_id".into()
                        }
                        ResolveResult::Unknown(_) => {
                            return Err("Undeclared attribute namespace".into())
                        }
                        _ => continue,
                    };
                    attrs.insert(
                        key,
                        attr.decode_and_unescape_value(reader.decoder())
                            .map_err(|e| e.to_string())?
                            .into_owned(),
                    );
                }
                if matched {
                    visit(&name, stack.last().map_or("", String::as_str), &attrs)?;
                }
                if matches!(event, Event::Start(_)) {
                    stack.push(if matched { name } else { String::new() });
                }
            }
            Event::End(_) => {
                stack.pop().ok_or("Unexpected XML end")?;
            }
            Event::Text(e) => {
                let text = e.unescape().map_err(|e| e.to_string())?;
                if stack.is_empty() && !text.trim().is_empty() {
                    return Err("Text outside XML root".into());
                }
            }
            Event::DocType(_) => return Err("XML DTD is unsupported".into()),
            Event::Eof => {
                if !seen_root || !stack.is_empty() {
                    return Err("Incomplete XML".into());
                }
                return Ok(());
            }
            _ => {}
        }
    }
}

fn target_path(target: &str) -> Result<String, String> {
    if target.contains(['\\', ':', '?', '#', '%']) {
        return Err(format!("Unsupported XLSX relationship target: {target}"));
    }
    let source = if target.starts_with('/') {
        target.to_owned()
    } else {
        format!("xl/{target}")
    };
    let mut parts = Vec::new();
    for part in source.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop().ok_or("Relationship escapes package root")?;
            }
            _ => parts.push(part),
        }
    }
    Ok(parts.join("/"))
}

fn integer(attrs: &Attributes, key: &str, default: usize) -> Result<usize, String> {
    attrs.get(key).map_or(Ok(default), |value| {
        value.parse().map_err(|_| format!("Invalid {key}: {value}"))
    })
}

fn builtin(id: usize) -> Option<&'static str> {
    Some(match id {
        0 => "General",
        1 => "0",
        2 => "0.00",
        3 => "#,##0",
        4 => "#,##0.00",
        9 => "0%",
        10 => "0.00%",
        11 => "0.00E+00",
        12 => "# ?/?",
        13 => "# ??/??",
        14 => "mm-dd-yy",
        15 => "d-mmm-yy",
        16 => "d-mmm",
        17 => "mmm-yy",
        18 => "h:mm AM/PM",
        19 => "h:mm:ss AM/PM",
        20 => "h:mm",
        21 => "h:mm:ss",
        22 => "m/d/yy h:mm",
        37 => "#,##0 ;(#,##0)",
        38 => "#,##0 ;[Red](#,##0)",
        39 => "#,##0.00;(#,##0.00)",
        40 => "#,##0.00;[Red](#,##0.00)",
        45 => "mm:ss",
        46 => "[h]:mm:ss",
        47 => "mmss.0",
        48 => "##0.0E+0",
        49 => "@",
        _ => return None,
    })
}

pub(crate) fn load(bytes: &[u8], workbook: &mut Workbook) -> Result<(), String> {
    let mut archive = zip2::ZipArchive::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let mut copied_text = 0usize;
    let mut entries = 0usize;
    let mut sheets = Vec::new();
    scan(
        &part(&mut archive, "xl/workbook.xml")?,
        MAIN,
        "workbook",
        |name, parent, attrs| {
            if name == "workbookPr"
                && parent == "workbook"
                && attrs
                    .get("date1904")
                    .is_some_and(|v| v == "1" || v == "true")
            {
                return Err("The XLSX 1904 date system is not supported; import cancelled to preserve date values".into());
            }
            if name == "sheet" && parent == "sheets" {
                sheets.push((
                    attrs.get("name").ok_or("Missing sheet name")?.clone(),
                    attrs
                        .get("relationship_id")
                        .ok_or("Missing worksheet relationship")?
                        .clone(),
                ));
            }
            Ok(())
        },
    )?;
    let mut relationships = HashMap::new();
    let mut styles_path = None;
    scan(
        &part(&mut archive, "xl/_rels/workbook.xml.rels")?,
        RELS,
        "Relationships",
        |name, parent, attrs| {
            if name == "Relationship" && parent == "Relationships" {
                let kind = attrs.get("Type").map_or("", String::as_str);
                if kind.ends_with("/worksheet") || kind.ends_with("/styles") {
                    if attrs.get("TargetMode").is_some_and(|v| v == "External") {
                        return Err("External worksheet/styles relationship".into());
                    }
                    let path =
                        target_path(attrs.get("Target").ok_or("Missing relationship target")?)?;
                    if kind.ends_with("/styles") {
                        styles_path = Some(path);
                    } else {
                        relationships.insert(
                            attrs.get("Id").ok_or("Missing relationship ID")?.clone(),
                            path,
                        );
                    }
                }
            }
            Ok(())
        },
    )?;
    let mut formats = vec![CellFormat::default()];
    if let Some(path) = styles_path {
        formats.clear();
        let mut fonts: Vec<(bool, bool)> = Vec::new();
        let mut numbers = HashMap::new();
        scan(
            &part(&mut archive, &path)?,
            MAIN,
            "styleSheet",
            |name, parent, attrs| {
                match (name, parent) {
                    ("font", "fonts") => fonts.push((false, false)),
                    ("b" | "i", "font") => {
                        let value = !attrs.get("val").is_some_and(|v| v == "0" || v == "false");
                        if let Some(font) = fonts.last_mut() {
                            if name == "b" {
                                font.0 = value;
                            } else {
                                font.1 = value;
                            }
                        }
                    }
                    ("numFmt", "numFmts") => {
                        let code = attrs
                            .get("formatCode")
                            .ok_or("Missing number format code")?;
                        numbers.insert(integer(attrs, "numFmtId", 0)?, code.clone());
                    }
                    ("xf", "cellXfs") => {
                        let id = integer(attrs, "numFmtId", 0)?;
                        let (code, builtin_number_format) = if let Some(code) = numbers.get(&id) {
                            copied_text = copied_text.saturating_add(code.len());
                            if copied_text > crate::xlsx_limits::MAX_TEXT_BYTES {
                                return Err("XLSX presentation text exceeds 32 MiB".into());
                            }
                            (code.clone(), None)
                        } else if let Some(code) = builtin(id) {
                            (code.to_owned(), None)
                        } else if id < 164 {
                            ("General".into(), Some(id as u8))
                        } else {
                            return Err(format!("Missing number format ID {id}"));
                        };
                        let font_id = integer(attrs, "fontId", 0)?;
                        let &(bold, italic) = fonts
                            .get(font_id)
                            .ok_or("Missing font referenced by cell format")?;
                        if formats.len() >= crate::xlsx_limits::MAX_CELLS {
                            return Err("XLSX exceeds the style count limit".into());
                        }
                        formats.push(CellFormat {
                            number_format: code,
                            builtin_number_format,
                            bold,
                            italic,
                        });
                    }
                    _ => {}
                }
                Ok(())
            },
        )?;
    }
    for sheet in &mut workbook.sheets {
        let (_, id) = sheets
            .iter()
            .find(|(name, _)| name == &sheet.name)
            .ok_or("Missing worksheet in workbook XML")?;
        let path = relationships
            .get(id)
            .ok_or("Missing worksheet relationship")?;
        scan(
            &part(&mut archive, path)?,
            MAIN,
            "worksheet",
            |name, parent, attrs| {
                use crate::DimensionAxis;
                let size = |key: &str| -> Result<f64, String> {
                    attrs.get(key).ok_or_else(|| format!("Missing {key}"))?.parse::<f64>().map_err(|_| format!("Invalid {key}"))
                };
                let hidden = |key: &str| attrs.get(key).is_some_and(|v| v == "1" || v == "true");
                if (name == "row" || name == "col") && hidden("hidden") || name == "sheetFormatPr" && hidden("zeroHeight") {
                    return Err("Hidden rows/columns are not supported; import cancelled to preserve worksheet layout".into());
                }
                if name == "sheetFormatPr" && parent == "worksheet" {
                    if attrs.contains_key("defaultRowHeight") {
                        let height = size("defaultRowHeight")?;
                        DimensionAxis::Rows.validate(height).map_err(|e| e.to_string())?;
                        sheet.dimensions.default_row = height;
                    }
                    if attrs.contains_key("defaultColWidth") {
                        let width = (size("defaultColWidth")? * 7.0).round();
                        DimensionAxis::Columns.validate(width).map_err(|e| e.to_string())?;
                        sheet.dimensions.default_column = width;
                    }
                }
                if name == "col" && parent == "cols" && attrs.contains_key("width") {
                    let first = integer(attrs, "min", 0)?;
                    let last = integer(attrs, "max", 0)?;
                    if first == 0 || first > last || last > MAX_SHEET_COLS as usize { return Err("Invalid column-size range".into()); }
                    // XLSX column widths use a Calibri-11 digit width of seven pixels.
                    let width = (size("width")? * 7.0).round();
                    DimensionAxis::Columns.validate(width).map_err(|e| e.to_string())?;
                    entries = entries.saturating_add(last - first + 1);
                    if entries > crate::xlsx_limits::MAX_CELLS {
                        return Err("XLSX presentation exceeds 100000 entries".into());
                    }
                    for col in first - 1..last { sheet.set_dimension(DimensionAxis::Columns, col as u32, width); }
                }
                if name == "row" && parent == "sheetData" && attrs.contains_key("ht") {
                    let row = integer(attrs, "r", 0)?;
                    if row == 0 || row > MAX_SHEET_ROWS as usize { return Err("Invalid row-size index".into()); }
                    let height = size("ht")?;
                    DimensionAxis::Rows.validate(height).map_err(|e| e.to_string())?;
                    entries = entries.saturating_add(1);
                    if entries > crate::xlsx_limits::MAX_CELLS {
                        return Err("XLSX presentation exceeds 100000 entries".into());
                    }
                    sheet.set_dimension(DimensionAxis::Rows, row as u32 - 1, height);
                }
                if name == "c" && parent == "row" {
                    let style = integer(attrs, "s", 0)?;
                    let format = formats
                        .get(style)
                        .ok_or_else(|| format!("Missing cell style index {style}"))?;
                    if format != &CellFormat::default() {
                        let addr = parse_a1(attrs.get("r").ok_or("Styled cell lacks address")?)
                            .map_err(|e| e.to_string())?;
                        if addr.col >= MAX_SHEET_COLS || addr.row >= MAX_SHEET_ROWS {
                            return Err("Styled cell exceeds XLSX worksheet bounds".into());
                        }
                        copied_text = copied_text.saturating_add(format.number_format.len());
                        entries = entries.saturating_add(1);
                        if copied_text > crate::xlsx_limits::MAX_TEXT_BYTES || entries > crate::xlsx_limits::MAX_CELLS {
                            return Err("XLSX presentation exceeds entry or text limits".into());
                        }
                        sheet.set_format(addr, format.clone());
                    }
                }
                Ok(())
            },
        )
        .map_err(|e| format!("{path}: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{load_xlsx_path, write_xlsx_bytes, CellAddr, CellRange, FormatChange};
    use std::io::{Cursor, Write};

    fn fixture() -> Workbook {
        let mut book = Workbook::new();
        book.paste_tsv(CellAddr::new(0, 0), "0.125\t=A1*2\t25569\t'0.125")
            .unwrap();
        for (col, code) in [(0, "0.00%"), (1, "0.00"), (2, "yyyy-mm-dd"), (3, "0.00%")] {
            let addr = CellAddr::new(col, 0);
            book.format_range(
                CellRange {
                    start: addr,
                    end: addr,
                },
                FormatChange::Number(code.into()),
            )
            .unwrap();
        }
        for change in [FormatChange::Bold(true), FormatChange::Italic(true)] {
            book.format_range(
                CellRange {
                    start: CellAddr::new(0, 0),
                    end: CellAddr::new(4, 1),
                },
                change,
            )
            .unwrap();
        }
        book.add_sheet("別表");
        book.set_cell(CellAddr::new(0, 0), "1.2345678901234567");
        book
    }

    fn reload(bytes: &[u8]) -> Result<Workbook, crate::XlsxError> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("styles.xlsx");
        std::fs::write(&path, bytes).unwrap();
        load_xlsx_path(&path)
    }

    fn rewrite(bytes: &[u8], mut change: impl FnMut(&str, String) -> (String, String)) -> Vec<u8> {
        let mut input = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut output = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for i in (0..input.len()).rev() {
            let mut file = input.by_index(i).unwrap();
            let mut xml = String::new();
            file.read_to_string(&mut xml).unwrap();
            let (name, xml) = change(file.name(), xml);
            output
                .start_file(name, zip::write::FileOptions::default())
                .unwrap();
            output.write_all(xml.as_bytes()).unwrap();
        }
        output.finish().unwrap().into_inner()
    }

    #[test]
    fn repeated_format_codes_are_bounded_before_copying() {
        let bytes = write_xlsx_bytes(&fixture()).unwrap();
        for repeat_definitions in [true, false] {
            let code = "0".repeat(1024 * 1024);
            let definitions = if repeat_definitions { 40 } else { 1 };
            let styles = format!("<styleSheet xmlns=\"{}\"><numFmts><numFmt numFmtId=\"164\" formatCode=\"{code}\"/></numFmts><fonts><font/></fonts><cellXfs>{}</cellXfs></styleSheet>",
                std::str::from_utf8(MAIN).unwrap(), "<xf numFmtId=\"164\" fontId=\"0\"/>".repeat(definitions));
            let cells = (1..=40).map(|row| format!("<row r=\"{row}\"><c r=\"A{row}\" s=\"0\"/></row>"))
                .collect::<String>();
            let worksheet = format!("<worksheet xmlns=\"{}\"><sheetData>{cells}</sheetData></worksheet>",
                std::str::from_utf8(MAIN).unwrap());
            let changed = rewrite(&bytes, |name, xml| {
                let xml = if name == "xl/styles.xml" { styles.clone() }
                    else if name.starts_with("xl/worksheets/") { worksheet.clone() } else { xml };
                (name.into(), xml)
            });
            crate::xlsx_limits::package(&changed).unwrap();
            assert!(load(&changed, &mut fixture()).unwrap_err().contains("text"));
        }
    }

    #[test]
    fn xlsx_round_trips_pixel_widths_point_heights_blank_dimensions_and_defaults() {
        use crate::DimensionAxis::*;
        let mut book = fixture();
        book.set_active_sheet(0);
        for (col, width) in [1.0, 7.0, 12.0, 88.0, 1790.0].into_iter().enumerate() {
            book.resize_range(Columns, col as u32, col as u32, Some(width))
                .unwrap();
        }
        book.resize_range(Rows, 0, 1, Some(31.5)).unwrap();
        book.resize_range(Rows, 100, 100, Some(409.5)).unwrap();
        book.set_active_sheet(1);
        book.resize_range(Columns, 1, 1, Some(120.0)).unwrap();
        let restored = reload(&write_xlsx_bytes(&book).unwrap()).unwrap();
        for index in 0..2 {
            for col in 0..6 {
                assert_eq!(
                    restored.sheets[index].column_width(col),
                    book.sheets[index].column_width(col)
                );
            }
            for row in [0, 1, 100] {
                assert_eq!(
                    restored.sheets[index].row_height(row),
                    book.sheets[index].row_height(row)
                );
            }
        }
        assert_eq!(restored.sheets[0].rows, 101);
        assert_eq!(restored.sheets[0].raw(crate::CellAddr::new(1, 0)), "=A1*2");
        assert!(restored.sheets[0].format(crate::CellAddr::new(0, 0)).bold);
        assert!(!restored.is_dirty() && !restored.can_undo());
        let again = reload(&write_xlsx_bytes(&restored).unwrap()).unwrap();
        assert_eq!(again.sheets[0].column_width(4), 1790.0);
        assert_eq!(again.sheets[0].row_height(100), 409.5);
    }

    #[test]
    fn imported_sheet_defaults_remain_defaults_after_export() {
        let bytes = rewrite(&write_xlsx_bytes(&Workbook::new()).unwrap(), |name, xml| {
            let xml = if name == "xl/worksheets/sheet1.xml" {
                xml.replace("defaultRowHeight=\"18\"", "defaultRowHeight=\"25.5\"")
                    .replace(
                        "defaultColWidth=\"12.571428571428571\"",
                        "defaultColWidth=\"20\"",
                    )
            } else {
                xml
            };
            (name.into(), xml)
        });
        let mut loaded = reload(&bytes).unwrap();
        assert_eq!(
            loaded
                .active_sheet()
                .default_dimension(crate::DimensionAxis::Rows),
            25.5
        );
        assert_eq!(
            loaded
                .active_sheet()
                .default_dimension(crate::DimensionAxis::Columns),
            140.0
        );
        loaded
            .resize_range(crate::DimensionAxis::Columns, 0, 0, Some(200.0))
            .unwrap();
        loaded
            .resize_range(crate::DimensionAxis::Columns, 0, 0, None)
            .unwrap();
        let again = reload(&write_xlsx_bytes(&loaded).unwrap()).unwrap();
        assert_eq!(
            again
                .active_sheet()
                .default_dimension(crate::DimensionAxis::Columns),
            140.0
        );
        assert_eq!(again.active_sheet().column_width(0), 140.0);
        assert_eq!(again.active_sheet().row_height(2), 25.5);
    }

    #[test]
    fn invalid_size_ranges_non_finite_sizes_and_hidden_rows_fail_the_whole_import() {
        let mut book = Workbook::new();
        book.resize_range(crate::DimensionAxis::Rows, 0, 0, Some(30.0))
            .unwrap();
        let bytes = write_xlsx_bytes(&book).unwrap();
        for case in 0..5 {
            let broken = rewrite(&bytes, |name, mut xml| {
                if name == "xl/worksheets/sheet1.xml" {
                    xml = match case {
                        0 => xml.replace("min=\"1\"", "min=\"0\""),
                        1 => xml.replace("ht=\"30\"", "ht=\"NaN\""),
                        2 => xml.replace("ht=\"30\"", "ht=\"410\""),
                        3 => xml.replace("ht=\"30\"", "ht=\"30\" hidden=\"1\""),
                        _ => xml.replace("width=\"12.5703125\"", "width=\"inf\""),
                    };
                }
                (name.into(), xml)
            });
            assert!(reload(&broken).is_err(), "case {case}");
        }
    }

    #[test]
    fn xlsx_preserves_number_formats_decoration_blank_cells_raw_values_and_formulas() {
        let original = fixture();
        let bytes = write_xlsx_bytes(&original).unwrap();
        let restored = reload(&bytes).unwrap();
        for sheet in 0..2 {
            for row in 0..2 {
                for col in 0..5 {
                    let addr = CellAddr::new(col, row);
                    assert_eq!(
                        restored.sheets[sheet].raw(addr),
                        original.sheets[sheet].raw(addr)
                    );
                    assert_eq!(
                        restored.sheets[sheet].format(addr),
                        original.sheets[sheet].format(addr)
                    );
                }
            }
        }
        assert_eq!(restored.sheets[0].display(CellAddr::new(0, 0)), "12.50%");
        assert_eq!(restored.sheets[0].display(CellAddr::new(1, 0)), "0.25");
        assert_eq!(
            restored.sheets[0].display(CellAddr::new(2, 0)),
            "1970-01-01"
        );
        assert_eq!(restored.sheets[0].display(CellAddr::new(3, 0)), "0.125");
        assert_eq!(restored.sheets[0].raw(CellAddr::new(4, 1)), "");
        assert!(restored.sheets[0].format(CellAddr::new(4, 1)).bold);
        assert!(!restored.is_dirty() && !restored.can_undo());
    }

    #[test]
    fn presentation_follows_styles_relationships_and_zip_order() {
        let bytes = rewrite(&write_xlsx_bytes(&fixture()).unwrap(), |name, xml| {
            let name = if name == "xl/styles.xml" {
                "xl/metadata/appearance.xml"
            } else {
                name
            };
            let xml = if name.ends_with("workbook.xml.rels") {
                xml.replace(
                    "Target=\"styles.xml\"",
                    "Target=\"metadata/appearance.xml\"",
                )
            } else {
                xml
            };
            (name.into(), xml)
        });
        let restored = reload(&bytes).unwrap();
        assert!(restored.sheets[0].format(CellAddr::new(4, 1)).italic);
        assert_eq!(
            restored.sheets[0].display(CellAddr::new(2, 0)),
            "1970-01-01"
        );
    }

    #[test]
    fn broken_required_style_references_and_1904_dates_reject_the_import() {
        let bytes = write_xlsx_bytes(&fixture()).unwrap();
        for case in 0..4 {
            let broken = rewrite(&bytes, |name, mut xml| {
                if case == 0 && name == "xl/styles.xml" {
                    xml.truncate(xml.len() - 15);
                }
                if case == 1 && name == "xl/worksheets/sheet1.xml" {
                    xml = xml.replace("s=\"1\"", "s=\"999\"");
                }
                if case == 2 && name == "xl/styles.xml" {
                    xml = xml.replace("fontId=\"1\"", "fontId=\"999\"");
                }
                if case == 3 && name == "xl/workbook.xml" {
                    xml = xml.replace("<workbookPr ", "<workbookPr date1904=\"1\" ");
                }
                (name.into(), xml)
            });
            assert!(reload(&broken).is_err(), "case {case}");
        }
    }

    #[test]
    fn custom_codes_and_locale_builtins_remain_exportable_without_claiming_display_support() {
        let mut book = Workbook::new();
        let addr = CellAddr::new(0, 0);
        book.set_cell(addr, "12.5");
        for format in [
            CellFormat {
                number_format: "0.0000 \"units\"".into(),
                ..Default::default()
            },
            CellFormat {
                builtin_number_format: Some(27),
                ..Default::default()
            },
        ] {
            book.active_sheet_mut().set_format(addr, format.clone());
            let loaded = reload(&write_xlsx_bytes(&book).unwrap()).unwrap();
            assert_eq!(loaded.active_sheet().format(addr), format);
            assert_eq!(loaded.active_sheet().raw(addr), "12.5");
        }
    }

    #[test]
    fn xml_parser_requires_complete_namespace_correct_roots_and_checks_entities() {
        for xml in ["<styleSheet/>", "<styleSheet xmlns=\"urn:wrong\"/>",
            "<styleSheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">",
            "<styleSheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" value=\"&missing;\"/>"] {
            assert!(scan(xml, MAIN, "styleSheet", |_, _, _| Ok(())).is_err());
        }
    }
}
