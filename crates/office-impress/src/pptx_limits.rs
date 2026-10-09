//! Admission limits for PPTX packages and their editable presentation model.
use crate::{ObjectKind, PptxError, Slide};
use std::io::{Read, Seek};
use zip::ZipArchive;

pub(super) const MAX_PACKAGE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PARTS: usize = 4096;
const MAX_EXPANDED_BYTES: u64 = 128 * 1024 * 1024;
const MAX_SLIDES: usize = 1000;
const MAX_OBJECTS_PER_SLIDE: usize = 1000;
const MAX_OBJECTS: usize = 10_000;
const MAX_TEXT_BYTES: usize = 64 * 1024 * 1024;

fn error(message: impl Into<String>) -> PptxError {
    PptxError::Parse(message.into())
}
pub(super) fn package_size(size: u64) -> Result<(), PptxError> {
    if size > MAX_PACKAGE_BYTES {
        Err(error("PPTX package exceeds the 64 MiB file limit"))
    } else {
        Ok(())
    }
}
/// Check the directory count before ZipArchive allocates entry metadata.
/// These bounded packages do not support ZIP64 central directories.
pub(super) fn directory(bytes: &[u8]) -> Result<(), PptxError> {
    let start = bytes.len().saturating_sub(65_535 + 22);
    let index = bytes[start..]
        .windows(4)
        .rposition(|w| w == b"PK\x05\x06")
        .map(|i| i + start)
        .ok_or_else(|| error("PPTX ZIP end record is missing"))?;
    let footer = bytes
        .get(index..index + 22)
        .ok_or_else(|| error("Truncated PPTX ZIP end record"))?;
    let u16_at = |i| u16::from_le_bytes([footer[i], footer[i + 1]]);
    if index + 22 + u16_at(20) as usize != bytes.len() {
        return Err(error("Invalid PPTX ZIP end-record length"));
    }
    let zip64 = index >= 20 && &bytes[index - 20..index - 16] == b"PK\x06\x07";
    if zip64
        || u16_at(8) == u16::MAX
        || u16_at(10) == u16::MAX
        || footer[12..16] == [255; 4]
        || footer[16..20] == [255; 4]
    {
        return Err(error(
            "ZIP64 central directories are unsupported for bounded PPTX packages",
        ));
    }
    if u16_at(8) as usize > MAX_PARTS || u16_at(10) as usize > MAX_PARTS {
        return Err(error("PPTX exceeds the 4096 ZIP-entry limit"));
    }
    Ok(())
}
pub(super) fn archive<R: Read + Seek>(archive: &mut ZipArchive<R>) -> Result<(), PptxError> {
    archive_with_limits(archive, MAX_PARTS, MAX_EXPANDED_BYTES)
}
fn archive_with_limits<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    parts: usize,
    expanded: u64,
) -> Result<(), PptxError> {
    if archive.len() > parts {
        return Err(error(format!("PPTX exceeds the {parts} ZIP-entry limit")));
    }
    let mut total = 0u64;
    let mut names = std::collections::HashSet::new();
    for index in 0..archive.len() {
        let file = archive.by_index_raw(index)?;
        if !names.insert(file.name().to_owned()) {
            return Err(error(format!("Duplicate PPTX part {}", file.name())));
        }
        if (file.name().ends_with(".xml") || file.name().ends_with(".rels"))
            && file.size() > 8 * 1024 * 1024
        {
            return Err(error(format!(
                "{}: XML part exceeds the 8 MiB limit",
                file.name()
            )));
        }
        total = total
            .checked_add(file.size())
            .ok_or_else(|| error("PPTX expanded-size overflow"))?;
        if total > expanded {
            return Err(error(format!(
                "PPTX expanded parts exceed the {} MiB limit",
                expanded / (1024 * 1024)
            )));
        }
    }
    Ok(())
}

#[derive(Default)]
pub(super) struct ModelBudget {
    slides: usize,
    objects: usize,
    text: usize,
}
impl ModelBudget {
    pub fn slide_count(count: usize) -> Result<(), PptxError> {
        if count > MAX_SLIDES {
            Err(error("PPTX exceeds the 1000-slide limit"))
        } else {
            Ok(())
        }
    }
    pub fn add(&mut self, slide: &Slide) -> Result<(), PptxError> {
        self.add_with_limits(
            slide,
            MAX_SLIDES,
            MAX_OBJECTS_PER_SLIDE,
            MAX_OBJECTS,
            MAX_TEXT_BYTES,
        )
    }
    fn add_with_limits(
        &mut self,
        slide: &Slide,
        max_slides: usize,
        per_slide: usize,
        max_objects: usize,
        max_text: usize,
    ) -> Result<(), PptxError> {
        // Count title/body boxes as well as added objects.
        let count = slide
            .objects
            .len()
            .checked_add(2)
            .ok_or_else(|| error("PPTX object-count overflow"))?;
        if count > per_slide {
            return Err(error(format!(
                "PPTX slide exceeds the {per_slide}-object limit (including title/body)"
            )));
        }
        let slides = self
            .slides
            .checked_add(1)
            .ok_or_else(|| error("PPTX slide-count overflow"))?;
        if slides > max_slides {
            return Err(error(format!("PPTX exceeds the {max_slides}-slide limit")));
        }
        let objects = self
            .objects
            .checked_add(count)
            .ok_or_else(|| error("PPTX object-count overflow"))?;
        if objects > max_objects {
            return Err(error(format!(
                "PPTX exceeds the {max_objects}-object deck limit"
            )));
        }
        let mut text = self.text;
        for value in [&slide.title.text, &slide.body.text, &slide.notes]
            .into_iter()
            .chain(slide.objects.iter().filter_map(|o| match &o.kind {
                ObjectKind::Text { text, .. } => Some(text),
                _ => None,
            }))
        {
            text = text
                .checked_add(value.len())
                .ok_or_else(|| error("PPTX text-size overflow"))?;
            if text > max_text {
                return Err(error(
                    "PPTX editable text exceeds its aggregate limit (64 MiB by default)",
                ));
            }
        }
        self.slides = slides;
        self.objects = objects;
        self.text = text;
        Ok(())
    }
}

