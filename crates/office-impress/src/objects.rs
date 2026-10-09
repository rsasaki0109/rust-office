//! Native slide objects and bounded embedded-image decoding.
use std::{io::Cursor, path::Path, sync::Arc};

use image::GenericImageView;
use serde::{Deserialize, Serialize};

const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;
const MAX_IMAGE_EDGE: u32 = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Bounds {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Default for Bounds {
    fn default() -> Self {
        Self {
            x: 0.25,
            y: 0.25,
            w: 0.3,
            h: 0.2,
        }
    }
}

impl Bounds {
    pub fn is_valid(self) -> bool {
        [self.x, self.y, self.w, self.h]
            .iter()
            .all(|n| n.is_finite())
            && self.x >= 0.0
            && self.y >= 0.0
            && self.w >= 0.01
            && self.h >= 0.01
            && self.x + self.w <= 1.00001
            && self.y + self.h <= 1.00001
    }
    pub fn constrain(&mut self) {
        for (value, fallback) in [
            (&mut self.x, 0.0),
            (&mut self.y, 0.0),
            (&mut self.w, 0.3),
            (&mut self.h, 0.2),
        ] {
            if !value.is_finite() {
                *value = fallback;
            }
        }
        self.w = self.w.clamp(0.01, 1.0);
        self.h = self.h.clamp(0.01, 1.0);
        self.x = self.x.clamp(0.0, 1.0 - self.w);
        self.y = self.y.clamp(0.0, 1.0 - self.h);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ShapeKind {
    Rectangle,
    Ellipse,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ObjectKind {
    Text {
        text: String,
        font_pt: f32,
        color: [u8; 3],
    },
    Shape {
        shape: ShapeKind,
        fill: [u8; 3],
    },
    Image {
        data: Arc<Vec<u8>>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SlideObject {
    pub bounds: Bounds,
    pub kind: ObjectKind,
}

impl SlideObject {
    pub fn text() -> Self {
        Self {
            bounds: Bounds::default(),
            kind: ObjectKind::Text {
                text: "Text".into(),
                font_pt: 24.0,
                color: [30, 30, 30],
            },
        }
    }
    pub fn shape(shape: ShapeKind) -> Self {
        Self {
            bounds: Bounds::default(),
            kind: ObjectKind::Shape {
                shape,
                fill: [45, 120, 190],
            },
        }
    }
    pub fn image_path(path: &Path) -> Result<Self, String> {
        let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
        if file.metadata().map_err(|e| e.to_string())?.len() > MAX_IMAGE_BYTES as u64 {
            return Err("Image exceeds the 8 MiB limit".into());
        }
        use std::io::Read;
        let mut data = Vec::new();
        file.take(MAX_IMAGE_BYTES as u64 + 1)
            .read_to_end(&mut data)
            .map_err(|e| e.to_string())?;
        let decoded = decode_image(&data)?;
        let (w, h) = decoded.dimensions();
        let mut bounds = Bounds::default();
        bounds.h = (bounds.w * (1.0 / w as f32) * h as f32 * 960.0 / 540.0).clamp(0.01, 0.5);
        // Recompute width if a tall image reached the height limit.
        bounds.w = (bounds.h * 540.0 / 960.0 * w as f32 / h as f32).clamp(0.01, 0.5);
        Ok(Self {
            bounds,
            kind: ObjectKind::Image {
                data: Arc::new(data),
            },
        })
    }
    pub fn validate(&self) -> Result<(), String> {
        if !self.bounds.is_valid() {
            return Err("Object position or size is outside the slide".into());
        }
        match &self.kind {
            ObjectKind::Text { font_pt, .. }
                if !font_pt.is_finite() || !(1.0..=200.0).contains(font_pt) =>
            {
                Err("Invalid text size".into())
            }
            ObjectKind::Image { data } => decode_image(data).map(|_| ()),
            _ => Ok(()),
        }
    }
}

pub fn decode_image(data: &[u8]) -> Result<image::DynamicImage, String> {
    if data.len() > MAX_IMAGE_BYTES {
        return Err("Image exceeds the 8 MiB limit".into());
    }
    let mut reader = image::ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_EDGE);
    limits.max_image_height = Some(MAX_IMAGE_EDGE);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let decoded = reader
        .decode()
        .map_err(|e| format!("Invalid or oversized image: {e}"))?;
    if decoded.width() == 0 || decoded.height() == 0 {
        return Err("Empty image".into());
    }
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{load_json_path, write_json_path, write_pptx_path, Presentation};

    fn png(width: u32, height: u32) -> Vec<u8> {
        let image = image::DynamicImage::new_rgba8(width, height);
        let mut bytes = Cursor::new(Vec::new());
        image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        bytes.into_inner()
    }

    #[test]
    fn embedded_images_survive_source_removal_and_share_history_storage() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("image.png");
        let data = png(12, 6);
        std::fs::write(&source, &data).unwrap();
        let object = SlideObject::image_path(&source).unwrap();
        std::fs::remove_file(source).unwrap();
        let mut p = Presentation::new();
        let mut slide = p.active_slide().unwrap().clone();
        slide.objects = vec![
            SlideObject::text(),
            SlideObject::shape(ShapeKind::Ellipse),
            object,
        ];
        p.update_active_slide(slide, false);
        p.duplicate_active_slide();
        let ObjectKind::Image { data: first } = &p.slides[0].objects[2].kind else {
            panic!()
        };
        let ObjectKind::Image { data: second } = &p.slides[1].objects[2].kind else {
            panic!()
        };
        assert!(Arc::ptr_eq(first, second));
        let path = dir.path().join("deck.json");
        write_json_path(&p, &path).unwrap();
        let loaded = load_json_path(&path).unwrap();
        assert_eq!(loaded.slides, p.slides);
        let ObjectKind::Image { data: saved } = &loaded.slides[1].objects[2].kind else {
            panic!()
        };
        assert_eq!(saved.as_ref(), &data);
        assert_eq!(decode_image(saved).unwrap().dimensions(), (12, 6));
    }

    #[test]
    fn move_resize_remove_undo_and_saved_revision_keep_objects() {
        let mut p = Presentation::new();
        let mut slide = p.active_slide().unwrap().clone();
        slide.objects.push(SlideObject::shape(ShapeKind::Rectangle));
        p.update_active_slide(slide.clone(), false);
        p.mark_clean();
        slide.objects[0].bounds = Bounds {
            x: 0.4,
            y: 0.3,
            w: 0.5,
            h: 0.4,
        };
        p.update_active_slide(slide.clone(), false);
        slide.objects[0].bounds.x = 0.45;
        p.update_active_slide(slide.clone(), true);
        assert!(p.undo());
        assert_eq!(
            p.active_slide().unwrap().objects[0].bounds,
            Bounds::default()
        );
        assert!(!p.is_dirty());
        p.redo();
        assert_eq!(p.active_slide().unwrap().objects[0].bounds.x, 0.45);
        slide.objects.clear();
        p.update_active_slide(slide, false);
        p.undo();
        assert_eq!(p.active_slide().unwrap().objects.len(), 1);
    }

    #[test]
    fn image_limits_and_corrupt_bytes_fail_before_insertion() {
        assert!(decode_image(b"not an image").is_err());
        assert!(decode_image(&vec![0; MAX_IMAGE_BYTES + 1]).is_err());
        assert!(decode_image(&png(MAX_IMAGE_EDGE + 1, 1)).is_err());
        assert!(decode_image(&png(1, MAX_IMAGE_EDGE + 1)).is_err());
        assert!(decode_image(&png(2, 2)).is_ok());
    }

    #[test]
    fn invalid_native_objects_are_rejected_and_export_preserves_destination() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deck.json");
        let mut p = Presentation::new();
        p.slides[0].objects.push(SlideObject::text());
        write_json_path(&p, &path).unwrap();
        let original = std::fs::read(&path).unwrap();
        p.slides[0].objects[0].bounds.w = -1.0;
        assert!(write_json_path(&p, &path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        std::fs::write(&path, serde_json::to_vec(&p).unwrap()).unwrap();
        assert!(load_json_path(&path).is_err());
        p.slides[0].objects[0] = SlideObject::shape(ShapeKind::Rectangle);
        let pptx = dir.path().join("existing.pptx");
        std::fs::write(&pptx, b"original pptx").unwrap();
        assert!(write_pptx_path(&p, &pptx).is_err());
        assert_eq!(std::fs::read(pptx).unwrap(), b"original pptx");
    }

    #[test]
    fn legacy_json_without_objects_loads_and_bounds_clamp_within_slide() {
        let mut json = serde_json::to_value(Presentation::demo()).unwrap();
        for slide in json["slides"].as_array_mut().unwrap() {
            slide.as_object_mut().unwrap().remove("objects");
        }
        let loaded: Presentation = serde_json::from_value(json).unwrap();
        assert!(loaded.slides.iter().all(|s| s.objects.is_empty()));
        let mut bounds = Bounds {
            x: -2.0,
            y: 0.99,
            w: 4.0,
            h: -1.0,
        };
        bounds.constrain();
        assert!(bounds.is_valid());
        assert_eq!(bounds.x, 0.0);
        assert_eq!(bounds.w, 1.0);
    }
}
