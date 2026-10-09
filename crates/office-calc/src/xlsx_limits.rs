//! XLSX admission before calamine allocates dense ranges or shared-formula maps.
use super::xlsx::XlsxError;
use quick_xml::{
    events::{BytesStart, Event},
    Reader,
};
use std::io::{Cursor, Read};

pub(crate) const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_XML_BYTES: u64 = 8 * 1024 * 1024;
const MAX_EXPANDED_BYTES: u64 = 128 * 1024 * 1024;
const MAX_PARTS: usize = 4096;
pub(crate) const MAX_CELLS: usize = 100_000;
pub(crate) const MAX_SHEETS: usize = 1000;
pub(crate) const MAX_TEXT_BYTES: usize = 32 * 1024 * 1024;
const MAX_DENSE_CELLS: u64 = 1_000_000;

pub(crate) fn error(message: impl Into<String>) -> XlsxError {
    XlsxError::Read(message.into())
}
fn directory(bytes: &[u8]) -> Result<(), XlsxError> {
    let start = bytes.len().saturating_sub(65_535 + 22);
    let index = bytes[start..]
        .windows(4)
        .rposition(|w| w == b"PK\x05\x06")
        .map(|i| i + start)
        .ok_or_else(|| error("XLSX ZIP end record is missing"))?;
    let footer = bytes
        .get(index..index + 22)
        .ok_or_else(|| error("Truncated XLSX ZIP end record"))?;
    let n = |i| u16::from_le_bytes([footer[i], footer[i + 1]]);
    if index + 22 + n(20) as usize != bytes.len() {
        return Err(error("Invalid XLSX ZIP end-record length"));
    }
    if (index >= 20 && &bytes[index - 20..index - 16] == b"PK\x06\x07")
        || n(8) == u16::MAX
        || n(10) == u16::MAX
        || footer[12..16] == [255; 4]
        || footer[16..20] == [255; 4]
    {
        return Err(error(
            "ZIP64 central directories are unsupported for bounded XLSX input",
        ));
    }
    if n(8) as usize > MAX_PARTS || n(10) as usize > MAX_PARTS {
        return Err(error("XLSX exceeds the 4096 ZIP-entry limit"));
    }
    Ok(())
}

/// Returns true for an XLSX package, including one renamed to a legacy extension.
/// Non-ZIP legacy files use only the file/model checks at the caller.
pub(crate) fn package(bytes: &[u8]) -> Result<bool, XlsxError> {
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(error("Spreadsheet exceeds the 64 MiB file limit"));
    }
    if !bytes.starts_with(b"PK") {
        return Ok(false);
    }
    directory(bytes)?;
    let mut zip = zip2::ZipArchive::new(Cursor::new(bytes)).map_err(|e| error(e.to_string()))?;
    if zip.len() > MAX_PARTS {
        return Err(error("Spreadsheet exceeds the 4096 ZIP-entry limit"));
    }
    let mut expanded = 0u64;
    let mut names = std::collections::HashSet::new();
    let mut xml_parts = Vec::new();
    let mut is_xlsx = false;
    for index in 0..zip.len() {
        let file = zip.by_index_raw(index).map_err(|e| error(e.to_string()))?;
        // calamine resolves XML parts case-insensitively and ignores leading slashes.
        let name = file.name().trim_start_matches('/').to_ascii_lowercase();
        if !names.insert(name.clone()) {
            return Err(error("Duplicate or ambiguous spreadsheet ZIP part"));
        }
        is_xlsx |= name == "xl/workbook.xml";
        expanded = expanded
            .checked_add(file.size())
            .ok_or_else(|| error("Spreadsheet expanded-size overflow"))?;
        if expanded > MAX_EXPANDED_BYTES {
            return Err(error("Spreadsheet expanded parts exceed the 128 MiB limit"));
        }
        let sheet_part = ["xl/worksheets/", "xl/chartsheets/", "xl/dialogsheets/"]
            .iter()
            .any(|prefix| name.starts_with(prefix));
        if name.ends_with(".xml") || name.ends_with(".rels") || sheet_part {
            if file.size() > MAX_XML_BYTES {
                return Err(error(format!(
                    "{}: XML part exceeds the 8 MiB limit",
                    file.name()
                )));
            }
            xml_parts.push((index, name == "xl/sharedstrings.xml", sheet_part));
        }
    }
    // XLSB's binary range allocations are not covered by the XML admission pass.
    if names.contains("xl/workbook.bin") {
        return Err(error(
            "XLSB input is unsupported by spreadsheet admission checks",
        ));
    }
    if !is_xlsx {
        return Ok(false);
    }
    let mut budget = XmlBudget::default();
    let mut actual = 0u64;
    xml_parts.sort_by_key(|(_, shared, _)| !shared);
    for (index, is_shared, sheet_part) in xml_parts {
        let mut file = zip.by_index(index).map_err(|e| error(e.to_string()))?;
        let mut xml = Vec::new();
        file.by_ref()
            .take(MAX_XML_BYTES + 1)
            .read_to_end(&mut xml)?;
        if xml.len() as u64 > MAX_XML_BYTES {
            return Err(error("Spreadsheet XML read exceeds the 8 MiB limit"));
        }
        actual = actual.saturating_add(xml.len() as u64);
        if actual > MAX_EXPANDED_BYTES {
            return Err(error(
                "Spreadsheet actual XML reads exceed the 128 MiB limit",
            ));
        }
        budget
            .scan(&xml, is_shared, sheet_part)
            .map_err(|e| error(format!("{}: {e}", file.name())))?;
    }
    Ok(true)
}

