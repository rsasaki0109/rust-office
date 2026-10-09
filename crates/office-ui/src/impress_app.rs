//! Impress (presentation) application UI.

use std::{collections::HashMap, path::PathBuf, sync::Arc};

use eframe::App;
use egui::{self, Color32, RichText, Sense, Ui, Vec2};
use office_impress::{
    load_json_path, load_pptx_path, write_json_path, write_pptx_path, Bounds, ObjectKind,
    Presentation, ShapeKind, SlideObject, Theme, SLIDE_HEIGHT_PT, SLIDE_WIDTH_PT,
};

use crate::theme::{CANVAS_BG, STATUS_BG, TOOLBAR_BG};
use crate::unsaved::{self, Choice, DocumentAction};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchTo {
    Writer,
    Calc,
    Quit,
}

pub struct ImpressApp {
    presentation: Presentation,
    file_path: Option<PathBuf>,
    status_message: String,
    last_error: Option<String>,
    pending_action: Option<DocumentAction>,
    pending_delete: Option<usize>,
    edit_session: Option<(usize, usize)>,
    selected_object: usize,
    selection_slide: usize,
    drag_session: Option<(usize, usize, bool)>,
    textures: HashMap<usize, (Arc<Vec<u8>>, egui::TextureHandle)>,
    pub pending_switch: Option<SwitchTo>,
}

impl ImpressApp {
    pub fn new() -> Self {
        Self {
            presentation: Presentation::demo(),
            file_path: None,
            status_message: "Ready — Impress MVP (themes, slides, JSON, PPTX open/export)".into(),
            last_error: None,
            pending_action: None,
            pending_delete: None,
            edit_session: None,
            selected_object: 0,
            selection_slide: 0,
            drag_session: None,
            textures: HashMap::new(),
            pending_switch: None,
        }
    }

    pub fn take_switch(&mut self) -> Option<SwitchTo> {
        self.pending_switch.take()
    }

