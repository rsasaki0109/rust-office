//! Decode document images into egui [`ColorImage`]s for GPU upload.

use std::collections::HashMap;
use std::path::Path;

use egui::{ColorImage, Context, TextureHandle, TextureOptions};
use office_core::{Document, Image, ImageSource};

/// Max edge length (pixels) uploaded to GPU to keep memory bounded.
const MAX_TEX_EDGE: u32 = 2048;

/// Load raw bytes for an image source.
pub fn load_image_bytes(source: &ImageSource) -> Result<Vec<u8>, String> {
    match source {
        ImageSource::Path { path } => std::fs::read(path).map_err(|e| format!("read {path}: {e}")),
        ImageSource::Embedded { data, .. } => Ok(data.clone()),
    }
}

/// Decode image bytes into an egui color image (RGBA8).
pub fn decode_color_image(bytes: &[u8]) -> Result<ColorImage, String> {
    let dyn_img = image::load_from_memory(bytes).map_err(|e| format!("decode image: {e}"))?;
    let dyn_img = downscale_if_needed(dyn_img);
    let rgba = dyn_img.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Ok(ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()))
}

fn downscale_if_needed(img: image::DynamicImage) -> image::DynamicImage {
    let w = img.width();
    let h = img.height();
    if w <= MAX_TEX_EDGE && h <= MAX_TEX_EDGE {
        return img;
    }
    let scale = (MAX_TEX_EDGE as f32 / w.max(h) as f32).min(1.0);
    let nw = ((w as f32) * scale).round().max(1.0) as u32;
    let nh = ((h as f32) * scale).round().max(1.0) as u32;
    img.resize(nw, nh, image::imageops::FilterType::Triangle)
}

/// Pixel size of an image on disk / in memory (before GPU downscale).
pub fn probe_pixel_size(source: &ImageSource) -> Result<(u32, u32), String> {
    let bytes = load_image_bytes(source)?;
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| format!("image format: {e}"))?;
    let size = reader.into_dimensions().map_err(|e| format!("image size: {e}"))?;
    Ok(size)
}

/// Fit pixel dimensions into a max display width (points), preserving aspect ratio.
pub fn fit_display_size(px_w: u32, px_h: u32, max_width_pt: f32) -> (f32, f32) {
    let px_w = px_w.max(1) as f32;
    let px_h = px_h.max(1) as f32;
    // Treat pixels ≈ points at 96dpi-ish (1px ≈ 0.75pt); clamp by max width.
    let native_w = px_w * 0.75;
    let native_h = px_h * 0.75;
    if native_w <= max_width_pt {
        (native_w, native_h)
    } else {
        let scale = max_width_pt / native_w;
        (max_width_pt, native_h * scale)
    }
}

/// Ensure every image in the document has a cached GPU texture.
pub fn ensure_image_textures(
    ctx: &Context,
    document: &Document,
    cache: &mut HashMap<String, TextureHandle>,
) {
    for block in document.blocks() {
        let office_core::Block::Image(image) = block else {
            continue;
        };
        let key = image.cache_key();
        if cache.contains_key(&key) {
            continue;
        }
        match load_and_decode(image) {
            Ok(color) => {
                let name = format!("rust-office-img-{key}");
                let handle = ctx.load_texture(name, color, TextureOptions::LINEAR);
                cache.insert(key, handle);
            }
            Err(_) => {
                // Leave missing; paint falls back to placeholder.
            }
        }
    }
}

fn load_and_decode(image: &Image) -> Result<ColorImage, String> {
    let bytes = load_image_bytes(&image.source)?;
    decode_color_image(&bytes)
}

/// Build an embedded [`Image`] from a filesystem path (bytes copied into the document).
pub fn image_from_path_fitted(path: impl AsRef<Path>, max_width_pt: f32) -> Result<Image, String> {
    let path = path.as_ref();
    let path_str = path.to_string_lossy().into_owned();
    let data = std::fs::read(path).map_err(|e| format!("read {path_str}: {e}"))?;
    let mime = office_core::mime_from_path(&path_str).to_string();
    let source = ImageSource::Embedded {
        mime: mime.clone(),
        data: data.clone(),
    };
    let (px_w, px_h) = {
        let reader = image::ImageReader::new(std::io::Cursor::new(&data))
            .with_guessed_format()
            .map_err(|e| format!("image format: {e}"))?;
        reader
            .into_dimensions()
            .map_err(|e| format!("image size: {e}"))?
    };
    let (w, h) = fit_display_size(px_w, px_h, max_width_pt);
    let alt = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "image".into());
    let _ = source; // constructed for clarity; from_embedded owns data
    Ok(Image::from_embedded(mime, data, alt, w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_display_size_scales_wide_images() {
        let (w, h) = fit_display_size(800, 400, 200.0);
        assert!((w - 200.0).abs() < 0.01);
        assert!((h - 100.0).abs() < 0.01);
    }

    #[test]
    fn decode_tiny_png() {
        use image::{ImageBuffer, ImageFormat, Rgb};
        let img: ImageBuffer<Rgb<u8>, _> =
            ImageBuffer::from_fn(2, 2, |x, y| Rgb([x as u8 * 80, y as u8 * 80, 200]));
        let mut bytes = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
        let decoded = decode_color_image(&bytes).expect("decode");
        assert_eq!(decoded.width(), 2);
        assert_eq!(decoded.height(), 2);
    }
}
