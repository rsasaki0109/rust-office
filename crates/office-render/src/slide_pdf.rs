//! One rasterized 144-dpi slide per PDF page; rendering uses the shared egui scene.
use std::{collections::HashMap, path::Path};

use egui::{
    epaint::{Primitive, Vertex},
    ColorImage, Pos2, Rect, TextureId,
};
use office_impress::{Presentation, Slide, Theme, SLIDE_HEIGHT_PT, SLIDE_WIDTH_PT};
use printpdf::{
    ColorBits, ColorSpace, ImageFilter, ImageTransform, ImageXObject, Mm, PdfDocument, Px,
};

use crate::slides::{slide_image, slide_shapes};

pub const SLIDE_PDF_DPI: f32 = 144.0;
const MAX_PDF_SLIDES: usize = 200;

pub fn write_presentation_pdf_path(
    ctx: &egui::Context,
    presentation: &Presentation,
    path: &Path,
) -> Result<(), String> {
    let bytes = presentation_pdf_bytes(ctx, presentation)?;
    office_core::storage::atomic_write(path, &bytes).map_err(|e| e.to_string())
}

/// Render using initialized egui fonts; call within an egui pass. Text is rasterized.
pub fn presentation_pdf_bytes(
    ctx: &egui::Context,
    presentation: &Presentation,
) -> Result<Vec<u8>, String> {
    if presentation.slides.is_empty() || presentation.slides.len() > MAX_PDF_SLIDES {
        return Err(format!("PDF export requires 1–{MAX_PDF_SLIDES} slides"));
    }
    let width = Mm(SLIDE_WIDTH_PT * 25.4 / 72.0);
    let height = Mm(SLIDE_HEIGHT_PT * 25.4 / 72.0);
    let (doc, first_page, first_layer) =
        PdfDocument::new(&presentation.title, width, height, "Slide 1");
    for (index, slide) in presentation.slides.iter().enumerate() {
        let bitmap = raster_slide(ctx, slide, &presentation.theme, 2)?;
        let mut encoded = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, 95)
            .encode(
                bitmap.as_raw(),
                bitmap.width(),
                bitmap.height(),
                image::ExtendedColorType::Rgb8,
            )
            .map_err(|e| e.to_string())?;
        let image = printpdf::Image {
            image: ImageXObject {
                width: Px(bitmap.width() as usize),
                height: Px(bitmap.height() as usize),
                color_space: ColorSpace::Rgb,
                bits_per_component: ColorBits::Bit8,
                interpolate: true,
                image_data: encoded,
                image_filter: Some(ImageFilter::DCT),
                smask: None,
                clipping_bbox: None,
            },
        };
        let (page, layer) = if index == 0 {
            (first_page, first_layer)
        } else {
            doc.add_page(width, height, format!("Slide {}", index + 1))
        };
        image.add_to_layer(
            doc.get_page(page).get_layer(layer),
            ImageTransform {
                dpi: Some(SLIDE_PDF_DPI),
                ..Default::default()
            },
        );
    }
    doc.save_to_bytes().map_err(|e| e.to_string())
}

pub(crate) fn raster_slide(
    ctx: &egui::Context,
    slide: &Slide,
    theme: &Theme,
    scale: u32,
) -> Result<image::RgbImage, String> {
    let width = SLIDE_WIDTH_PT as u32 * scale;
    let height = SLIDE_HEIGHT_PT as u32 * scale;
    let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(width as f32, height as f32));
    let mut textures = HashMap::new();
    let shapes = slide_shapes(ctx, rect, slide, theme, |data| {
        let id = TextureId::User(std::sync::Arc::as_ptr(data) as usize as u64);
        if let std::collections::hash_map::Entry::Vacant(entry) = textures.entry(id) {
            entry.insert(slide_image(data)?);
        }
        Ok(id)
    })?;
    // Background is initialized directly to avoid two full-screen triangles.
    let shapes = shapes.into_iter().skip(1).collect();
    let meshes = ctx.tessellate(shapes, 1.0);
    textures.insert(TextureId::default(), ctx.fonts(|fonts| fonts.image()));
    let mut bitmap = image::RgbImage::from_pixel(width, height, image::Rgb(theme.background));
    for primitive in meshes {
        let Primitive::Mesh(mesh) = primitive.primitive else {
            return Err("Unsupported PDF paint callback".into());
        };
        let texture = textures
            .get(&mesh.texture_id)
            .ok_or("Missing PDF texture")?;
        let clip = primitive.clip_rect.intersect(rect);
        for indices in mesh.indices.as_chunks::<3>().0 {
            let vertices = [
                mesh.vertices[indices[0] as usize],
                mesh.vertices[indices[1] as usize],
                mesh.vertices[indices[2] as usize],
            ];
            paint_triangle(&mut bitmap, texture, clip, vertices);
        }
    }
    Ok(bitmap)
}

