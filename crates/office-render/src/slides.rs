//! Shared slide scene for the editor, slide show and raster PDF export.
use std::{collections::HashMap, sync::Arc};

use egui::{
    epaint::{ClippedShape, Shape},
    Color32, FontId, Pos2, Rect, TextureHandle, TextureId, Ui,
};
use office_impress::{Bounds, ObjectKind, ShapeKind, Slide, Theme, SLIDE_HEIGHT_PT};

#[derive(Default)]
pub struct SlideTextures {
    entries: HashMap<usize, (Arc<Vec<u8>>, TextureHandle)>,
}

impl SlideTextures {
    pub fn clear(&mut self) {
        self.entries.clear();
    }
    pub fn paint(
        &mut self,
        ui: &Ui,
        rect: Rect,
        slide: &Slide,
        theme: &Theme,
    ) -> Result<(), String> {
        let mut active = Vec::new();
        let shapes = slide_shapes(ui.ctx(), rect, slide, theme, |data| {
            let key = Arc::as_ptr(data) as usize;
            active.push(key);
            if let std::collections::hash_map::Entry::Vacant(entry) = self.entries.entry(key) {
                let image = slide_image(data)?;
                let texture = ui.ctx().load_texture(
                    format!("impress_image_{key}"),
                    image,
                    egui::TextureOptions::LINEAR,
                );
                entry.insert((data.clone(), texture));
            }
            Ok(self.entries[&key].1.id())
        })?;
        self.entries.retain(|key, _| active.contains(key));
        for shape in shapes {
            ui.painter()
                .with_clip_rect(shape.clip_rect)
                .add(shape.shape);
        }
        Ok(())
    }
}

pub(crate) fn slide_image(data: &[u8]) -> Result<egui::ColorImage, String> {
    let image = office_impress::decode_image(data)?
        .thumbnail(2048, 2048)
        .to_rgba8();
    Ok(egui::ColorImage::from_rgba_unmultiplied(
        [image.width() as usize, image.height() as usize],
        image.as_raw(),
    ))
}

pub(crate) fn slide_shapes(
    ctx: &egui::Context,
    rect: Rect,
    slide: &Slide,
    theme: &Theme,
    mut image_texture: impl FnMut(&Arc<Vec<u8>>) -> Result<TextureId, String>,
) -> Result<Vec<ClippedShape>, String> {
    for size in [theme.title_font_pt, theme.body_font_pt] {
        if !size.is_finite() || !(1.0..=200.0).contains(&size) {
            return Err("Invalid theme font size".into());
        }
    }
    let mut shapes = vec![ClippedShape {
        clip_rect: rect,
        shape: Shape::rect_filled(rect, 0.0, rgb(theme.background)),
    }];
    let scale = rect.height() / SLIDE_HEIGHT_PT;
    for (text, size, color) in [
        (&slide.title, theme.title_font_pt, theme.title_color),
        (&slide.body, theme.body_font_pt, theme.body_color),
    ] {
        let bounds = Bounds {
            x: text.x,
            y: text.y,
            w: text.w,
            h: text.h,
        };
        if !bounds.is_valid() {
            return Err("Invalid text box geometry".into());
        }
        add_text(
            ctx,
            &mut shapes,
            rect,
            bounds,
            &text.text,
            size * scale,
            rgb(color),
        );
    }
    for object in &slide.objects {
        if !object.bounds.is_valid() {
            return Err("Invalid object geometry".into());
        }
        let object_rect = object_rect(rect, object.bounds);
        let clip_rect = object_rect.intersect(rect);
        let shape = match &object.kind {
            ObjectKind::Text {
                text,
                font_pt,
                color,
            } => {
                if !font_pt.is_finite() || !(1.0..=200.0).contains(font_pt) {
                    return Err("Invalid text box font size".into());
                }
                add_text(
                    ctx,
                    &mut shapes,
                    rect,
                    object.bounds,
                    text,
                    font_pt * scale,
                    rgb(*color),
                );
                continue;
            }
            ObjectKind::Shape {
                shape: ShapeKind::Rectangle,
                fill,
            } => Shape::rect_filled(object_rect, 0.0, rgb(*fill)),
            ObjectKind::Shape {
                shape: ShapeKind::Ellipse,
                fill,
            } => Shape::Ellipse(egui::epaint::EllipseShape::filled(
                object_rect.center(),
                object_rect.size() / 2.0,
                rgb(*fill),
            )),
            ObjectKind::Image { data } => Shape::image(
                image_texture(data)?,
                object_rect,
                Rect::from_min_max(Pos2::ZERO, egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            ),
        };
        shapes.push(ClippedShape { clip_rect, shape });
    }
    Ok(shapes)
}

fn add_text(
    ctx: &egui::Context,
    shapes: &mut Vec<ClippedShape>,
    rect: Rect,
    bounds: Bounds,
    text: &str,
    size: f32,
    color: Color32,
) {
    let object_rect = object_rect(rect, bounds);
    let galley = ctx.fonts_mut(|fonts| {
        fonts.layout(
            text.into(),
            FontId::proportional(size),
            color,
            object_rect.width(),
        )
    });
    shapes.push(ClippedShape {
        clip_rect: object_rect.intersect(rect),
        shape: Shape::galley(object_rect.min, galley, color),
    });
}

fn object_rect(rect: Rect, bounds: Bounds) -> Rect {
    Rect::from_min_size(
        rect.min + egui::vec2(bounds.x * rect.width(), bounds.y * rect.height()),
        egui::vec2(bounds.w * rect.width(), bounds.h * rect.height()),
    )
}
fn rgb(color: [u8; 3]) -> Color32 {
    Color32::from_rgb(color[0], color[1], color[2])
}