#[derive(Default)]
struct Span {
    min: Option<(u32, u32)>,
    max: (u32, u32),
}
impl Span {
    fn add(&mut self, point: (u32, u32)) -> Result<(), XlsxError> {
        if point.0 >= 1_048_576 || point.1 >= 16_384 {
            return Err(error("XLSX cell address exceeds Excel grid bounds"));
        }
        self.min = Some(
            self.min
                .map_or(point, |m| (m.0.min(point.0), m.1.min(point.1))),
        );
        self.max = (self.max.0.max(point.0), self.max.1.max(point.1));
        self.size()?;
        Ok(())
    }
    fn size(&self) -> Result<u64, XlsxError> {
        let Some(min) = self.min else {
            return Ok(0);
        };
        let size = u64::from(self.max.0 - min.0 + 1) * u64::from(self.max.1 - min.1 + 1);
        if size > MAX_DENSE_CELLS {
            Err(error("XLSX dense range exceeds the 1000000-cell limit"))
        } else {
            Ok(size)
        }
    }
}
fn address(bytes: &[u8]) -> Result<(u32, u32), XlsxError> {
    let text = std::str::from_utf8(bytes).map_err(|e| error(e.to_string()))?;
    // Accept only explicit A1 coordinates; reject arithmetic overflow before parsing.
    let split = text
        .find(|c: char| c.is_ascii_digit())
        .ok_or_else(|| error("Invalid XLSX cell address"))?;
    let (letters, digits) = text.split_at(split);
    if letters.is_empty()
        || !letters.bytes().all(|c| c.is_ascii_alphabetic())
        || !digits.bytes().all(|c| c.is_ascii_digit())
    {
        return Err(error("Invalid XLSX cell address"));
    }
    let mut col = 0u32;
    for c in letters.bytes() {
        col = col
            .checked_mul(26)
            .and_then(|v| v.checked_add(u32::from(c.to_ascii_uppercase() - b'A' + 1)))
            .ok_or_else(|| error("XLSX column overflow"))?;
    }
    let row = digits
        .parse::<u32>()
        .map_err(|_| error("XLSX row overflow"))?;
    if row == 0 || row > 1_048_576 || col == 0 || col > 16_384 {
        return Err(error("XLSX cell address exceeds Excel grid bounds"));
    }
    Ok((row - 1, col - 1))
}
fn range(bytes: &[u8]) -> Result<Span, XlsxError> {
    let mut points = bytes.split(|c| *c == b':');
    let first = address(points.next().unwrap_or_default())?;
    let second = points.next().map(address).transpose()?.unwrap_or(first);
    if points.next().is_some() || second.0 < first.0 || second.1 < first.1 {
        return Err(error("Invalid or reversed XLSX range"));
    }
    let mut span = Span::default();
    span.add(first)?;
    span.add(second)?;
    Ok(span)
}
fn attr<'a>(node: &'a BytesStart<'a>, key: &[u8]) -> Result<Option<Vec<u8>>, XlsxError> {
    let mut value = None;
    for a in node.attributes() {
        let a = a.map_err(|e| error(e.to_string()))?;
        if a.key.as_ref() == key {
            value = Some(a.value.into_owned());
        }
    }
    Ok(value)
}
#[derive(Default)]
struct XmlBudget {
    cells: usize,
    sheets: usize,
    dense: u64,
    shared: u64,
    shared_sizes: Vec<usize>,
    shared_text: usize,
    formula_text: usize,
}
impl XmlBudget {
    fn scan(&mut self, xml: &[u8], is_shared: bool, sheet_part: bool) -> Result<(), XlsxError> {
        let mut reader = Reader::from_reader(xml);
        reader.config_mut().expand_empty_elements = true;
        let mut depth = 0usize;
        let mut worksheet = sheet_part;
        let mut workbook = false;
        let mut span = Span::default();
        let mut row = 0u32;
        let mut col = 0u32;
        let mut in_string = false;
        let mut string_has_value = false;
        let mut phonetic = false;
        let mut cell_shared = false;
        let mut formula_sizes: Vec<Option<usize>> = Vec::new();
        loop {
            match reader.read_event().map_err(|e| error(e.to_string()))? {
                Event::Start(node) => {
                    let name = node.local_name();
                    let name = name.as_ref();
                    if depth == 0 {
                        worksheet |= name == b"worksheet";
                        workbook = name == b"workbook";
                    }
                    depth += 1;
                    if workbook && name == b"sheet" {
                        self.sheets += 1;
                        if self.sheets > MAX_SHEETS {
                            return Err(error("XLSX exceeds the 1000-sheet limit"));
                        }
                    }
                    if is_shared && name == b"si" {
                        if in_string {
                            return Err(error("Nested XLSX shared-string records are unsupported"));
                        }
                        in_string = true;
                        string_has_value = false;
                        phonetic = false;
                        self.shared_sizes.push(0);
                        if self.shared_sizes.len() > MAX_DENSE_CELLS as usize {
                            return Err(error("XLSX shared-string table exceeds the entry limit"));
                        }
                    }
                    if in_string {
                        if name == b"rPh" {
                            phonetic = true;
                        }
                        if name == b"r" || (name == b"t" && !phonetic) {
                            string_has_value = true;
                        }
                    }
                    if !worksheet {
                        continue;
                    }
                    match name {
                        b"dimension" => {
                            if let Some(v) = attr(&node, b"ref")? {
                                range(&v)?;
                            }
                        }
                        b"row" => {
                            if let Some(v) = attr(&node, b"r")? {
                                let n = std::str::from_utf8(&v)
                                    .ok()
                                    .and_then(|v| v.parse::<u32>().ok())
                                    .filter(|v| *v > 0 && *v <= 1_048_576)
                                    .ok_or_else(|| error("Invalid XLSX row index"))?;
                                row = n - 1;
                            }
                        }
                        b"c" => {
                            cell_shared = attr(&node, b"t")?.as_deref() == Some(b"s");
                            self.cells += 1;
                            if self.cells > MAX_CELLS {
                                return Err(error("XLSX exceeds the 100000-cell record limit"));
                            }
                            let point = if let Some(v) = attr(&node, b"r")? {
                                let p = address(&v)?;
                                col = p.1;
                                p
                            } else {
                                (row, col)
                            };
                            span.add(point)?;
                        }
                        b"v" if cell_shared => {
                            let text = reader
                                .read_text(node.name())
                                .map_err(|e| error(e.to_string()))?;
                            depth -= 1;
                            let index = text
                                .parse::<usize>()
                                .map_err(|_| error("Invalid XLSX shared-string index"))?;
                            let size = *self
                                .shared_sizes
                                .get(index)
                                .ok_or_else(|| error("Missing XLSX shared string"))?;
                            self.shared_text =
                                self.shared_text.saturating_add(size.saturating_add(1));
                            if self.shared_text > MAX_TEXT_BYTES {
                                return Err(error("XLSX expanded shared-string references exceed the 32 MiB limit"));
                            }
                        }
                        b"f" => {
                            let shared = attr(&node, b"t")?.as_deref() == Some(b"shared");
                            let mut shared_index = None;
                            let mut shared_ref = false;
                            if shared {
                                let v = attr(&node, b"si")?
                                    .ok_or_else(|| error("Missing XLSX shared-formula index"))?;
                                let index = std::str::from_utf8(&v)
                                    .ok()
                                    .and_then(|v| v.parse::<usize>().ok())
                                    .ok_or_else(|| error("Invalid XLSX shared-formula index"))?;
                                if index >= MAX_CELLS {
                                    return Err(error(
                                        "XLSX shared-formula index exceeds the 100000 limit",
                                    ));
                                }
                                shared_index = Some(index);
                                if let Some(v) = attr(&node, b"ref")? {
                                    shared_ref = true;
                                    self.shared = self.shared.saturating_add(range(&v)?.size()?);
                                    if self.shared > MAX_DENSE_CELLS {
                                        return Err(error("XLSX shared-formula ranges exceed the 1000000-cell aggregate limit"));
                                    }
                                }
                            }
                            let text = reader
                                .read_text(node.name())
                                .map_err(|e| error(e.to_string()))?;
                            depth -= 1;
                            let mut size = text.len().saturating_add(1);
                            if let Some(index) = shared_index {
                                if shared_ref {
                                    // Coordinate substitution can lengthen short A1 references.
                                    size = text.len().saturating_mul(16).saturating_add(1);
                                    formula_sizes.resize(formula_sizes.len().max(index + 1), None);
                                    formula_sizes[index] = Some(size);
                                } else if let Some(Some(base)) = formula_sizes.get(index) {
                                    size = size.max(*base);
                                } else {
                                    return Err(error("Missing XLSX shared-formula base"));
                                }
                            }
                            self.formula_text = self.formula_text.saturating_add(size);
                            if self.formula_text > MAX_TEXT_BYTES {
                                return Err(error(
                                    "XLSX expanded formula text exceeds the 32 MiB budget",
                                ));
                            }
                        }
                        _ => {}
                    }
                }
                Event::End(node) => {
                    depth = depth
                        .checked_sub(1)
                        .ok_or_else(|| error("Invalid XLSX XML nesting"))?;
                    if is_shared && node.local_name().as_ref() == b"si" {
                        // calamine omits records with neither text nor a rich-text run.
                        // Reject them so reference-size indices cannot diverge.
                        if !string_has_value {
                            return Err(error(
                                "XLSX shared-string record has no supported text content",
                            ));
                        }
                        in_string = false;
                    }
                    if is_shared && node.local_name().as_ref() == b"rPh" {
                        phonetic = false;
                    }
                    if worksheet {
                        match node.local_name().as_ref() {
                            b"row" => {
                                row = row.saturating_add(1);
                                col = 0;
                            }
                            b"c" => {
                                col = col.saturating_add(1);
                                cell_shared = false;
                            }
                            _ => {}
                        }
                    }
                }
                Event::Text(text) if in_string => {
                    if let Some(size) = self.shared_sizes.last_mut() {
                        *size = size.saturating_add(text.len());
                    }
                }
                Event::CData(text) if in_string => {
                    if let Some(size) = self.shared_sizes.last_mut() {
                        *size = size.saturating_add(text.len());
                    }
                }
                Event::Decl(decl) => {
                    if let Some(encoding) = decl.encoding() {
                        let encoding = encoding.map_err(|e| error(e.to_string()))?;
                        if !encoding.eq_ignore_ascii_case(b"UTF-8")
                            && !encoding.eq_ignore_ascii_case(b"UTF8")
                        {
                            return Err(error("XLSX admission supports UTF-8 XML only"));
                        }
                    }
                }
                Event::DocType(_) => return Err(error("XLSX XML document types are unsupported")),
                Event::Eof => {
                    if depth != 0 {
                        return Err(error("Truncated XLSX XML part"));
                    }
                    break;
                }
                _ => {}
            }
        }
        self.dense = self.dense.saturating_add(span.size()?);
        if self.dense > MAX_DENSE_CELLS {
            return Err(error(
                "XLSX dense ranges exceed the 1000000-cell aggregate limit",
            ));
        }
        Ok(())
    }
}