fn edge(a: Pos2, b: Pos2, p: Pos2) -> f32 {
    (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x)
}
fn top_left(a: Pos2, b: Pos2) -> bool {
    b.y < a.y || (b.y == a.y && b.x > a.x)
}

fn paint_triangle(
    bitmap: &mut image::RgbImage,
    texture: &ColorImage,
    clip: Rect,
    mut vertices: [Vertex; 3],
) {
    let mut area = edge(vertices[0].pos, vertices[1].pos, vertices[2].pos);
    if area == 0.0 {
        return;
    }
    if area < 0.0 {
        vertices.swap(1, 2);
        area = -area;
    }
    let [a, b, c] = vertices;
    let min_x = a
        .pos
        .x
        .min(b.pos.x)
        .min(c.pos.x)
        .max(clip.min.x)
        .max(0.0)
        .floor() as u32;
    let min_y = a
        .pos
        .y
        .min(b.pos.y)
        .min(c.pos.y)
        .max(clip.min.y)
        .max(0.0)
        .floor() as u32;
    let max_x = a
        .pos
        .x
        .max(b.pos.x)
        .max(c.pos.x)
        .min(clip.max.x)
        .ceil()
        .min(bitmap.width() as f32) as u32;
    let max_y = a
        .pos
        .y
        .max(b.pos.y)
        .max(c.pos.y)
        .min(clip.max.y)
        .ceil()
        .min(bitmap.height() as f32) as u32;
    let colors = [a.color.to_array(), b.color.to_array(), c.color.to_array()];
    let opaque_fill = a.uv == egui::epaint::WHITE_UV
        && b.uv == a.uv
        && c.uv == a.uv
        && a.color == b.color
        && a.color == c.color
        && colors[0][3] == 255;
    for y in min_y..max_y {
        for x in min_x..max_x {
            let p = egui::pos2(x as f32 + 0.5, y as f32 + 0.5);
            if !clip.contains(p) {
                continue;
            }
            let weights = [
                edge(b.pos, c.pos, p),
                edge(c.pos, a.pos, p),
                edge(a.pos, b.pos, p),
            ];
            if weights
                .iter()
                .zip([
                    top_left(b.pos, c.pos),
                    top_left(c.pos, a.pos),
                    top_left(a.pos, b.pos),
                ])
                .any(|(&weight, inclusive)| weight < 0.0 || (weight == 0.0 && !inclusive))
            {
                continue;
            }
            let pixel = bitmap.get_pixel_mut(x, y);
            if opaque_fill {
                pixel.0.copy_from_slice(&colors[0][..3]);
                continue;
            }
            let weights = weights.map(|w| w / area);
            let uv = a.uv.to_vec2() * weights[0]
                + b.uv.to_vec2() * weights[1]
                + c.uv.to_vec2() * weights[2];
            let sample = sample_texture(texture, uv.x, uv.y);
            let mut color = [0.0; 4];
            for channel in 0..4 {
                color[channel] = sample[channel]
                    * (colors[0][channel] as f32 * weights[0]
                        + colors[1][channel] as f32 * weights[1]
                        + colors[2][channel] as f32 * weights[2])
                    / 255.0;
            }
            let inverse_alpha = 1.0 - color[3] / 255.0;
            for (channel, value) in pixel.0.iter_mut().enumerate() {
                *value = (color[channel] + *value as f32 * inverse_alpha)
                    .round()
                    .clamp(0.0, 255.0) as u8;
            }
        }
    }
}