    pub(super) fn title(&self) -> String {
        let name = self
            .file_path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| self.presentation.title.clone());
        let dirty = if self.presentation.is_dirty() {
            " *"
        } else {
            ""
        };
        format!("rust-office — Impress — {name}{dirty}")
    }

    fn set_status(&mut self, msg: impl Into<String>) {
        self.status_message = msg.into();
        self.last_error = None;
    }

    fn set_error(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        self.last_error = Some(msg.clone());
        self.status_message = msg;
    }

    fn new_deck(&mut self) {
        self.request_action(DocumentAction::New);
    }

    fn do_new_deck(&mut self) {
        self.edit_session = None;
        self.textures.clear();
        self.selected_object = 0;
        self.drag_session = None;
        self.presentation = Presentation::new();
        self.file_path = None;
        self.set_status("New presentation");
    }

    fn open_file(&mut self) {
        self.request_action(DocumentAction::Open);
    }

    fn do_open_file(&mut self) {
        let path = rfd::FileDialog::new()
            .add_filter("PowerPoint", &["pptx"])
            .add_filter("rust-office Impress", &["rimpress.json", "json"])
            .pick_file();
        let Some(path) = path else {
            return;
        };
        self.open_path(&path);
    }

    fn open_path(&mut self, path: &std::path::Path) {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let result = if ext == "pptx" {
            load_pptx_path(path).map_err(|e| e.to_string())
        } else {
            load_json_path(path).map_err(|e| e.to_string())
        };
        match result {
            Ok(p) => {
                self.edit_session = None;
                self.textures.clear();
                self.selected_object = 0;
                self.drag_session = None;
                self.presentation = p;
                self.file_path = Some(path.to_path_buf());
                self.set_status(format!("Opened {}", path.display()));
            }
            Err(e) => self.set_error(format!("Open failed: {e}")),
        }
    }

    pub(super) fn is_dirty(&self) -> bool {
        self.presentation.is_dirty()
    }

    pub(super) fn cancel_pending_action(&mut self) {
        self.pending_action = None;
    }

    fn request_action(&mut self, action: DocumentAction) {
        if self.is_dirty() {
            self.pending_action = Some(action);
        } else {
            self.run_action(action);
        }
    }

    fn run_action(&mut self, action: DocumentAction) {
        match action {
            DocumentAction::New => self.do_new_deck(),
            DocumentAction::Open => self.do_open_file(),
        }
    }

    fn resolve_pending_action(&mut self, choice: Choice) {
        if choice == Choice::Save && !self.save_file() {
            self.pending_action = None;
            return;
        }
        if let Some(action) = self.pending_action.take() {
            if choice != Choice::Cancel {
                self.run_action(action);
            }
        }
    }

    fn show_unsaved_dialog(&mut self, ctx: &egui::Context) {
        if self.pending_action.is_some() {
            if let Some(choice) = unsaved::confirm(ctx, "impress_unsaved", &self.title()) {
                self.resolve_pending_action(choice);
            }
        }
    }

    pub(super) fn save_file(&mut self) -> bool {
        if let Some(path) = self.file_path.clone() {
            if is_json_destination(&path) {
                return self.save_json_to(&path);
            }
        }
        // PPTX import only models a subset of the original file. Save to a new
        // native file; PPTX export remains a separate, explicit operation.
        self.save_json_as()
    }

    fn save_json_as(&mut self) -> bool {
        let path = rfd::FileDialog::new()
            .add_filter("rust-office Impress", &["rimpress.json", "json"])
            .set_file_name("deck.rimpress.json")
            .save_file();
        let Some(path) = path else {
            return false;
        };
        self.save_json_to(&path)
    }

    fn save_json_to(&mut self, path: &std::path::Path) -> bool {
        if !is_json_destination(path) {
            self.set_error("Save failed: choose a .rimpress.json or .json file");
            return false;
        }
        match write_json_path(&self.presentation, path) {
            Ok(()) => {
                self.presentation.mark_clean();
                self.file_path = Some(path.to_path_buf());
                self.set_status(format!("Saved {}", path.display()));
                true
            }
            Err(e) => {
                self.set_error(format!("Save failed: {e}"));
                false
            }
        }
    }

    fn export_pptx(&mut self) {
        let path = rfd::FileDialog::new()
            .add_filter("PowerPoint", &["pptx"])
            .set_file_name("deck.pptx")
            .save_file();
        let Some(path) = path else {
            return;
        };
        match write_pptx_path(&self.presentation, &path) {
            Ok(()) => self.set_status(format!("Exported PPTX {}", path.display())),
            Err(e) => self.set_error(format!("PPTX export failed: {e}")),
        }
    }

    fn history(&mut self, redo: bool) {
        self.edit_session = None;
        self.drag_session = None;
        let changed = if redo {
            self.presentation.redo()
        } else {
            self.presentation.undo()
        };
        if changed {
            self.set_status(if redo { "Redone" } else { "Undone" });
        }
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        if self.pending_action.is_some()
            || self.pending_delete.is_some()
            || ctx.memory(|m| m.top_modal_layer().is_some())
        {
            return;
        }
        let command = egui::Modifiers::COMMAND;
        if ctx.input_mut(|i| {
            i.consume_key(command | egui::Modifiers::SHIFT, egui::Key::Z)
                || i.consume_key(command, egui::Key::Y)
        }) {
            self.history(true);
        } else if ctx.input_mut(|i| i.consume_key(command, egui::Key::Z)) {
            self.history(false);
        }
        if ctx.input_mut(|i| i.consume_key(command | egui::Modifiers::SHIFT, egui::Key::S)) {
            self.save_json_as();
        } else if ctx.input_mut(|i| i.consume_key(command, egui::Key::S)) {
            self.save_file();
        }
        if ctx.input_mut(|i| i.consume_key(command, egui::Key::O)) {
            self.open_file();
        }
        if ctx.input_mut(|i| i.consume_key(command, egui::Key::N)) {
            self.new_deck();
        }
    }

    fn show_delete_dialog(&mut self, ctx: &egui::Context) {
        let Some(index) = self.pending_delete else {
            return;
        };
        let response = egui::Modal::new(egui::Id::new("impress_delete_slide")).show(ctx, |ui| {
            ui.heading("Delete slide?");
            ui.label(format!("Delete slide {}? Undo can restore it.", index + 1));
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    self.pending_delete = None;
                }
                if ui.button("Delete").clicked() {
                    self.pending_delete = None;
                    self.edit_session = None;
                    if self.presentation.active == index && self.presentation.delete_active_slide()
                    {
                        self.set_status("Slide deleted");
                    }
                }
            });
        });
        if response.should_close() {
            self.pending_delete = None;
        }
    }

    fn menu_bar(&mut self, ui: &mut Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("New Presentation").clicked() {
                    self.new_deck();
                    ui.close();
                }
                if ui.button("Open…").clicked() {
                    self.open_file();
                    ui.close();
                }
                if ui.button("Save").clicked() {
                    self.save_file();
                    ui.close();
                }
                if ui.button("Save As…").clicked() {
                    self.save_json_as();
                    ui.close();
                }
                if ui.button("Export PPTX…").clicked() {
                    self.export_pptx();
                    ui.close();
                }
                ui.separator();
                if ui.button("Switch to Writer").clicked() {
                    self.pending_switch = Some(SwitchTo::Writer);
                    ui.close();
                }
                if ui.button("Switch to Calc").clicked() {
                    self.pending_switch = Some(SwitchTo::Calc);
                    ui.close();
                }
                ui.separator();
                if ui.button("Quit").clicked() {
                    self.pending_switch = Some(SwitchTo::Quit);
                    ui.close();
                }
            });
            ui.menu_button("Edit", |ui| {
                if ui
                    .add_enabled(
                        self.presentation.can_undo(),
                        egui::Button::new("Undo  Ctrl+Z"),
                    )
                    .clicked()
                {
                    self.history(false);
                    ui.close();
                }
                if ui
                    .add_enabled(
                        self.presentation.can_redo(),
                        egui::Button::new("Redo  Ctrl+Shift+Z"),
                    )
                    .clicked()
                {
                    self.history(true);
                    ui.close();
                }
            });
            ui.menu_button("Insert", |ui| {
                if ui.button("New Slide").clicked() {
                    self.edit_session = None;
                    self.presentation.insert_slide_after_current();
                    self.set_status("Slide inserted");
                    ui.close();
                }
                ui.separator();
                if ui.button("Text box").clicked() {
                    self.insert_object(SlideObject::text());
                    ui.close();
                }
                if ui.button("Rectangle").clicked() {
                    self.insert_object(SlideObject::shape(ShapeKind::Rectangle));
                    ui.close();
                }
                if ui.button("Ellipse").clicked() {
                    self.insert_object(SlideObject::shape(ShapeKind::Ellipse));
                    ui.close();
                }
                if ui.button("Image…").clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Image", &["png", "jpg", "jpeg", "gif", "webp", "bmp"])
                        .pick_file()
                    {
                        self.insert_image_path(&path);
                    }
                    ui.close();
                }
            });
            ui.menu_button("Theme", |ui| {
                for theme in Theme::builtins() {
                    let selected = self.presentation.theme.name == theme.name;
                    if ui.selectable_label(selected, theme.name.clone()).clicked() {
                        self.edit_session = None;
                        self.presentation.apply_theme(theme);
                        self.set_status("Theme applied");
                        ui.close();
                    }
                }
            });
        });
    }

    fn slide_list(&mut self, ui: &mut Ui) {
        ui.vertical(|ui| {
            ui.label(RichText::new("Slides").strong());
            ui.separator();
            let count = self.presentation.slides.len();
            let active = self.presentation.active;
            for i in 0..count {
                let label = self.presentation.slides[i]
                    .title
                    .text
                    .lines()
                    .next()
                    .filter(|s| !s.is_empty())
                    .map(|s| format!("{}. {s}", i + 1))
                    .unwrap_or_else(|| format!("{}. (untitled)", i + 1));
                if ui.selectable_label(i == active, label).clicked() {
                    self.edit_session = None;
                    self.presentation.set_active(i);
                }
            }
            ui.separator();
            if ui.button("+ Slide").clicked() {
                self.edit_session = None;
                self.presentation.add_slide();
            }
            if ui.button("Duplicate slide").clicked() {
                self.edit_session = None;
                self.presentation.duplicate_active_slide();
            }
            let active = self.presentation.active;
            if ui
                .add_enabled(active > 0, egui::Button::new("Move up"))
                .clicked()
            {
                self.edit_session = None;
                self.presentation.move_active_slide(active - 1);
            }
            if ui
                .add_enabled(
                    active + 1 < self.presentation.slides.len(),
                    egui::Button::new("Move down"),
                )
                .clicked()
            {
                self.edit_session = None;
                self.presentation.move_active_slide(active + 1);
            }
            if ui
                .add_enabled(
                    self.presentation.slides.len() > 1,
                    egui::Button::new("Delete slide…"),
                )
                .clicked()
            {
                self.pending_delete = Some(self.presentation.active);
            }
        });
    }

    fn insert_object(&mut self, object: SlideObject) {
        let Some(mut slide) = self.presentation.active_slide().cloned() else {
            return;
        };
        self.edit_session = None;
        self.drag_session = None;
        self.selected_object = slide.objects.len() + 2;
        slide.objects.push(object);
        self.presentation.update_active_slide(slide, false);
        self.set_status("Object inserted — drag to move; use the corner to resize");
    }

    fn insert_image_path(&mut self, path: &std::path::Path) {
        match SlideObject::image_path(path) {
            Ok(object) => self.insert_object(object),
            Err(error) => self.set_error(format!("Image insertion failed: {error}")),
        }
    }

    fn object_editor(&mut self, ui: &mut Ui) {
        let Some(mut slide) = self.presentation.active_slide().cloned() else {
            return;
        };
        if self.selection_slide != self.presentation.active {
            self.selection_slide = self.presentation.active;
            self.selected_object = 0;
            self.drag_session = None;
            self.edit_session = None;
        }
        self.selected_object = self.selected_object.min(slide.objects.len() + 1);
        ui.label(RichText::new("Object").strong());
        egui::ComboBox::from_id_salt("impress_object_selection")
            .selected_text(object_label(&slide, self.selected_object))
            .show_ui(ui, |ui| {
                for index in 0..slide.objects.len() + 2 {
                    if ui
                        .selectable_value(
                            &mut self.selected_object,
                            index,
                            object_label(&slide, index),
                        )
                        .changed()
                    {
                        self.edit_session = None;
                        self.drag_session = None;
                    }
                }
            });
        let mut bounds = object_bounds(&slide, self.selected_object);
        let mut changed = false;
        egui::Grid::new("impress_geometry")
            .num_columns(2)
            .show(ui, |ui| {
                for (label, value) in [
                    ("X (%)", &mut bounds.x),
                    ("Y (%)", &mut bounds.y),
                    ("Width (%)", &mut bounds.w),
                    ("Height (%)", &mut bounds.h),
                ] {
                    ui.label(label);
                    let mut percent = *value * 100.0;
                    let response = ui.add(
                        egui::DragValue::new(&mut percent)
                            .speed(0.5)
                            .range(0.0..=100.0),
                    );
                    if response.changed() {
                        *value = percent / 100.0;
                        changed = true;
                    }
                    ui.end_row();
                }
            });
        if self.selected_object >= 2 {
            let object = &mut slide.objects[self.selected_object - 2];
            match &mut object.kind {
                ObjectKind::Text {
                    text,
                    font_pt,
                    color,
                } => {
                    let response = ui.add(
                        egui::TextEdit::multiline(text)
                            .id_salt((
                                "object_text",
                                self.presentation.active,
                                self.selected_object,
                            ))
                            .desired_rows(2),
                    );
                    if response.changed() {
                        let session = (self.presentation.active, self.selected_object + 3);
                        let coalesce = self.edit_session == Some(session);
                        self.edit_session = Some(session);
                        self.presentation.update_active_slide(slide, coalesce);
                        return;
                    }
                    if !response.has_focus()
                        && self.edit_session.is_some_and(|(_, field)| field >= 3)
                    {
                        self.edit_session = None;
                    }
                    ui.horizontal(|ui| {
                        ui.label("Font size");
                        changed |= ui
                            .add(egui::DragValue::new(font_pt).range(1.0..=200.0))
                            .changed();
                    });
                    if !font_pt.is_finite() {
                        *font_pt = 24.0;
                    }
                    changed |= ui.color_edit_button_srgb(color).changed();
                }
                ObjectKind::Shape { fill, .. } => {
                    changed |= ui.color_edit_button_srgb(fill).changed();
                }
                ObjectKind::Image { .. } => {
                    ui.label("Image embedded in this document");
                }
            }
            if ui.button("Remove object").clicked() {
                slide.objects.remove(self.selected_object - 2);
                self.selected_object = 0;
                self.edit_session = None;
                self.drag_session = None;
                self.presentation.update_active_slide(slide, false);
                return;
            }
        }
        if changed {
            bounds.constrain();
            set_object_bounds(&mut slide, self.selected_object, bounds);
            self.edit_session = None;
            self.presentation.update_active_slide(slide, false);
        }
        ui.separator();
    }

    fn canvas(&mut self, ui: &mut Ui) {
        let Some(mut slide) = self.presentation.active_slide().cloned() else {
            return;
        };
        let theme = self.presentation.theme.clone();
        let avail = ui.available_size();
        let aspect = SLIDE_WIDTH_PT / SLIDE_HEIGHT_PT;
        let w = (avail.x - 16.0)
            .max(120.0)
            .min((avail.y - 16.0).max(80.0) * aspect);
        let h = w / aspect;
        let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
        let painter = ui.painter().with_clip_rect(rect);
        painter.rect_filled(rect, 0.0, rgb(theme.background));
        let scale = h / SLIDE_HEIGHT_PT;
        self.selected_object = self.selected_object.min(slide.objects.len() + 1);
        let mut active_images = Vec::new();
        for (index, object) in slide.objects.iter().enumerate() {
            if let ObjectKind::Image { data } = &object.kind {
                let key = Arc::as_ptr(data) as usize;
                active_images.push(key);
                if let std::collections::hash_map::Entry::Vacant(entry) = self.textures.entry(key) {
                    if let Ok(image) = office_impress::decode_image(data) {
                        let rgba = image.thumbnail(2048, 2048).to_rgba8();
                        let color = egui::ColorImage::from_rgba_unmultiplied(
                            [rgba.width() as usize, rgba.height() as usize],
                            rgba.as_raw(),
                        );
                        entry.insert((
                            data.clone(),
                            ui.ctx().load_texture(
                                format!("impress_image_{index}"),
                                color,
                                egui::TextureOptions::LINEAR,
                            ),
                        ));
                    }
                }
            }
        }
        self.textures.retain(|key, _| active_images.contains(key));
        let mut drag = None;
        // Register in paint order: the last (frontmost) interaction wins overlaps.
        for index in 0..slide.objects.len() + 2 {
            let bounds = object_bounds(&slide, index);
            let object_rect = bounds_rect(rect, bounds);
            let response = ui.interact(
                object_rect,
                egui::Id::new(("impress_move", self.presentation.active, index)),
                Sense::click_and_drag(),
            );
            if response.clicked() || response.drag_started() {
                self.selected_object = index;
                self.edit_session = None;
            }
            if response.dragged() {
                drag = Some((index, false));
            }
        }
        let selected_rect = bounds_rect(rect, object_bounds(&slide, self.selected_object));
        let handle_rect =
            egui::Rect::from_center_size(selected_rect.right_bottom(), Vec2::splat(12.0));
        let handle = ui.interact(
            handle_rect,
            egui::Id::new((
                "impress_resize",
                self.presentation.active,
                self.selected_object,
            )),
            Sense::drag(),
        );
        if handle.dragged() {
            drag = Some((self.selected_object, true));
        }
        if let Some((index, resize)) = drag {
            let delta = ui.input(|i| i.pointer.delta());
            let mut bounds = object_bounds(&slide, index);
            if resize {
                bounds.w = (bounds.w + delta.x / w).clamp(0.01, 1.0 - bounds.x);
                bounds.h = (bounds.h + delta.y / h).clamp(0.01, 1.0 - bounds.y);
            } else {
                bounds.x += delta.x / w;
                bounds.y += delta.y / h;
            }
            bounds.constrain();
            set_object_bounds(&mut slide, index, bounds);
            let session = (self.presentation.active, index, resize);
            self.presentation
                .update_active_slide(slide.clone(), self.drag_session == Some(session));
            self.drag_session = Some(session);
            self.edit_session = None;
        } else {
            self.drag_session = None;
        }
        for index in 0..slide.objects.len() + 2 {
            let object_rect = bounds_rect(rect, object_bounds(&slide, index));
            let clipped = painter.with_clip_rect(object_rect.intersect(rect));
            if index < 2 {
                let (text, font, color) = if index == 0 {
                    (&slide.title.text, theme.title_font_pt, theme.title_color)
                } else {
                    (&slide.body.text, theme.body_font_pt, theme.body_color)
                };
                paint_text(&clipped, object_rect, text, font * scale, rgb(color));
            } else {
                match &slide.objects[index - 2].kind {
                    ObjectKind::Text {
                        text,
                        font_pt,
                        color,
                    } => paint_text(&clipped, object_rect, text, font_pt * scale, rgb(*color)),
                    ObjectKind::Shape { shape, fill } => match shape {
                        ShapeKind::Rectangle => {
                            clipped.rect_filled(object_rect, 0.0, rgb(*fill));
                        }
                        ShapeKind::Ellipse => {
                            clipped.add(egui::epaint::EllipseShape::filled(
                                object_rect.center(),
                                object_rect.size() / 2.0,
                                rgb(*fill),
                            ));
                        }
                    },
                    ObjectKind::Image { data } => {
                        if let Some((_, texture)) = self.textures.get(&(Arc::as_ptr(data) as usize))
                        {
                            clipped.image(
                                texture.id(),
                                object_rect,
                                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                                Color32::WHITE,
                            );
                        }
                    }
                }
            }
        }
        let selected_rect = bounds_rect(rect, object_bounds(&slide, self.selected_object));
        painter.rect_stroke(
            selected_rect,
            0.0,
            egui::Stroke::new(1.5, Color32::from_rgb(50, 120, 220)),
            egui::StrokeKind::Inside,
        );
        painter.rect_filled(
            egui::Rect::from_center_size(selected_rect.right_bottom(), Vec2::splat(8.0)),
            0.0,
            Color32::from_rgb(50, 120, 220),
        );
    }
}

