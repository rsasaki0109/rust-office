//! Admission before calamine allocates ODS repetitions or BIFF dense ranges.
use crate::{
    xlsx::XlsxError,
    xlsx_limits::{error, MAX_CELLS, MAX_SHEETS, MAX_TEXT_BYTES},
};
use quick_xml::{events::Event, Reader};
use std::io::{Cursor, Read};
const MAX_DENSE: u64 = 1_000_000;

pub(crate) fn ods(xml: &[u8]) -> Result<(), XlsxError> {
    let mut reader = Reader::from_reader(xml);
    let mut buf = Vec::new();
    let (mut sheets, mut rows, mut cols, mut repeat, mut cells, mut dense, mut text) =
        (0usize, 0u64, 0u64, 1u64, 0u64, 0u64, 0u64);
    let mut row_width = 0u64;
    loop {
        match reader
            .read_event_into(&mut buf)
            .map_err(|e| error(e.to_string()))?
        {
            Event::Start(e) | Event::Empty(e) => {
                match e.name().as_ref() {
                    b"table:table" => {
                        sheets += 1;
                        rows = 0;
                        cols = 0;
                    }
                    b"table:table-row" => {
                        repeat = attribute(&e, b"table:number-rows-repeated")?.unwrap_or(1);
                        rows = rows.saturating_add(repeat);
                        row_width = 0;
                        if rows > 1_048_576 {
                            return Err(error("ODS row repetitions exceed grid bounds"));
                        }
                    }
                    b"table:table-cell" | b"table:covered-table-cell" => {
                        let count = attribute(&e, b"table:number-columns-repeated")?.unwrap_or(1);
                        row_width = row_width.saturating_add(count);
                        cols = cols.max(row_width);
                        cells = cells.saturating_add(count.saturating_mul(repeat));
                        // All XML cells count, including repeated empty records.
                        if row_width > 16_384 || cells > MAX_DENSE {
                            return Err(error("ODS repetitions exceed the 1000000-cell limit"));
                        }
                    }
                    b"text:s" => {
                        text = text.saturating_add(
                            attribute(&e, b"text:c")?
                                .unwrap_or(1)
                                .saturating_mul(repeat),
                        )
                    }
                    _ => {}
                }
                for a in e.attributes() {
                    let a = a.map_err(|e| error(e.to_string()))?;
                    text = text.saturating_add((a.value.len() as u64).saturating_mul(repeat));
                }
            }
            Event::End(e) if e.name().as_ref() == b"table:table" => {
                dense = dense.saturating_add(rows.saturating_mul(cols));
                if dense > MAX_DENSE {
                    return Err(error(
                        "ODS aggregate dense ranges exceed the 1000000-cell limit",
                    ));
                }
                repeat = 1;
            }
            Event::Text(e) => {
                text = text.saturating_add(
                    (e.len() as u64)
                        .saturating_mul(repeat)
                        .saturating_mul(row_width.max(1)),
                )
            }
            Event::CData(e) => {
                text = text.saturating_add(
                    (e.len() as u64)
                        .saturating_mul(repeat)
                        .saturating_mul(row_width.max(1)),
                )
            }
            Event::DocType(_) => return Err(error("ODS DTDs are unsupported")),
            Event::Eof => break,
            _ => {}
        }
        if sheets > MAX_SHEETS || text > MAX_TEXT_BYTES as u64 {
            return Err(error("ODS exceeds sheet or expanded text limits"));
        }
        buf.clear();
    }
    Ok(())
}
fn attribute(e: &quick_xml::events::BytesStart<'_>, name: &[u8]) -> Result<Option<u64>, XlsxError> {
    for a in e.attributes() {
        let a = a.map_err(|e| error(e.to_string()))?;
        if a.key.as_ref() == name {
            let value = std::str::from_utf8(&a.value)
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .filter(|v| *v > 0)
                .ok_or_else(|| error("Invalid ODS repeat count"))?;
            return Ok(Some(value));
        }
    }
    Ok(None)
}

pub(crate) fn xls(bytes: &[u8]) -> Result<(), XlsxError> {
    let mut compound =
        cfb::CompoundFile::open_strict(Cursor::new(bytes)).map_err(|e| error(e.to_string()))?;
    let mut entries = 0usize;
    let mut total = 0u64;
    for entry in compound.walk() {
        entries += 1;
        total = total.saturating_add(entry.len());
        if entries > 4096 || entry.len() > 8 * 1024 * 1024 || total > 128 * 1024 * 1024 {
            return Err(error("XLS compound streams exceed admission limits"));
        }
    }
    let path = if compound.is_stream("/Workbook") {
        "/Workbook"
    } else {
        "/Book"
    };
    let mut stream = Vec::new();
    compound
        .open_stream(path)?
        .take(8 * 1024 * 1024 + 1)
        .read_to_end(&mut stream)?;
    if stream.len() > 8 * 1024 * 1024 {
        return Err(error("XLS workbook stream exceeds 8 MiB"));
    }
    biff(&stream)
}
fn u16_at(bytes: &[u8], offset: usize) -> Result<u16, XlsxError> {
    let b = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| error("Truncated XLS record"))?;
    Ok(u16::from_le_bytes([b[0], b[1]]))
}
fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, XlsxError> {
    let b = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| error("Truncated XLS record"))?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}