fn sample_texture(texture: &ColorImage, u: f32, v: f32) -> [f32; 4] {
    let x = (u * texture.width() as f32 - 0.5).clamp(0.0, texture.width() as f32 - 1.0);
    let y = (v * texture.height() as f32 - 0.5).clamp(0.0, texture.height() as f32 - 1.0);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = (
        (x0 + 1).min(texture.width() - 1),
        (y0 + 1).min(texture.height() - 1),
    );
    let colors = [
        texture[(x0, y0)].to_array(),
        texture[(x1, y0)].to_array(),
        texture[(x0, y1)].to_array(),
        texture[(x1, y1)].to_array(),
    ];
    let (dx, dy) = (x - x0 as f32, y - y0 as f32);
    std::array::from_fn(|i| {
        (colors[0][i] as f32 * (1.0 - dx) + colors[1][i] as f32 * dx) * (1.0 - dy)
            + (colors[2][i] as f32 * (1.0 - dx) + colors[3][i] as f32 * dx) * dy
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use office_impress::{Bounds, ObjectKind, ShapeKind, SlideObject};
    use std::sync::Arc;

    fn with_fonts<T>(function: impl FnOnce(&egui::Context) -> T) -> T {
        let ctx = egui::Context::default();
        let mut function = Some(function);
        let mut result = None;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            result = Some(function.take().unwrap()(ui.ctx()));
        });
        output.textures_delta.clear();
        result.unwrap()
    }

    fn image_data(color: [u8; 4]) -> Arc<Vec<u8>> {
        let image = image::RgbaImage::from_pixel(2, 2, image::Rgba(color));
        let mut out = std::io::Cursor::new(Vec::new());
        image.write_to(&mut out, image::ImageFormat::Png).unwrap();
        Arc::new(out.into_inner())
    }

    #[test]
    fn raster_keeps_frontmost_shapes_ellipse_corners_and_image_transparency() {
        let mut slide = Slide::blank();
        slide.objects = vec![
            SlideObject {
                bounds: Bounds {
                    x: 0.1,
                    y: 0.1,
                    w: 0.4,
                    h: 0.4,
                },
                kind: ObjectKind::Shape {
                    shape: ShapeKind::Rectangle,
                    fill: [255, 0, 0],
                },
            },
            SlideObject {
                bounds: Bounds {
                    x: 0.2,
                    y: 0.2,
                    w: 0.4,
                    h: 0.4,
                },
                kind: ObjectKind::Shape {
                    shape: ShapeKind::Ellipse,
                    fill: [0, 0, 255],
                },
            },
            SlideObject {
                bounds: Bounds {
                    x: 0.5,
                    y: 0.5,
                    w: 0.2,
                    h: 0.2,
                },
                kind: ObjectKind::Image {
                    data: image_data([0, 255, 0, 128]),
                },
            },
        ];
        let bitmap = with_fonts(|ctx| raster_slide(ctx, &slide, &Theme::light(), 1)).unwrap();
        assert_eq!(bitmap.get_pixel(5, 5).0, [255, 255, 255]);
        assert_eq!(bitmap.get_pixel(145, 80).0, [255, 0, 0]);
        assert_eq!(bitmap.get_pixel(202, 114).0, [255, 0, 0]);
        assert_eq!(bitmap.get_pixel(384, 216).0, [0, 0, 255]);
        let green = bitmap.get_pixel(576, 324).0;
        assert_eq!(green[1], 255);
        assert!(
            green[0].abs_diff(127) <= 2 && green[2].abs_diff(127) <= 2,
            "{green:?}"
        );
    }

    #[test]
    fn text_wraps_and_remains_clipped_to_its_box() {
        let mut slide = Slide::blank();
        let bounds = Bounds {
            x: 0.2,
            y: 0.2,
            w: 0.1,
            h: 0.1,
        };
        slide.objects.push(SlideObject {
            bounds,
            kind: ObjectKind::Text {
                text: "XXXXXXXXXXXXXX\nmore text\nlast line".into(),
                font_pt: 24.0,
                color: [0, 0, 0],
            },
        });
        let bitmap = with_fonts(|ctx| raster_slide(ctx, &slide, &Theme::light(), 1)).unwrap();
        let mut ink = 0;
        for (x, y, pixel) in bitmap.enumerate_pixels() {
            if pixel.0 != [255, 255, 255] {
                assert!(
                    (192..288).contains(&x) && (108..162).contains(&y),
                    "ink outside box at {x},{y}"
                );
                ink += 1;
            }
        }
        assert!(ink > 100);
    }

    #[test]
    fn pdf_pages_have_widescreen_dimensions_and_slide_order_without_notes_or_state_changes() {
        let mut presentation = Presentation::new();
        let colors = [[255, 0, 0], [0, 255, 0], [0, 0, 255]];
        presentation.slides = colors
            .iter()
            .map(|&fill| {
                let mut slide = Slide::blank();
                slide.notes = "Private speaker notes".into();
                slide.objects.push(SlideObject {
                    bounds: Bounds {
                        x: 0.0,
                        y: 0.0,
                        w: 1.0,
                        h: 1.0,
                    },
                    kind: ObjectKind::Shape {
                        shape: ShapeKind::Rectangle,
                        fill,
                    },
                });
                slide
            })
            .collect();
        presentation.active = 2;
        presentation.mark_dirty();
        let original = presentation.clone();
        let bytes = with_fonts(|ctx| presentation_pdf_bytes(ctx, &presentation)).unwrap();
        assert_eq!(presentation, original);
        let doc = printpdf::lopdf::Document::load_mem(&bytes).unwrap();
        let pages = doc.get_pages();
        assert_eq!(pages.len(), 3);
        for (index, id) in pages.values().enumerate() {
            let page = doc.get_dictionary(*id).unwrap();
            let bounds = page.get(b"MediaBox").unwrap().as_array().unwrap();
            assert!((bounds[2].as_float().unwrap() - 960.0).abs() < 0.01);
            assert!((bounds[3].as_float().unwrap() - 540.0).abs() < 0.01);
            let resources = doc
                .dereference(page.get(b"Resources").unwrap())
                .unwrap()
                .1
                .as_dict()
                .unwrap();
            let objects = doc
                .dereference(resources.get(b"XObject").unwrap())
                .unwrap()
                .1
                .as_dict()
                .unwrap();
            let stream = objects
                .iter()
                .find_map(|(_, object)| doc.dereference(object).unwrap().1.as_stream().ok())
                .unwrap();
            assert_eq!(stream.dict.get(b"Width").unwrap().as_i64().unwrap(), 1920);
            assert_eq!(stream.dict.get(b"Height").unwrap().as_i64().unwrap(), 1080);
            let image = image::load_from_memory(&stream.content).unwrap().to_rgb8();
            let pixel = image.get_pixel(960, 540).0;
            for (got, wanted) in pixel.into_iter().zip(colors[index]) {
                assert!(got.abs_diff(wanted) <= 3);
            }
        }
        assert!(!String::from_utf8_lossy(&bytes).contains("Private speaker notes"));
    }

    #[test]
    fn invalid_images_geometry_and_page_limits_fail_without_overwriting_destination() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("existing.pdf");
        std::fs::write(&path, b"original pdf").unwrap();
        let mut presentation = Presentation::new();
        presentation.slides[0].objects.push(SlideObject {
            bounds: Bounds::default(),
            kind: ObjectKind::Image {
                data: Arc::new(b"bad image".to_vec()),
            },
        });
        assert!(with_fonts(|ctx| write_presentation_pdf_path(ctx, &presentation, &path)).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"original pdf");
        presentation.slides[0].objects.clear();
        presentation.slides[0].title.w = f32::NAN;
        assert!(with_fonts(|ctx| write_presentation_pdf_path(ctx, &presentation, &path)).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"original pdf");
        presentation.slides = vec![Slide::blank(); MAX_PDF_SLIDES + 1];
        assert!(with_fonts(|ctx| presentation_pdf_bytes(ctx, &presentation)).is_err());
        presentation.slides.clear();
        assert!(with_fonts(|ctx| presentation_pdf_bytes(ctx, &presentation)).is_err());
    }
}