impl Default for ImpressApp {
    fn default() -> Self {
        Self::new()
    }
}

impl App for ImpressApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(self.title()));
        self.handle_keys(&ctx);
        self.show_unsaved_dialog(&ctx);
        self.show_delete_dialog(&ctx);
        if self.pending_action.is_some() || self.pending_delete.is_some() {
            ui.disable();
        }

        egui::Panel::top("impress_menu")
            .frame(egui::Frame::new().fill(TOOLBAR_BG).inner_margin(4.0))
            .show(ui, |ui| {
                self.menu_bar(ui);
            });

        egui::Panel::bottom("impress_status")
            .frame(egui::Frame::new().fill(STATUS_BG).inner_margin(6.0))
            .show(ui, |ui| {
                let color = if self.last_error.is_some() {
                    Color32::from_rgb(180, 40, 40)
                } else {
                    Color32::DARK_GRAY
                };
                let n = self.presentation.slides.len();
                let active = self.presentation.active + 1;
                let theme = self.presentation.theme.name.clone();
                ui.horizontal(|ui| {
                    ui.colored_label(color, &self.status_message);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(format!("Slide {active}/{n} · {theme}"));
                    });
                });
            });

        egui::Panel::left("impress_slides")
            .resizable(true)
            .default_size(180.0)
            .frame(egui::Frame::new().fill(TOOLBAR_BG).inner_margin(8.0))
            .show(ui, |ui| {
                self.slide_list(ui);
            });

        egui::Panel::right("impress_edit")
            .resizable(true)
            .default_size(280.0)
            .frame(egui::Frame::new().fill(TOOLBAR_BG).inner_margin(8.0))
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    self.object_editor(ui);
                    ui.label(RichText::new("Edit").strong());
                    ui.separator();
                    if let Some(mut slide) = self.presentation.active_slide().cloned() {
                        let mut changed_field = None;
                        let mut focused = false;
                        for (field, label, text, rows) in [
                            (0, "Title", &mut slide.title.text, 2),
                            (1, "Body", &mut slide.body.text, 10),
                            (2, "Notes", &mut slide.notes, 3),
                        ] {
                            ui.label(label);
                            let response = ui.add(
                                egui::TextEdit::multiline(text)
                                    .id(egui::Id::new((
                                        "impress_text",
                                        self.presentation.active,
                                        field,
                                    )))
                                    .desired_rows(rows)
                                    .desired_width(f32::INFINITY),
                            );
                            focused |= response.has_focus();
                            if response.changed() {
                                changed_field = Some(field);
                            }
                            ui.add_space(8.0);
                        }
                        if let Some(field) = changed_field {
                            let session = (self.presentation.active, field);
                            self.presentation
                                .update_active_slide(slide, self.edit_session == Some(session));
                            self.edit_session = Some(session);
                        } else if !focused && self.edit_session.is_some_and(|(_, field)| field < 3)
                        {
                            self.edit_session = None;
                        }
                    }
                });
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(CANVAS_BG).inner_margin(12.0))
            .show(ui, |ui| {
                self.canvas(ui);
            });
    }
}

