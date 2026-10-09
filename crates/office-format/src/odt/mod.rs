//! Minimal OpenDocument Text (.odt) read/write.
//!
//! Supports paragraphs, character runs (bold / italic / underline / font size),
//! paragraph alignment, tables, and embedded images (`Pictures/` + `draw:frame`).

mod read;
mod styles;
mod write;
mod xml;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::io::{Cursor, Read, Write};

use office_core::Document;

use crate::{DocumentFormat, FormatError};

pub use read::{parse_content_xml, parse_content_xml_with_pictures};
pub use styles::{apply_styles_xml, parse_master_header_footer};
pub use write::build_content_xml;

use xml::{content_images, image_part_path, invalid, part_error};

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

        let content = utf8_part(read_zip_bytes(&mut archive, "content.xml")?, "content.xml")?;
        let mut pictures: HashMap<String, Vec<u8>> = HashMap::new();
        for href in content_images(&content)? {
            let path = image_part_path(&href).map_err(|e| part_error("content.xml", e))?;
            if !pictures.contains_key(&path) {
                pictures.insert(path.clone(), read_zip_bytes(&mut archive, &path)?);
            }
        }
        let mut doc = parse_content_xml_with_pictures(&content, &pictures)?;

        if let Some(bytes) = read_optional_zip_bytes(&mut archive, "styles.xml")? {
            let styles = utf8_part(bytes, "styles.xml")?;
            apply_styles_xml(&mut doc, &styles)?;
        }

        Ok(doc)
    }

    pub fn save_to_bytes(&self, document: &Document) -> Result<Vec<u8>, FormatError> {
        let mut cursor = Cursor::new(Vec::new());
        write::write_odt_package(document, &mut cursor)?;
        Ok(cursor.into_inner())
    }
}

fn utf8_part(bytes: Vec<u8>, path: &str) -> Result<String, FormatError> {
    String::from_utf8(bytes).map_err(|e| part_error(path, invalid(e.to_string())))
}

fn read_zip_bytes(
    archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
    path: &str,
) -> Result<Vec<u8>, FormatError> {
    read_optional_zip_bytes(archive, path)?.ok_or_else(|| invalid(format!("ODT missing {path}")))
}

fn read_optional_zip_bytes(
    archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
    path: &str,
) -> Result<Option<Vec<u8>>, FormatError> {
    let mut file = match archive.by_name(path) {
        Ok(file) => file,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(error) => return Err(part_error(path, error.into())),
    };
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|e| part_error(path, e.into()))?;
    Ok(Some(bytes))
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
