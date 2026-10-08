//! Minimal Office Open XML Word (.docx) read/write.

mod read;
mod write;
mod xml;

#[cfg(test)]
mod tests;

use std::io::{Cursor, Read, Write};

use office_core::Document;

use crate::{DocumentFormat, FormatError};

pub use read::{parse_document_parts, parse_document_xml, MediaMap, MediaPart};
pub use write::build_document_xml;

use read::{
    document_references, mime_for_media_path, parse_hf_paragraph, parse_typed_rels,
    resolve_reference, word_part_path, ReferenceKind,
};
use xml::{invalid, part_error};

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

        let document_xml = read_zip_string(&mut archive, "word/document.xml")?;
        let references = document_references(&document_xml)?;
        let rels = read_optional_zip_bytes(&mut archive, "word/_rels/document.xml.rels")?
            .map(|bytes| utf8_part(bytes, "word/_rels/document.xml.rels"))
            .transpose()?;
        let typed = rels
            .as_deref()
            .map(parse_typed_rels)
            .transpose()?
            .unwrap_or_default();
        let mut media = MediaMap::new();
        let mut header: Option<(bool, String)> = None;
        let mut footer: Option<(bool, String)> = None;
        for reference in &references {
            let relationship = resolve_reference(&typed, reference)?;
            match reference.kind {
                ReferenceKind::Image => {
                    if media.contains_key(&reference.id) {
                        continue;
                    }
                    let path = word_part_path(&relationship.target)
                        .map_err(|e| part_error("word/_rels/document.xml.rels", e))?;
                    let data = read_zip_bytes(&mut archive, &path)?;
                    media.insert(
                        reference.id.clone(),
                        MediaPart {
                            mime: mime_for_media_path(&path),
                            data,
                        },
                    );
                }
                ReferenceKind::Header | ReferenceKind::Footer => {
                    let path = word_part_path(&relationship.target)
                        .map_err(|e| part_error("word/_rels/document.xml.rels", e))?;
                    let text = read_zip_string(&mut archive, &path)?;
                    let root = if reference.kind == ReferenceKind::Header {
                        "hdr"
                    } else {
                        "ftr"
                    };
                    parse_hf_paragraph(&text, root).map_err(|e| part_error(&path, e))?;
                    let selected = if reference.kind == ReferenceKind::Header {
                        &mut header
                    } else {
                        &mut footer
                    };
                    if selected
                        .as_ref()
                        .is_none_or(|(is_default, _)| reference.is_default && !is_default)
                    {
                        *selected = Some((reference.is_default, text));
                    }
                }
                ReferenceKind::Hyperlink => {}
            }
        }

        parse_document_parts(
            &document_xml,
            rels.as_deref().unwrap_or(""),
            &media,
            header.as_ref().map(|(_, text)| text.as_str()),
            footer.as_ref().map(|(_, text)| text.as_str()),
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
) -> Result<String, FormatError> {
    utf8_part(read_zip_bytes(archive, path)?, path)
}

fn utf8_part(bytes: Vec<u8>, path: &str) -> Result<String, FormatError> {
    String::from_utf8(bytes).map_err(|e| part_error(path, invalid(e.to_string())))
}

fn read_zip_bytes(
    archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
    path: &str,
) -> Result<Vec<u8>, FormatError> {
    read_optional_zip_bytes(archive, path)?.ok_or_else(|| invalid(format!("DOCX missing {path}")))
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