fn is_json_destination(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_pptx_open_retains_slides_selection_path_and_unsaved_edits() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("broken.pptx");
        // A valid ZIP with no presentation manifest must not replace the deck.
        std::fs::write(&bad, b"PK\x05\x06\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0").unwrap();
        let mut app = ImpressApp::new();
        app.file_path = Some(dir.path().join("original.rimpress.json"));
        app.presentation.set_active(1);
        app.presentation.active_slide_mut().unwrap().body.text = "keep edited body".into();
        app.presentation.mark_dirty();
        let before = app.presentation.clone();
        let path = app.file_path.clone();
        let title = app.title();
        app.open_path(&bad);
        assert_eq!(app.presentation, before);
        assert_eq!(app.file_path, path);
        assert_eq!(app.title(), title);
        assert!(app.is_dirty());
        assert!(app
            .last_error
            .as_ref()
            .unwrap()
            .contains("ppt/presentation.xml"));
        // The retained edits can still be saved after the failure.
        assert!(app.save_file());
        assert_eq!(
            load_json_path(app.file_path.as_ref().unwrap())
                .unwrap()
                .slides,
            before.slides
        );
    }

    #[test]
    fn successful_pptx_open_replaces_the_deck_and_resets_active_and_dirty_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ordered.pptx");
        let mut deck = Presentation::demo();
        deck.slides.reverse();
        write_pptx_path(&deck, &path).unwrap();
        let mut app = ImpressApp::new();
        app.presentation.set_active(2);
        app.presentation.mark_dirty();
        app.open_path(&path);
        assert_eq!(app.presentation.slides, deck.slides);
        assert_eq!(app.presentation.active, 0);
        assert!(!app.is_dirty());
        assert_eq!(app.file_path, Some(path));
        assert!(app.last_error.is_none());
    }

    #[test]
    fn json_save_cannot_overwrite_imported_pptx() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("original.pptx");
        let mut app = ImpressApp::new();
        write_pptx_path(&app.presentation, &path).unwrap();
        let original = std::fs::read(&path).unwrap();
        app.presentation = load_pptx_path(&path).unwrap();
        app.file_path = Some(path.clone());
        app.presentation.mark_dirty();
        assert!(!app.save_json_to(&path));
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert!(load_pptx_path(&path).is_ok());
        assert_eq!(app.file_path, Some(path));
        assert!(app.is_dirty());
    }

    #[test]
    fn failed_save_as_preserves_old_destination_and_dirty_state() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("original.json");
        let bad = dir.path().join("destination.json");
        std::fs::write(&original, "original file").unwrap();
        std::fs::create_dir(&bad).unwrap();
        let mut app = ImpressApp::new();
        app.file_path = Some(original.clone());
        app.presentation.mark_dirty();
        assert!(!app.save_json_to(&bad));
        assert_eq!(app.file_path, Some(original.clone()));
        assert_eq!(std::fs::read_to_string(original).unwrap(), "original file");
        assert!(app.is_dirty());
    }

    #[test]
    fn successful_save_updates_destination_and_can_be_reopened() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deck.rimpress.json");
        let mut app = ImpressApp::new();
        app.presentation.mark_dirty();
        assert!(app.save_json_to(&path));
        assert!(!app.is_dirty());
        assert_eq!(app.file_path.as_deref(), Some(path.as_path()));
        assert_eq!(
            load_json_path(&path).unwrap().slides,
            app.presentation.slides
        );
    }

    #[test]
    fn cancel_new_or_open_retains_edited_slides() {
        for action in [DocumentAction::New, DocumentAction::Open] {
            let mut app = ImpressApp::new();
            app.presentation.active_slide_mut().unwrap().title.text = "keep edits".into();
            app.presentation.mark_dirty();
            app.request_action(action);
            assert_eq!(app.pending_action, Some(action));
            app.resolve_pending_action(Choice::Cancel);
            assert!(app.pending_action.is_none());
            assert_eq!(
                app.presentation.active_slide().unwrap().title.text,
                "keep edits"
            );
            assert!(app.is_dirty());
        }
    }

    #[test]
    fn save_failure_blocks_new_and_explicit_discard_allows_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("destination.json");
        std::fs::create_dir(&path).unwrap();
        let mut app = ImpressApp::new();
        app.file_path = Some(path);
        app.presentation.mark_dirty();
        let slides = app.presentation.slides.clone();
        app.new_deck();
        app.resolve_pending_action(Choice::Save);
        assert_eq!(app.presentation.slides, slides);
        assert!(app.is_dirty());
        app.new_deck();
        app.resolve_pending_action(Choice::Discard);
        assert!(!app.is_dirty());
        assert!(app.file_path.is_none());
        assert_eq!(app.presentation.slides.len(), 1);
    }
}