#[derive(Default)]
struct Span {
    min: Option<(u32, u32)>,
    max: (u32, u32),
}
impl Span {
    fn add(&mut self, row: u32, col: u32) -> Result<(), XlsxError> {
        if row >= 65536 || col >= 256 {
            return Err(error("XLS cell exceeds BIFF8 grid bounds"));
        }
        self.min = Some(
            self.min
                .map_or((row, col), |m| (m.0.min(row), m.1.min(col))),
        );
        self.max = (self.max.0.max(row), self.max.1.max(col));
        if self.size() > MAX_DENSE {
            return Err(error("XLS sparse range exceeds 1000000 cells"));
        }
        Ok(())
    }
    fn size(&self) -> u64 {
        self.min.map_or(0, |m| {
            u64::from(self.max.0 - m.0 + 1) * u64::from(self.max.1 - m.1 + 1)
        })
    }
}
fn biff(stream: &[u8]) -> Result<(), XlsxError> {
    let mut records = Vec::new();
    let mut starts = std::collections::HashMap::new();
    let mut offset = 0;
    while offset < stream.len() {
        // CFB streams may contain zero padding after the final BIFF EOF.
        if stream.get(offset..offset + 4) == Some(&[0, 0, 0, 0])
            && stream[offset..].iter().all(|b| *b == 0)
        {
            break;
        }
        let kind = u16_at(stream, offset)?;
        let len = usize::from(u16_at(stream, offset + 2)?);
        let data = stream
            .get(offset + 4..offset + 4 + len)
            .ok_or_else(|| error("Truncated XLS record"))?;
        starts.insert(offset, (kind, data));
        records.push((kind, data));
        offset += 4 + len;
        if records.len() > 500_000 {
            return Err(error("XLS record count exceeds admission limit"));
        }
    }
    let mut positions = std::collections::HashSet::new();
    for (kind, data) in &records {
        if *kind == 0x0085 {
            let pos = u32_at(data, 0)? as usize;
            let (bof, header) = starts
                .get(&pos)
                .ok_or_else(|| error("Invalid XLS sheet offset"))?;
            if *bof != 0x0809 || !positions.insert(pos) || u16_at(header, 2)? != 0x0010 {
                return Err(error("Unsupported or duplicate XLS sheet stream"));
            }
        }
    }
    let mut strings = Vec::new();
    for (index, (kind, data)) in records.iter().enumerate() {
        if *kind == 0x00fc {
            let unique = u32_at(data, 4)? as usize;
            if unique > MAX_CELLS {
                return Err(error("XLS shared-string count exceeds 100000"));
            }
            let chunks = std::iter::once(&data[8..])
                .chain(
                    records[index + 1..]
                        .iter()
                        .take_while(|(kind, _)| *kind == 0x003c)
                        .map(|(_, data)| *data),
                )
                .collect();
            strings = shared_sizes(chunks, unique)?;
        }
    }
    let (mut sheets, mut cells, mut text, mut dense) = (0usize, 0usize, 0usize, 0u64);
    let mut span = Span::default();
    for (kind, data) in records {
        match kind {
            0x0809 => {
                if u16_at(data, 0)? != 0x0600 {
                    return Err(error("Only bounded BIFF8 XLS input is supported"));
                }
                dense = dense.saturating_add(span.size());
                span = Span::default();
            }
            0x0085 => {
                sheets += 1;
            }
            0x0200 => {
                let start = u32_at(data, 0)?;
                let end = u32_at(data, 4)?;
                let left = u16_at(data, 8)?;
                let right = u16_at(data, 10)?;
                if end < start
                    || right < left
                    || end > 65536
                    || right > 256
                    || u64::from(end - start) * u64::from(right - left) > MAX_DENSE
                {
                    return Err(error("XLS declared dimensions exceed 1000000 cells"));
                }
            }
            0x0203 | 0x0204 | 0x0205 | 0x027e | 0x00fd | 0x0006 => {
                span.add(u16_at(data, 0)? as u32, u16_at(data, 2)? as u32)?;
                cells += 1;
                if kind == 0x00fd {
                    let size = *strings
                        .get(u32_at(data, 6)? as usize)
                        .ok_or_else(|| error("Invalid XLS shared-string index"))?;
                    text = text.saturating_add(size);
                } else if kind == 0x0006 {
                    // A BIFF token can expand into a sheet/name reference. Bound
                    // that expansion before calamine constructs formula strings.
                    text = text.saturating_add(data.len().saturating_mul(1024));
                } else {
                    text = text.saturating_add(data.len().saturating_mul(3));
                }
            }
            0x00bd => {
                let row = u16_at(data, 0)? as u32;
                let first = u16_at(data, 2)?;
                let last = u16_at(data, data.len().saturating_sub(2))?;
                if last < first {
                    return Err(error("Invalid XLS multiple-cell record"));
                }
                span.add(row, first as u32)?;
                span.add(row, last as u32)?;
                cells += usize::from(last - first) + 1;
            }
            _ => {}
        }
        if cells > MAX_CELLS
            || sheets > MAX_SHEETS
            || text > MAX_TEXT_BYTES
            || dense.saturating_add(span.size()) > MAX_DENSE
        {
            return Err(error(
                "XLS exceeds aggregate cell, text, sheet or dense-range limits",
            ));
        }
    }
    Ok(())
}
// Measure BIFF8 shared strings across CONTINUE records without allocating text.
fn shared_sizes(chunks: Vec<&[u8]>, count: usize) -> Result<Vec<usize>, XlsxError> {
    struct Bytes<'a> {
        chunks: Vec<&'a [u8]>,
        index: usize,
        offset: usize,
    }
    impl Bytes<'_> {
        fn byte(&mut self) -> Result<u8, XlsxError> {
            while self
                .chunks
                .get(self.index)
                .is_some_and(|c| self.offset == c.len())
            {
                self.index += 1;
                self.offset = 0;
            }
            let b = *self
                .chunks
                .get(self.index)
                .and_then(|c| c.get(self.offset))
                .ok_or_else(|| error("Truncated XLS shared strings"))?;
            self.offset += 1;
            Ok(b)
        }
        fn number(&mut self, n: usize) -> Result<usize, XlsxError> {
            let mut value = 0;
            for i in 0..n {
                value |= (self.byte()? as usize) << (8 * i);
            }
            Ok(value)
        }
    }
    let mut bytes = Bytes {
        chunks,
        index: 0,
        offset: 0,
    };
    let mut sizes = Vec::new();
    let mut total = 0usize;
    for _ in 0..count {
        let chars = bytes.number(2)?;
        let flags = bytes.byte()?;
        let rich = if flags & 8 != 0 { bytes.number(2)? } else { 0 };
        let ext = if flags & 4 != 0 { bytes.number(4)? } else { 0 };
        let mut wide = flags & 1 != 0;
        for _ in 0..chars {
            if bytes
                .chunks
                .get(bytes.index)
                .is_some_and(|c| bytes.offset == c.len())
            {
                wide = bytes.byte()? & 1 != 0;
            }
            bytes.byte()?;
            if wide {
                bytes.byte()?;
            }
        }
        let skip = rich.saturating_mul(4).saturating_add(ext);
        if skip > 8 * 1024 * 1024 {
            return Err(error("XLS string metadata exceeds limit"));
        }
        for _ in 0..skip {
            bytes.byte()?;
        }
        let size = chars.saturating_mul(3);
        total = total.saturating_add(size);
        if total > MAX_TEXT_BYTES {
            return Err(error("XLS shared-string text exceeds 32 MiB"));
        }
        sizes.push(size);
    }
    Ok(sizes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    fn record(stream: &mut Vec<u8>, kind: u16, data: &[u8]) {
        stream.extend(kind.to_le_bytes());
        stream.extend((data.len() as u16).to_le_bytes());
        stream.extend(data);
    }
    fn workbook() -> Vec<u8> {
        let mut stream = Vec::new();
        let mut bof = [0u8; 16];
        bof[..4].copy_from_slice(&[0, 6, 5, 0]);
        record(&mut stream, 0x0809, &bof);
        let sheet = [41, 0, 0, 0, 0, 0, 5, 0, b'S', b'h', b'e', b'e', b't'];
        record(&mut stream, 0x0085, &sheet);
        record(&mut stream, 0x000a, &[]);
        bof[2] = 0x10;
        record(&mut stream, 0x0809, &bof);
        let dims = [0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0];
        record(&mut stream, 0x0200, &dims);
        let mut number = vec![0; 6];
        number.extend(42.0f64.to_le_bytes());
        record(&mut stream, 0x0203, &number);
        record(&mut stream, 0x000a, &[]);
        stream
    }
    #[test]
    fn valid_biff8_compound_file_still_imports() {
        let mut compound = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
        compound
            .create_stream("/Workbook")
            .unwrap()
            .write_all(&workbook())
            .unwrap();
        let bytes = compound.into_inner().into_inner();
        xls(&bytes).unwrap();
        let path = std::env::temp_dir().join(format!("office-bounded-{}.xls", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        let loaded = crate::load_xlsx_path(&path).unwrap();
        assert_eq!(loaded.sheets[0].raw(crate::CellAddr::new(0, 0)), "42");
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn forged_dimensions_and_sparse_records_are_rejected_before_calamine() {
        let mut stream = workbook();
        let pos = 61 + 4; // BOF ends at 61; dimensions payload follows.
        stream[pos + 4..pos + 8].copy_from_slice(&65536u32.to_le_bytes());
        stream[pos + 10..pos + 12].copy_from_slice(&256u16.to_le_bytes());
        assert!(biff(&stream)
            .unwrap_err()
            .to_string()
            .contains("dimensions"));
        let mut sparse = workbook();
        let mut number = vec![255, 255, 255, 0, 0, 0];
        number.extend(1.0f64.to_le_bytes());
        record(&mut sparse, 0x0203, &number);
        assert!(biff(&sparse)
            .unwrap_err()
            .to_string()
            .contains("sparse range"));
    }
    #[test]
    fn sst_continuations_are_measured_and_repeated_refs_are_bounded() {
        let sizes = shared_sizes(vec![&[4, 0, 0, b'a', b'b'], &[1, b'c', 0, b'd', 0]], 1).unwrap();
        assert_eq!(sizes, [12]);
        let mut stream = workbook();
        let mut sst = vec![1, 0, 0, 0, 1, 0, 0, 0];
        sst.extend([0, 4, 0]);
        sst.extend(vec![b'x'; 1024]);
        record(&mut stream, 0x00fc, &sst);
        let cell = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        for _ in 0..12000 {
            record(&mut stream, 0x00fd, &cell);
        }
        assert!(biff(&stream).unwrap_err().to_string().contains("aggregate"));
    }
    #[test]
    fn ods_repetitions_sparse_ranges_and_invalid_counts_are_bounded() {
        for inner in [
            "<table:table-row table:number-rows-repeated=\"1048576\"><table:table-cell table:number-columns-repeated=\"16384\"/></table:table-row>",
            "<table:table-row><table:table-cell table:number-columns-repeated=\"18446744073709551615\"/></table:table-row>",
            "<table:table-row table:number-rows-repeated=\"0\"/>",
        ] { let xml=format!("<table:table>{inner}</table:table>");assert!(ods(xml.as_bytes()).is_err(),"{inner}"); }
        ods(b"<table:table><table:table-row><table:table-cell office:value=\"42\"/></table:table-row></table:table>").unwrap();
    }
}

#[cfg(test)]
mod ods_package_tests {
    use std::io::{Cursor, Write};
    #[test]
    fn ods_package_still_imports_and_renaming_does_not_bypass_repetition_checks() {
        let content = r#"<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0"><office:body><office:spreadsheet><table:table table:name="Sheet1"><table:table-row><table:table-cell office:value-type="float" office:value="42"/></table:table-row></table:table></office:spreadsheet></office:body></office:document-content>"#;
        let path = std::env::temp_dir().join(format!("office-ods-{}.xlsx", std::process::id()));
        for (xml, accepted) in [
            (content.to_string(), true),
            (
                content.replace(
                    "<table:table-row>",
                    "<table:table-row table:number-rows-repeated=\"1048576\">",
                ),
                false,
            ),
        ] {
            let mut zip = zip2::ZipWriter::new(Cursor::new(Vec::new()));
            for (name, part) in [
                ("mimetype", "application/vnd.oasis.opendocument.spreadsheet"),
                ("content.xml", xml.as_str()),
                ("META-INF/manifest.xml", "<manifest:manifest xmlns:manifest=\"urn:oasis:names:tc:opendocument:xmlns:manifest:1.0\"/>"),
            ] {
                zip.start_file(name, zip2::write::SimpleFileOptions::default())
                    .unwrap();
                zip.write_all(part.as_bytes()).unwrap();
            }
            std::fs::write(&path, zip.finish().unwrap().into_inner()).unwrap();
            let result = crate::load_xlsx_path(&path);
            assert_eq!(result.is_ok(), accepted, "{result:?}");
            if let Ok(book) = result {
                assert_eq!(book.sheets[0].raw(crate::CellAddr::new(0, 0)), "42");
            }
        }
        std::fs::remove_file(path).unwrap();
    }
}
