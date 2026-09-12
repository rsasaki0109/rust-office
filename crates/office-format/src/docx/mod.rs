//! Minimal Office Open XML Word (.docx) read/write.

mod read;
mod write;

use std::io::{Cursor, Read, Write};

use office_core::Document;

use crate::{DocumentFormat, FormatError};

pub use read::{parse_document_parts, parse_document_xml, MediaMap, MediaPart};
pub use write::build_document_xml;

use read::{mime_for_media_path, parse_typed_rels, word_part_path};

/// DOCX (OOXML Word) backend (ZIP package).
#[derive(Debug, Default, Clone, Copy)]
pub struct DocxFormat;

impl DocxFormat {
    pub fn is_implemented(&self) -> bool {
        true
    }

    pub fn load_from_bytes(&self, bytes: &[u8]) -> Result<Document, FormatError> {
        let cursor = Cursor::new(bytes);
        let mut archive = zip::ZipArchive::new(cursor).map_err(FormatError::from)?;

        let mut document_xml = String::new();
        {
            let mut file = archive.by_name("word/document.xml").map_err(|_| {
                FormatError::InvalidDocument("DOCX missing word/document.xml".into())
            })?;
            file.read_to_string(&mut document_xml)?;
        }

        let mut rels = String::new();
        if let Ok(mut file) = archive.by_name("word/_rels/document.xml.rels") {
            let _ = file.read_to_string(&mut rels);
        }

        let typed = parse_typed_rels(&rels);

        let mut media = MediaMap::new();
        for (id, target) in &typed.images {
            let path = word_part_path(target);
            if let Ok(mut file) = archive.by_name(&path) {
                let mut data = Vec::new();
                if file.read_to_end(&mut data).is_ok() {
                    media.insert(
                        id.clone(),
                        MediaPart {
                            mime: mime_for_media_path(&path),
                            data,
                        },
                    );
                }
            }
        }

        let header_xml = typed.header.as_ref().and_then(|t| {
            let path = word_part_path(t);
            read_zip_string(&mut archive, &path)
        });
        let footer_xml = typed.footer.as_ref().and_then(|t| {
            let path = word_part_path(t);
            read_zip_string(&mut archive, &path)
        });

        parse_document_parts(
            &document_xml,
            &rels,
            &media,
            header_xml.as_deref(),
            footer_xml.as_deref(),
        )
    }

    pub fn save_to_bytes(&self, document: &Document) -> Result<Vec<u8>, FormatError> {
        let mut cursor = Cursor::new(Vec::new());
        write::write_docx_package(document, &mut cursor)?;
        Ok(cursor.into_inner())
    }
}

fn read_zip_string(
    archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
    path: &str,
) -> Option<String> {
    let mut file = archive.by_name(path).ok()?;
    let mut s = String::new();
    file.read_to_string(&mut s).ok()?;
    Some(s)
}

impl DocumentFormat for DocxFormat {
    fn extension(&self) -> &str {
        "docx"
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