#[cfg(test)]
mod history_tests {
    use super::*;

    #[test]
    fn failed_open_and_save_keep_history_then_successful_save_tracks_revision() {
        let mut app = ImpressApp::new();
        app.presentation.duplicate_active_slide();
        let slides = app.presentation.slides.clone();
        app.open_path(std::path::Path::new("/nonexistent/impress.pptx"));
        assert_eq!(app.presentation.slides, slides);
        assert!(app.presentation.can_undo());
        let dir = tempfile::tempdir().unwrap();
        let blocked = dir.path().join("blocked");
        std::fs::write(&blocked, b"not a directory").unwrap();
        assert!(!app.save_json_to(&blocked.join("deck.json")));
        assert!(app.presentation.is_dirty());
        assert!(app.presentation.can_undo());
        let path = dir.path().join("deck.json");
        assert!(app.save_json_to(&path));
        app.presentation.move_active_slide(3);
        app.history(false);
        assert!(!app.is_dirty());
        app.history(false);
        assert!(app.is_dirty());
        app.history(true);
        assert!(!app.is_dirty());
        assert_eq!(load_json_path(&path).unwrap().slides, slides);
    }

    #[test]
    fn duplicate_and_reorder_export_in_pptx_slide_order() {
        let mut app = ImpressApp::new();
        app.presentation.duplicate_active_slide();
        app.presentation.move_active_slide(3);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reordered.pptx");
        write_pptx_path(&app.presentation, &path).unwrap();
        let loaded = load_pptx_path(&path).unwrap();
        let text = |p: &Presentation| p.slides.iter().map(|s| s.plain_text()).collect::<Vec<_>>();
        assert_eq!(text(&loaded), text(&app.presentation));
        assert!(app.is_dirty());
    }
}