#[derive(Default)]
pub(super) struct OutputBudget {
    expanded: u64,
}
impl OutputBudget {
    pub fn add(&mut self, size: usize) -> Result<(), PptxError> {
        self.expanded = self
            .expanded
            .checked_add(size as u64)
            .ok_or_else(|| error("PPTX output-size overflow"))?;
        if self.expanded > MAX_EXPANDED_BYTES {
            return Err(error("PPTX output parts exceed the 128 MiB expanded limit"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use zip::{write::FileOptions, ZipWriter};
    fn zip(parts: &[(&str, &[u8])]) -> Vec<u8> {
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        for (name, data) in parts {
            zip.start_file(
                *name,
                FileOptions::default().compression_method(zip::CompressionMethod::Deflated),
            )
            .unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }
    #[test]
    fn package_and_archive_limits_admit_the_boundary_and_reject_before_expansion() {
        assert!(package_size(MAX_PACKAGE_BYTES).is_ok());
        assert!(package_size(MAX_PACKAGE_BYTES + 1).is_err());
        let bytes = zip(&[("first", b"123"), ("second", b"45")]);
        let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
        assert!(archive_with_limits(&mut archive, 2, 5).is_ok());
        assert!(archive_with_limits(&mut archive, 1, 5).is_err());
        assert!(archive_with_limits(&mut archive, 2, 4).is_err());
        // Highly compressed data is charged at its expanded size.
        let data = vec![0; 128 * 1024];
        let bytes = zip(&[("compressed", &data)]);
        assert!(bytes.len() < 1024);
        assert!(
            archive_with_limits(&mut ZipArchive::new(Cursor::new(bytes)).unwrap(), 1, 1024)
                .is_err()
        );
    }
    #[test]
    fn duplicate_parts_are_rejected_even_when_zip_name_lookup_would_hide_one() {
        let mut bytes = zip(&[("first", b"one"), ("other", b"two")]);
        let positions: Vec<usize> = bytes
            .windows(5)
            .enumerate()
            .filter_map(|(i, w)| (w == b"other").then_some(i))
            .collect();
        assert_eq!(positions.len(), 2);
        for offset in positions {
            bytes[offset..offset + 5].copy_from_slice(b"first");
        }
        let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
        assert!(archive_with_limits(&mut archive, 2, 6)
            .unwrap_err()
            .to_string()
            .contains("Duplicate PPTX part"));
    }

    #[test]
    fn directory_counts_are_checked_before_zip_metadata_allocation() {
        let original = zip(&[("part", b"data")]);
        assert!(directory(&original).is_ok());
        let mut bytes = original.clone();
        let offset = bytes.len() - 22;
        bytes[offset + 8..offset + 10].copy_from_slice(&4097u16.to_le_bytes());
        bytes[offset + 10..offset + 12].copy_from_slice(&4097u16.to_le_bytes());
        assert!(directory(&bytes)
            .unwrap_err()
            .to_string()
            .contains("ZIP-entry limit"));
        let mut bytes = original;
        bytes[offset + 10..offset + 12].copy_from_slice(&u16::MAX.to_le_bytes());
        assert!(directory(&bytes).unwrap_err().to_string().contains("ZIP64"));
    }

    #[test]
    fn repeated_slides_charge_text_and_objects_and_failed_admission_is_atomic() {
        let slide = Slide::title_and_body("日本語", "body");
        let bytes = slide.title.text.len() + slide.body.text.len();
        let mut budget = ModelBudget::default();
        assert!(budget.add_with_limits(&slide, 2, 2, 4, bytes * 2).is_ok());
        assert!(budget.add_with_limits(&slide, 2, 2, 4, bytes * 2).is_ok());
        assert!(budget.add_with_limits(&slide, 2, 2, 4, bytes * 2).is_err());
        let mut budget = ModelBudget::default();
        assert!(budget.add_with_limits(&slide, 2, 2, 4, bytes - 1).is_err());
        assert_eq!((budget.slides, budget.objects, budget.text), (0, 0, 0));
        assert!(budget.add_with_limits(&slide, 2, 2, 4, bytes).is_ok());
        assert!(ModelBudget::slide_count(1000).is_ok());
        assert!(ModelBudget::slide_count(1001).is_err());
    }
}
