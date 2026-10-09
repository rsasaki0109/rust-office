//! Check compressed and expanded input before format parsers allocate model data.
use crate::FormatError;
use quick_xml::{events::Event, Reader};
use std::{
    collections::HashSet,
    io::{Cursor, Read},
};
pub(crate) const MAX_FILE: u64 = 64 * 1024 * 1024;
const MAX_XML: u64 = 8 * 1024 * 1024;
const MAX_EXPANDED: u64 = 128 * 1024 * 1024;
fn error(message: impl Into<String>) -> FormatError {
    FormatError::InvalidDocument(message.into())
}
pub(crate) fn read(reader: &mut dyn Read) -> Result<Vec<u8>, FormatError> {
    let mut bytes = Vec::new();
    reader.take(MAX_FILE + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE {
        return Err(error("Writer input exceeds the 64 MiB file limit"));
    }
    Ok(bytes)
}
pub(crate) fn part(reader: &mut dyn Read, path: &str) -> Result<Vec<u8>, FormatError> {
    let limit = if path.ends_with(".xml") || path.ends_with(".rels") {
        MAX_XML
    } else {
        office_core::limits::MAX_IMAGE_BYTES as u64
    };
    let mut bytes = Vec::new();
    reader
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| error(format!("{path}: {e}")))?;
    if bytes.len() as u64 > limit {
        return Err(error(format!("{path}: Writer part exceeds its byte limit")));
    }
    Ok(bytes)
}
pub(crate) fn package(bytes: &[u8]) -> Result<(), FormatError> {
    if bytes.len() as u64 > MAX_FILE {
        return Err(error("Writer input exceeds the 64 MiB file limit"));
    }
    let start = bytes.len().saturating_sub(65_535 + 22);
    let end = bytes[start..]
        .windows(4)
        .rposition(|w| w == b"PK\x05\x06")
        .map(|i| start + i)
        .ok_or_else(|| error("Writer ZIP end record is missing"))?;
    let footer = bytes
        .get(end..end + 22)
        .ok_or_else(|| error("Truncated Writer ZIP end record"))?;
    let n = |i| u16::from_le_bytes([footer[i], footer[i + 1]]);
    if end + 22 + n(20) as usize != bytes.len() || n(4) != 0 || n(6) != 0 || n(8) != n(10) {
        return Err(error("Invalid Writer ZIP directory"));
    }
    if n(8) > 4096
        || footer[12..16] == [255; 4]
        || footer[16..20] == [255; 4]
        || (end >= 20 && &bytes[end - 20..end - 16] == b"PK\x06\x07")
    {
        return Err(error(
            "Writer ZIP exceeds directory limits (ZIP64 unsupported)",
        ));
    }
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
    if zip.len() > 4096 {
        return Err(error("Writer ZIP exceeds 4096 parts"));
    }
    let mut names = HashSet::new();
    let mut expanded = 0u64;
    let mut actual = 0u64;
    let mut items = 0usize;
    let mut text = 0usize;
    for index in 0..zip.len() {
        let file = zip.by_index_raw(index)?;
        let path = file.name().to_string();
        if !names.insert(path.trim_start_matches('/').to_ascii_lowercase()) {
            return Err(error("Duplicate or ambiguous Writer ZIP part"));
        }
        expanded = expanded
            .checked_add(file.size())
            .ok_or_else(|| error("Writer ZIP size overflow"))?;
        if expanded > MAX_EXPANDED {
            return Err(error("Writer ZIP exceeds 128 MiB expanded limit"));
        }
        let xml = path.ends_with(".xml") || path.ends_with(".rels");
        let limit = if xml {
            MAX_XML
        } else {
            office_core::limits::MAX_IMAGE_BYTES as u64
        };
        if file.size() > limit {
            return Err(error(format!("{path}: Writer part exceeds its byte limit")));
        }
        drop(file);
        if xml {
            let data = part(&mut zip.by_index(index)?, &path)?;
            actual = actual.saturating_add(data.len() as u64);
            if actual > MAX_EXPANDED {
                return Err(error("Writer XML reads exceed the expanded limit"));
            }
            scan(&data, &mut items, &mut text).map_err(|e| error(format!("{path}: {e}")))?;
        }
    }
    Ok(())
}
fn scan(xml: &[u8], items: &mut usize, text: &mut usize) -> Result<(), FormatError> {
    let mut reader = Reader::from_reader(xml);
    let mut depth = 0usize;
    let mut buf = Vec::new();
    loop {
        let event = reader
            .read_event_into(&mut buf)
            .map_err(|e| error(e.to_string()))?;
        let start = matches!(&event, Event::Start(_));
        match event {
            Event::Start(e) | Event::Empty(e) => {
                *items = items.saturating_add(1);
                // Bound nesting before recursive/stack-based format interpretation.
                if start {
                    depth += 1;
                }
                if depth > 128 {
                    return Err(error("Writer XML exceeds the nesting limit"));
                }
                for a in e.attributes() {
                    let a = a.map_err(|e| error(e.to_string()))?;
                    *text = text.saturating_add(a.value.len());
                    if e.local_name().as_ref() == b"s" && a.key.local_name().as_ref() == b"c" {
                        let value = a
                            .decode_and_unescape_value(reader.decoder())
                            .map_err(|e| error(e.to_string()))?;
                        let count = value
                            .parse::<usize>()
                            .map_err(|_| error("Invalid ODT space count"))?;
                        *text = text.saturating_add(count);
                    }
                }
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::Text(e) => *text = text.saturating_add(e.len()),
            Event::CData(e) => *text = text.saturating_add(e.len()),
            Event::DocType(_) => return Err(error("Writer XML DTDs are unsupported")),
            Event::Eof => break,
            _ => {}
        }
        if *items > office_core::limits::MAX_DOCUMENT_ITEMS
            || *text > office_core::limits::MAX_DOCUMENT_TEXT
        {
            return Err(error(
                "Writer XML exceeds aggregate item or 4 MiB text limits",
            ));
        }
        buf.clear();
    }
    Ok(())
}
pub(crate) fn model(doc: &office_core::Document) -> Result<(), FormatError> {
    office_core::limits::validate_document(doc).map_err(error)
}