fn rgb(color: [u8; 3]) -> Color32 {
    Color32::from_rgb(color[0], color[1], color[2])
}
fn object_label(slide: &office_impress::Slide, index: usize) -> String {
    match index {
        0 => "Title".into(),
        1 => "Body".into(),
        _ => format!(
            "{}: {}",
            index - 1,
            match &slide.objects[index - 2].kind {
                ObjectKind::Text { .. } => "Text box",
                ObjectKind::Shape {
                    shape: ShapeKind::Rectangle,
                    ..
                } => "Rectangle",
                ObjectKind::Shape { .. } => "Ellipse",
                ObjectKind::Image { .. } => "Image",
            }
        ),
    }
}
fn object_bounds(slide: &office_impress::Slide, index: usize) -> Bounds {
    if index >= 2 {
        return slide.objects[index - 2].bounds;
    }
    let text = if index == 0 {
        &slide.title
    } else {
        &slide.body
    };
    Bounds {
        x: text.x,
        y: text.y,
        w: text.w,
        h: text.h,
    }
}
fn set_object_bounds(slide: &mut office_impress::Slide, index: usize, bounds: Bounds) {
    if index >= 2 {
        slide.objects[index - 2].bounds = bounds;
        return;
    }
    let text = if index == 0 {
        &mut slide.title
    } else {
        &mut slide.body
    };
    text.x = bounds.x;
    text.y = bounds.y;
    text.w = bounds.w;
    text.h = bounds.h;
}
fn bounds_rect(rect: egui::Rect, bounds: Bounds) -> egui::Rect {
    egui::Rect::from_min_size(
        rect.min + egui::vec2(bounds.x * rect.width(), bounds.y * rect.height()),
        egui::vec2(bounds.w * rect.width(), bounds.h * rect.height()),
    )
}
fn paint_text(painter: &egui::Painter, rect: egui::Rect, text: &str, size: f32, color: Color32) {
    let galley = painter.layout(
        text.into(),
        egui::FontId::proportional(size),
        color,
        rect.width(),
    );
    painter.galley(rect.min, galley, color);
}