pub(crate) fn model(workbook: &crate::Workbook) -> Result<(), XlsxError> {
    if workbook.sheets.len() > MAX_SHEETS {
        return Err(error("XLSX exceeds the 1000-sheet limit"));
    }
    let mut count = 0usize;
    let mut text = 0usize;
    let mut dense = 0u64;
    for sheet in &workbook.sheets {
        let mut span = Span::default();
        for (addr, cell) in sheet.occupied() {
            count += 1;
            text = text.saturating_add(cell.raw.len());
            if count > MAX_CELLS || text > MAX_TEXT_BYTES {
                return Err(error("XLSX model exceeds cell or 32 MiB raw-text limits"));
            }
            span.add((addr.row, addr.col))?;
        }
        dense = dense.saturating_add(span.size()?);
        if dense > MAX_DENSE_CELLS {
            return Err(error(
                "XLSX dense ranges exceed the 1000000-cell aggregate limit",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    fn zip(parts: &[(&str, &[u8])]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in parts {
            zip.start_file(
                *name,
                zip::write::FileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated),
            )
            .unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }
    fn scan(xml: &str) -> Result<(), XlsxError> {
        XmlBudget::default().scan(xml.as_bytes(), false, false)
    }
    #[test]
    fn ranges_accept_boundary_and_reject_reversed_overflow_or_sparse_explosion() {
        assert_eq!(range(b"A1:J100000").unwrap().size().unwrap(), 1_000_000);
        for bad in [
            "A1:J100001",
            "A1:XFD1048576",
            "B2:A1",
            "A0",
            "XFE1",
            "A1048577",
            "ZZZZZZZZ999999999999999",
            "A1:B2:C3",
        ] {
            assert!(range(bad.as_bytes()).is_err(), "{bad}");
        }
        assert!(scan("<worksheet><dimension ref=\"A1\"/><sheetData><row><c r=\"A1\"><v>1</v></c><c r=\"XFD1048576\"><v>2</v></c></row></sheetData></worksheet>").is_err());
    }
    #[test]
    fn implicit_cells_and_declared_dimensions_are_checked_before_dense_allocation() {
        assert!(
            scan("<worksheet><dimension ref=\"A1:XFD1048576\"/><sheetData/></worksheet>").is_err()
        );
        assert!(scan("<worksheet><sheetData><row r=\"1\"><c><v>1</v></c></row><row r=\"1048576\"><c><v>2</v></c></row></sheetData></worksheet>").is_err());
        assert!(
            scan("<worksheet><sheetData><row r=\"0\"><c/></row></sheetData></worksheet>").is_err()
        );
        assert!(
            scan("<worksheet><sheetData><row r=\"8\"><c/><c/></row></sheetData></worksheet>")
                .is_ok()
        );
    }
    #[test]
    fn worksheet_cell_counts_and_dense_spans_are_aggregate() {
        let mut budget = XmlBudget::default();
        let sheet="<worksheet><sheetData><row><c r=\"A1\"/><c r=\"A600000\"/></row></sheetData></worksheet>";
        budget.scan(sheet.as_bytes(), false, false).unwrap();
        assert!(budget.scan(sheet.as_bytes(), false, false).is_err());
        let xml = format!(
            "<worksheet><sheetData><row>{}</row></sheetData></worksheet>",
            "<c r=\"A1\"/>".repeat(MAX_CELLS + 1)
        );
        assert!(scan(&xml).unwrap_err().to_string().contains("cell record"));
    }
    #[test]
    fn shared_formula_indices_ranges_and_replication_are_bounded() {
        for f in [
            "<f t=\"shared\" si=\"100000\" ref=\"A1\">1</f>",
            "<f t=\"shared\" si=\"0\" ref=\"A1:XFD1048576\">1</f>",
            "<f t=\"shared\" si=\"0\"/>",
        ] {
            assert!(scan(&format!(
                "<worksheet><sheetData><row><c r=\"A1\">{f}</c></row></sheetData></worksheet>"
            ))
            .is_err());
        }
        let base = "1+".repeat(2048);
        let cells = format!(
            "<c r=\"A1\"><f t=\"shared\" si=\"0\" ref=\"A1:A600\">{base}</f></c>{}",
            "<c r=\"A2\"><f t=\"shared\" si=\"0\"/></c>".repeat(599)
        );
        assert!(scan(&format!(
            "<worksheet><sheetData><row>{cells}</row></sheetData></worksheet>"
        ))
        .unwrap_err()
        .to_string()
        .contains("expanded formula"));
    }
    #[test]
    fn empty_or_phonetic_only_shared_records_cannot_shift_reference_budgets() {
        let mut budget = XmlBudget::default();
        assert!(budget
            .scan(b"<sst><si/><si><t>large</t></si></sst>", true, false)
            .is_err());
        assert!(XmlBudget::default()
            .scan(
                b"<sst><si><rPh><t>phonetic</t></rPh></si></sst>",
                true,
                false
            )
            .is_err());
        assert!(XmlBudget::default()
            .scan(b"<sst><si><t/></si><si><r/></si></sst>", true, false)
            .is_ok());
    }
    #[test]
    fn shared_string_replication_is_checked_even_when_parts_are_out_of_order() {
        let sst = format!("<sst><si><t>{}</t></si></sst>", "x".repeat(4096));
        let worksheet = format!(
            "<worksheet><sheetData><row>{}</row></sheetData></worksheet>",
            "<c r=\"A1\" t=\"s\"><v>0</v></c>".repeat(8193)
        );
        let bytes = zip(&[
            ("xl/workbook.xml", b"<workbook/>"),
            ("xl/worksheets/sheet1.xml", worksheet.as_bytes()),
            ("xl/sharedStrings.xml", sst.as_bytes()),
        ]);
        assert!(package(&bytes)
            .unwrap_err()
            .to_string()
            .contains("expanded shared-string"));
        let mut budget = XmlBudget::default();
        budget
            .scan(
                "<sst><si><t>日本語</t></si></sst>".as_bytes(),
                true,
                false,
            )
            .unwrap();
        budget
            .scan(
                b"<worksheet><sheetData><row><c t=\"s\"><v>0</v></c></row></sheetData></worksheet>",
                false,
                false,
            )
            .unwrap();
        assert!(budget
            .scan(
                b"<worksheet><sheetData><row><c t=\"s\"><v>9</v></c></row></sheetData></worksheet>",
                false,
                false
            )
            .is_err());
    }
    #[test]
    fn package_directory_orphan_sizes_xml_lengths_and_ambiguous_parts_are_rejected() {
        let mut bytes = zip(&[
            ("xl/workbook.xml", b"<workbook/>"),
            ("orphan.bin", b"small"),
        ]);
        let central = bytes.windows(4).rposition(|w| w == b"PK\x01\x02").unwrap();
        bytes[central + 24..central + 28]
            .copy_from_slice(&((MAX_EXPANDED_BYTES + 1) as u32).to_le_bytes());
        assert!(package(&bytes).unwrap_err().to_string().contains("128 MiB"));
        let bytes = zip(&[
            ("xl/workbook.xml", b"<workbook/>"),
            ("XL/WORKBOOK.XML", b"<workbook/>"),
        ]);
        assert!(package(&bytes)
            .unwrap_err()
            .to_string()
            .contains("ambiguous"));
        let bytes = zip(&[
            ("xl/workbook.xml", b"<workbook/>"),
            ("orphan.xml", &vec![b' '; MAX_XML_BYTES as usize + 1]),
        ]);
        assert!(package(&bytes).unwrap_err().to_string().contains("8 MiB"));
        let mut bytes = zip(&[("xl/workbook.xml", b"<workbook/>")]);
        let footer = bytes.len() - 22;
        bytes[footer + 10..footer + 12].copy_from_slice(&4097u16.to_le_bytes());
        assert!(package(&bytes)
            .unwrap_err()
            .to_string()
            .contains("ZIP-entry"));
        bytes[footer + 10..footer + 12].copy_from_slice(&u16::MAX.to_le_bytes());
        assert!(package(&bytes).unwrap_err().to_string().contains("ZIP64"));
    }
    #[test]
    fn model_validation_prevents_export_wraparound_or_unreadable_ranges() {
        let mut book = crate::Workbook::new();
        book.set_cell(crate::CellAddr::new(16384, 0), "out of grid");
        assert!(model(&book).is_err());
        let mut book = crate::Workbook::new();
        book.set_cell(crate::CellAddr::new(0, 0), "1");
        book.set_cell(crate::CellAddr::new(16383, 1048575), "2");
        assert!(model(&book).is_err());
        assert!(crate::write_xlsx_bytes(&book).is_err());
    }
    #[test]
    fn non_xml_sheet_extensions_and_unexpected_roots_cannot_bypass_dense_limits() {
        let xml=b"<unexpected><sheetData><row><c r=\"A1\"/><c r=\"XFD1048576\"/></row></sheetData></unexpected>";
        let bytes = zip(&[
            ("xl/workbook.xml", b"<workbook/>"),
            ("xl/worksheets/custom.bin", xml),
        ]);
        assert!(package(&bytes)
            .unwrap_err()
            .to_string()
            .contains("dense range"));
    }
    #[test]
    fn truncated_or_non_utf8_xml_is_rejected_without_parser_fallback() {
        assert!(scan("<worksheet><sheetData>").is_err());
        assert!(scan("<!DOCTYPE worksheet><worksheet/>").is_err());
        assert!(scan("<?xml version=\"1.0\" encoding=\"UTF-16\"?><worksheet/>").is_err());
    }
}
