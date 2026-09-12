//! Minimal OpenDocument Text (.odt) read/write.
//!
//! Supports paragraphs, character runs (bold / italic / underline / font size),
//! paragraph alignment, tables, and embedded images (`Pictures/` + `draw:frame`).

mod read;
mod styles;
mod write;

use std::collections::HashMap;
use std::io::{Cursor, Read, Write};

use office_core::Document;

use crate::{DocumentFormat, FormatError};

pub use read::{parse_content_xml, parse_content_xml_with_pictures};
pub use styles::{apply_styles_xml, parse_master_header_footer};
pub use write::build_content_xml;

/// OpenDocument Text backend (ZIP package).
#[derive(Debug, Default, Clone, Copy)]
pub struct OdtFormat;

impl OdtFormat {
    pub fn is_implemented(&self) -> bool {
        true
    }

    pub fn load_from_bytes(&self, bytes: &[u8]) -> Result<Document, FormatError> {
        let cursor = Cursor::new(bytes);
        let mut archive = zip::ZipArchive::new(cursor).map_err(FormatError::from)?;

        let mut pictures: HashMap<String, Vec<u8>> = HashMap::new();
        let names: Vec<String> = (0..archive.len())
            .filter_map(|i| archive.by_index(i).ok().map(|f| f.name().to_string()))
            .collect();
        for name in names {
            let lower = name.to_ascii_lowercase();
            if !(lower.starts_with("pictures/") || lower.starts_with("media/")) {
                continue;
            }
            if name.ends_with('/') {
                continue;
            }
            let mut file = archive.by_name(&name).map_err(FormatError::from)?;
            let mut data = Vec::new();
            file.read_to_end(&mut data)?;
            pictures.insert(name, data);
        }

        let mut content = String::new();
        {
            let mut file = archive
                .by_name("content.xml")
                .map_err(|_| FormatError::InvalidDocument("ODT missing content.xml".into()))?;
            file.read_to_string(&mut content)?;
        }
        let mut doc = parse_content_xml_with_pictures(&content, &pictures)?;

        if let Ok(mut file) = archive.by_name("styles.xml") {
            let mut styles = String::new();
            file.read_to_string(&mut styles)?;
            let _ = apply_styles_xml(&mut doc, &styles);
        }

        Ok(doc)
    }

    pub fn save_to_bytes(&self, document: &Document) -> Result<Vec<u8>, FormatError> {
        let mut cursor = Cursor::new(Vec::new());
        write::write_odt_package(document, &mut cursor)?;
        Ok(cursor.into_inner())
    }
}

impl DocumentFormat for OdtFormat {
    fn extension(&self) -> &str {
        "odt"
    }

    fn load_from_reader(&self, reader: &mut dyn Read) -> Result<Document, FormatError> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes)?;
        self.load_from_bytes(&bytes)
    }

    fn save_to_writer(
        &self,
        document: &Document,
        writer: &mut dyn Write,
    ) -> Result<(), FormatError> {
        let bytes = self.save_to_bytes(document)?;
        writer.write_all(&bytes)?;
        Ok(())
    }
}