#[cfg(test)]
mod object_tests {
    use super::*;

    #[test]
    fn object_insert_remove_save_and_failed_open_preserve_history() {
        let mut app = ImpressApp::new();
        app.insert_object(SlideObject::text());
        app.insert_object(SlideObject::shape(ShapeKind::Ellipse));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("objects.json");
        assert!(app.save_json_to(&path));
        let original = app.presentation.slides.clone();
        let bad = dir.path().join("bad.json");
        let json = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&bad, json.replacen("\"w\": 0.3", "\"w\": -0.4", 1)).unwrap();
        app.open_path(&bad);
        assert_eq!(app.presentation.slides, original);
        assert_eq!(app.file_path.as_ref(), Some(&path));
        app.history(false);
        assert!(app.is_dirty());
        assert_eq!(app.presentation.slides[0].objects.len(), 1);
        app.history(true);
        assert!(!app.is_dirty());
        app.open_path(&path);
        assert_eq!(app.presentation.slides, original);
        assert!(!app.presentation.can_undo());
    }

    #[test]
    fn failed_image_insertion_does_not_replace_selection_or_document() {
        let mut app = ImpressApp::new();
        app.insert_object(SlideObject::shape(ShapeKind::Rectangle));
        let slides = app.presentation.slides.clone();
        let selected = app.selected_object;
        let dir = tempfile::tempdir().unwrap();
        let corrupt = dir.path().join("bad.png");
        std::fs::write(&corrupt, b"invalid image").unwrap();
        app.insert_image_path(&corrupt);
        assert!(app
            .last_error
            .as_ref()
            .unwrap()
            .contains("Image insertion failed"));
        assert_eq!(app.presentation.slides, slides);
        assert_eq!(app.selected_object, selected);
        app.history(false);
        assert!(!app.is_dirty());
        assert!(app.presentation.slides[0].objects.is_empty());
    }
}

#[cfg(test)]
mod canvas_tests {
    use super::*;

    #[test]
    fn dragging_frontmost_shape_does_not_move_overlapping_title() {
        let mut app = ImpressApp::new();
        app.insert_object(SlideObject::shape(ShapeKind::Rectangle));
        let title = app.presentation.slides[0].title.clone();
        let bounds = app.presentation.slides[0].objects[0].bounds;
        let ctx = egui::Context::default();
        let mut frame = |events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000.0, 600.0),
                )),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| app.canvas(ui));
            });
            output.textures_delta.clear();
        };
        frame(vec![]);
        let start = egui::pos2(300.0, 160.0);
        frame(vec![egui::Event::PointerMoved(start)]);
        frame(vec![egui::Event::PointerButton {
            pos: start,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        }]);
        let end = egui::pos2(360.0, 190.0);
        frame(vec![egui::Event::PointerMoved(end)]);
        frame(vec![egui::Event::PointerButton {
            pos: end,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        assert_eq!(app.presentation.slides[0].title, title);
        assert!(app.presentation.slides[0].objects[0].bounds.x > bounds.x);
        app.history(false);
        assert_eq!(app.presentation.slides[0].objects[0].bounds, bounds);
    }
}
