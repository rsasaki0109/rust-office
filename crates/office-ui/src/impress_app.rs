//! Impress (presentation) application UI.

use std::{path::PathBuf, sync::mpsc};

use eframe::App;
use egui::{self, Color32, RichText, Sense, Ui, Vec2};
use office_impress::{
    load_json_path, load_pptx_path, write_json_path, write_pptx_path, Bounds, ObjectKind,
    Presentation, ShapeKind, SlideObject, Theme, SLIDE_HEIGHT_PT, SLIDE_WIDTH_PT,
};

use crate::slideshow::{self, Navigation, SlideShow};
use crate::theme::{CANVAS_BG, STATUS_BG, TOOLBAR_BG};
use crate::unsaved::{self, Choice, DocumentAction};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchTo {
    Writer,
    Calc,
    Quit,
}

struct PdfExport {
    path: PathBuf,
    result: mpsc::Receiver<Result<(), String>>,
}

pub struct ImpressApp {
    recovery: Option<crate::recovery::Recovery<crate::impress_recovery::ImpressSnapshot>>,
    recovery_open: bool,
    recovery_selected: usize,
    recovery_delete: Option<usize>,
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
    textures: office_render::SlideTextures,
    pdf_export: Option<PdfExport>,
    pending_pdf: Option<PathBuf>,
    pending_pptx: Option<PathBuf>,
    show: Option<SlideShow>,
    pending_show: Option<bool>,
    pub pending_switch: Option<SwitchTo>,
}

impl ImpressApp {
    pub fn new() -> Self {
        Self {
            recovery: None,
            recovery_open: false,
            recovery_selected: 0,
            recovery_delete: None,
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
            textures: office_render::SlideTextures::default(),
            pdf_export: None,
            pending_pdf: None,
            pending_pptx: None,
            show: None,
            pending_show: None,
            pending_switch: None,
        }
    }

    pub(super) fn has_pending_recovery(&self) -> bool {
        self.recovery_open
    }

    pub(super) fn enable_recovery(&mut self, root: Result<PathBuf, String>) {
        match root.and_then(|root| crate::recovery::Recovery::new(&root)) {
            Ok(store) => {
                self.recovery_open = !store.entries.is_empty();
                self.recovery = Some(store);
            }
            Err(e) => self.set_error(format!("Impress recovery unavailable: {e}")),
        }
    }
    pub(super) fn recovery_tick(&mut self, ctx: &egui::Context) {
        let label = self
            .file_path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled Impress document".into());
        let dirty = self.is_dirty();
        if let Some(store) = &mut self.recovery {
            ctx.request_repaint_after(crate::recovery::INTERVAL);
            if dirty && store.due(std::time::Instant::now()) {
                let snapshot =
                    crate::impress_recovery::ImpressSnapshot::capture(&self.presentation);
                if let Err(e) = store.tick(&snapshot, true, &label, std::time::Instant::now()) {
                    self.set_error(format!("Impress recovery copy failed: {e}"));
                }
            } else if !dirty {
                self.clear_recovery();
            }
        }
    }
    pub(super) fn clear_recovery(&mut self) {
        if let Some(store) = &mut self.recovery {
            if let Err(e) = store.clear() {
                self.set_error(format!("Impress recovery cleanup failed: {e}"));
            }
        }
    }
    fn open_recovery(&mut self) {
        let Some(store) = &mut self.recovery else {
            self.set_error("Impress recovery is unavailable");
            return;
        };
        match store.refresh() {
            Ok(()) => {
                self.recovery_selected = 0;
                self.recovery_open = true;
            }
            Err(e) => self.set_error(format!("Could not list recovery copies: {e}")),
        }
    }
    fn restore_recovery(&mut self, index: usize, ctx: &egui::Context) -> bool {
        if self.is_dirty() {
            self.set_error("Save the current document before recovering another copy");
            return false;
        }
        let Some(store) = &mut self.recovery else {
            return false;
        };
        match store.restore(index) {
            Ok(doc) => {
                let warning = store.cleanup_warning.take();
                self.end_show(ctx);
                self.presentation = doc.into_presentation();
                self.file_path = None;
                self.textures.clear();
                self.edit_session = None;
                self.drag_session = None;
                self.selected_object = 0;
                self.selection_slide = self.presentation.active;
                self.pending_action = None;
                self.pending_delete = None;
                self.pending_show = None;
                self.pending_pdf = None;
                self.pending_pptx = None;
                self.recovery_open = false;
                self.recovery_delete = None;
                self.recovery.as_mut().unwrap().postpone();
                self.set_status("Recovered unsaved Impress document — use Save As to keep it");
                if let Some(e) = warning {
                    self.set_error(format!(
                        "Recovered document; older copy cleanup failed: {e}"
                    ));
                }
                true
            }
            Err(e) => {
                self.set_error(format!("Recovery failed: {e}"));
                false
            }
        }
    }
    fn show_recovery_dialog(&mut self, ctx: &egui::Context) {
        if !self.recovery_open {
            return;
        }
        let mut restore = None;
        let mut delete = None;
        let mut later = false;
        let response = egui::Modal::new(egui::Id::new("impress_recovery")).show(ctx, |ui| {
            ui.set_width(460.0);
            if let Some(index) = self.recovery_delete {
                ui.heading("Delete recovery copy?");
                ui.label("Permanently delete this recovery copy?");
                ui.horizontal(|ui| {
                    if ui.button("Delete").clicked() {
                        delete = Some(index);
                    }
                    if ui.button("Cancel").clicked() {
                        self.recovery_delete = None;
                    }
                });
            } else {
                ui.heading("Unsaved Impress documents found");
                ui.label("Select a copy to recover. Recovered documents use Save As.");
                let entries = self.recovery.as_ref().map(|s| &s.entries);
                let count = entries.map_or(0, Vec::len);
                egui::ScrollArea::vertical()
                    .max_height(200.0)
                    .show(ui, |ui| {
                        if let Some(entries) = entries {
                            for (index, entry) in entries.iter().enumerate() {
                                ui.selectable_value(
                                    &mut self.recovery_selected,
                                    index,
                                    &entry.label,
                                );
                            }
                        }
                    });
                if count == 0 {
                    ui.label("No available recovery copies.");
                }
                if self.is_dirty() {
                    ui.label("Save your current document before recovering another copy.");
                }
                if let Some(error) = &self.last_error {
                    ui.colored_label(Color32::RED, error);
                }
                let can_restore = count > 0 && !self.is_dirty();
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(can_restore, egui::Button::new("Recover (Enter)"))
                        .clicked()
                    {
                        restore = Some(self.recovery_selected);
                    }
                    if ui.button("Keep for later").clicked() {
                        later = true;
                    }
                    if ui
                        .add_enabled(count > 0, egui::Button::new("Delete copy…"))
                        .clicked()
                    {
                        self.recovery_delete = Some(self.recovery_selected);
                    }
                });
                if can_restore
                    && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter))
                {
                    restore = Some(self.recovery_selected);
                }
            }
        });
        if response.should_close() {
            if self.recovery_delete.is_some() {
                self.recovery_delete = None;
            } else {
                later = true;
            }
        }
        if let Some(index) = delete {
            let result = self.recovery.as_mut().unwrap().delete(index);
            match result {
                Ok(()) => {
                    self.recovery_selected = 0;
                    self.recovery_delete = None;
                }
                Err(e) => self.set_error(format!("Recovery copy deletion failed: {e}")),
            }
        }
        if let Some(index) = restore {
            self.restore_recovery(index, ctx);
        }
        if later {
            self.recovery_open = false;
            self.recovery_delete = None;
            if let Some(store) = &mut self.recovery {
                store.postpone();
            }
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
        self.clear_recovery();
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
                self.clear_recovery();
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
                self.clear_recovery();
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

    fn export_pdf(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("PDF (slides as images)", &["pdf"])
            .set_file_name("slides.pdf")
            .save_file()
        else {
            return;
        };
        self.pending_pdf = Some(path);
    }

    fn export_pdf_to(&mut self, ctx: &egui::Context, path: &std::path::Path) -> bool {
        if self.pdf_export.is_some() {
            return false;
        }
        if !path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
        {
            self.set_error("PDF export failed: choose a .pdf destination");
            return false;
        }
        let fonts = ctx.fonts(|fonts| fonts.definitions().clone());
        let presentation = self.presentation.clone();
        let path = path.to_path_buf();
        let destination = path.clone();
        let repaint = ctx.clone();
        let (sender, result) = mpsc::channel();
        let spawn = std::thread::Builder::new()
            .name("impress-pdf".into())
            .spawn(move || {
                let context = egui::Context::default();
                context.set_fonts(fonts);
                let mut snapshot = Some(presentation);
                let mut exported = None;
                let mut output = context.run_ui(egui::RawInput::default(), |ui| {
                    if let Some(presentation) = snapshot.take() {
                        exported = Some(office_render::write_presentation_pdf_path(
                            ui.ctx(),
                            &presentation,
                            &destination,
                        ));
                    }
                });
                output.textures_delta.clear();
                let _ = sender
                    .send(exported.unwrap_or_else(|| Err("PDF worker did not render".into())));
                repaint.request_repaint();
            });
        match spawn {
            Ok(_) => {
                self.pdf_export = Some(PdfExport { path, result });
                self.set_status("Exporting PDF — slides as images; editing remains available");
                true
            }
            Err(error) => {
                self.set_error(format!("PDF export failed: {error}"));
                false
            }
        }
    }

    fn poll_pdf_export(&mut self) {
        let Some(job) = &self.pdf_export else {
            return;
        };
        let result = match job.result.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err("PDF export worker stopped".into()),
        };
        let path = self.pdf_export.take().unwrap().path;
        match result {
            Ok(()) => self.set_status(format!(
                "Exported PDF {} — 144 dpi, slides as images",
                path.display()
            )),
            Err(error) => self.set_error(format!("PDF export failed: {error}")),
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
        self.pending_pptx = Some(path);
    }

    fn export_pptx_to(&mut self, path: &std::path::Path) -> bool {
        if !path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("pptx"))
        {
            self.set_error("PPTX export failed: choose a .pptx destination");
            return false;
        }
        match write_pptx_path(&self.presentation, path) {
            Ok(()) => {
                self.set_status(format!("Exported PPTX {}", path.display()));
                true
            }
            Err(e) => {
                self.set_error(format!("PPTX export failed: {e}"));
                false
            }
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
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F11)) {
            Self::toggle_fullscreen(ctx);
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::F5)) {
            self.pending_show = Some(true);
            return;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F5)) {
            self.pending_show = Some(false);
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
                if ui.button("Recovery copies…").clicked() {
                    self.open_recovery();
                    ui.close();
                }
                if ui
                    .add_enabled(
                        self.pdf_export.is_none() && self.pending_pdf.is_none(),
                        egui::Button::new("Export PDF (slides as images)…"),
                    )
                    .clicked()
                {
                    self.export_pdf();
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
            ui.menu_button("Slide Show", |ui| {
                if ui.button("Window fullscreen  F11").clicked() {
                    Self::toggle_fullscreen(ui.ctx());
                    ui.close();
                }
                ui.separator();
                if ui.button("From beginning  F5").clicked() {
                    self.pending_show = Some(false);
                    ui.close();
                }
                if ui.button("From current slide  Shift+F5").clicked() {
                    self.pending_show = Some(true);
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
        let avail = ui.available_size();
        let aspect = SLIDE_WIDTH_PT / SLIDE_HEIGHT_PT;
        let w = (avail.x - 16.0)
            .max(120.0)
            .min((avail.y - 16.0).max(80.0) * aspect);
        let h = w / aspect;
        let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
        let painter = ui.painter().with_clip_rect(rect);
        self.selected_object = self.selected_object.min(slide.objects.len() + 1);
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
        self.paint_slide(ui, rect, &slide);
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
    /// Editor and slide show deliberately share the same object rendering.
    fn paint_slide(&mut self, ui: &Ui, rect: egui::Rect, slide: &office_impress::Slide) {
        if let Err(error) = self
            .textures
            .paint(ui, rect, slide, &self.presentation.theme)
        {
            self.set_error(format!("Slide rendering failed: {error}"));
        }
    }

    fn toggle_fullscreen(ctx: &egui::Context) {
        let fullscreen = ctx.input(|i| i.viewport().fullscreen).unwrap_or(false);
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fullscreen));
        ctx.request_repaint();
    }

    fn start_show(&mut self, ctx: &egui::Context, from_current: bool) {
        if self.pending_action.is_some() || self.pending_delete.is_some() || self.show.is_some() {
            return;
        }
        let index = if from_current {
            self.presentation.active
        } else {
            0
        };
        let fullscreen = ctx.input(|i| i.viewport().fullscreen).unwrap_or(false);
        let Some(show) = SlideShow::new(index, self.presentation.slides.len(), fullscreen) else {
            return;
        };
        self.show = Some(show);
        self.edit_session = None;
        self.drag_session = None;
        ctx.memory_mut(|m| {
            if let Some(id) = m.focused() {
                m.surrender_focus(id);
            }
        });
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
        ctx.request_repaint();
    }

    pub(super) fn end_show(&mut self, ctx: &egui::Context) {
        self.pending_show = None;
        if let Some(show) = self.show.take() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(show.was_fullscreen));
            ctx.request_repaint();
        }
    }

    fn navigate_show(&mut self, ctx: &egui::Context, navigation: Navigation) {
        if let Some(show) = &mut self.show {
            if !show.navigate(navigation, self.presentation.slides.len()) {
                self.end_show(ctx);
            }
            ctx.request_repaint();
        }
    }

    fn slide_show_ui(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let enabled = ui.is_enabled() && !ctx.memory(|m| m.top_modal_layer().is_some());
        if enabled {
            if let Some(navigation) = slideshow::keyboard_navigation(&ctx) {
                self.navigate_show(&ctx, navigation);
            }
        }
        let Some(show) = self.show else {
            return;
        };
        let count = self.presentation.slides.len();
        let mut navigation = None;
        egui::Panel::bottom("impress_show_controls")
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_gray(25))
                    .inner_margin(8.0),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(show.index > 0, egui::Button::new("Previous"))
                        .clicked()
                    {
                        navigation = Some(Navigation::Previous);
                    }
                    ui.label(
                        RichText::new(format!("{} / {count}", show.index + 1))
                            .color(Color32::WHITE),
                    );
                    if ui
                        .add_enabled(show.index + 1 < count, egui::Button::new("Next"))
                        .clicked()
                    {
                        navigation = Some(Navigation::Next);
                    }
                    ui.label(
                        RichText::new("Arrows / Space · Home / End").color(Color32::LIGHT_GRAY),
                    );
                    if ui.button("Exit (Esc)").clicked() {
                        navigation = Some(Navigation::Exit);
                    }
                });
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(Color32::BLACK))
            .show(ui, |ui| {
                let area = ui.max_rect();
                let rect = fitted_slide_rect(area);
                if let Some(slide) = self.presentation.slides.get(show.index).cloned() {
                    self.paint_slide(ui, rect, &slide);
                }
                let response =
                    ui.interact(area, egui::Id::new("impress_show_canvas"), Sense::click());
                if enabled && response.clicked() {
                    navigation = Some(Navigation::Next);
                } else if enabled && response.secondary_clicked() {
                    navigation = Some(Navigation::Previous);
                }
            });
        if enabled {
            if let Some(navigation) = navigation {
                self.navigate_show(&ctx, navigation);
            }
        }
    }
}

impl Default for ImpressApp {
    fn default() -> Self {
        Self::new()
    }
}

impl App for ImpressApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.document_ui(ui);
    }
}

impl ImpressApp {
    fn document_ui(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        self.poll_pdf_export();
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(self.title()));
        if self.show.is_some() {
            self.slide_show_ui(ui);
            return;
        }
        let recovery_was_open = self.recovery_open;
        self.show_recovery_dialog(&ctx);
        if ui.is_enabled() && !recovery_was_open {
            self.handle_keys(&ctx);
        }
        self.show_unsaved_dialog(&ctx);
        self.show_delete_dialog(&ctx);
        if self.pending_action.is_some() || self.pending_delete.is_some() || recovery_was_open {
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
        if let Some(path) = self.pending_pptx.take() {
            if ui.is_enabled() {
                self.export_pptx_to(&path);
            }
        }
        if let Some(path) = self.pending_pdf.take() {
            if ui.is_enabled() {
                self.export_pdf_to(&ctx, &path);
            }
        }
        // Apply text events in this frame before entering the read-only presentation.
        if let Some(from_current) = self.pending_show.take() {
            if ui.is_enabled() {
                self.start_show(&ctx, from_current);
            }
        }
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

fn fitted_slide_rect(area: egui::Rect) -> egui::Rect {
    let aspect = SLIDE_WIDTH_PT / SLIDE_HEIGHT_PT;
    let width = area.width().min(area.height() * aspect).max(0.0);
    egui::Rect::from_center_size(area.center(), egui::vec2(width, width / aspect))
}

#[cfg(test)]
mod show_tests {
    use super::*;

    fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    fn frame(
        ctx: &egui::Context,
        app: &mut ImpressApp,
        events: Vec<egui::Event>,
        fullscreen: bool,
    ) -> Vec<egui::ViewportCommand> {
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1100.0, 800.0),
            )),
            events,
            ..Default::default()
        };
        input
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .fullscreen = Some(fullscreen);
        let mut output = ctx.run_ui(input, |ui| app.document_ui(ui));
        output.textures_delta.clear();
        output.viewport_output[&egui::ViewportId::ROOT]
            .commands
            .clone()
    }

    #[test]
    fn start_current_navigate_exit_preserves_document_selection_destination_and_history() {
        use egui::{Key, Modifiers};
        let ctx = egui::Context::default();
        let mut app = ImpressApp::new();
        app.presentation.set_active(1);
        app.insert_object(SlideObject::shape(ShapeKind::Rectangle));
        app.selection_slide = 1;
        let dir = tempfile::tempdir().unwrap();
        app.file_path = Some(dir.path().join("original.json"));
        let original = app.presentation.clone();
        let path = app.file_path.clone();
        let selected = app.selected_object;
        let commands = frame(&ctx, &mut app, vec![key(Key::F5, Modifiers::SHIFT)], false);
        assert!(commands
            .iter()
            .any(|cmd| matches!(cmd, egui::ViewportCommand::Fullscreen(true))));
        assert_eq!(app.show.unwrap().index, 1);
        for (key_value, index) in [
            (Key::Home, 0),
            (Key::Space, 1),
            (Key::Enter, 2),
            (Key::PageDown, 2),
            (Key::PageUp, 1),
            (Key::End, 2),
            (Key::ArrowUp, 1),
            (Key::ArrowRight, 2),
        ] {
            frame(&ctx, &mut app, vec![key(key_value, Modifiers::NONE)], true);
            assert_eq!(app.show.unwrap().index, index);
        }
        frame(
            &ctx,
            &mut app,
            vec![
                egui::Event::Text("ignored".into()),
                key(Key::Z, Modifiers::COMMAND),
                key(Key::N, Modifiers::COMMAND),
            ],
            true,
        );
        assert_eq!(app.presentation, original);
        let commands = frame(
            &ctx,
            &mut app,
            vec![key(Key::Escape, Modifiers::NONE)],
            true,
        );
        assert!(commands
            .iter()
            .any(|cmd| matches!(cmd, egui::ViewportCommand::Fullscreen(false))));
        assert!(app.show.is_none());
        frame(&ctx, &mut app, vec![], false);
        assert_eq!(app.presentation, original);
        assert_eq!(app.file_path, path);
        assert_eq!(app.selected_object, selected);
        app.history(false);
        assert!(!app.is_dirty());
    }

    #[test]
    fn final_text_input_is_committed_before_f5_and_show_typing_is_ignored() {
        use egui::{Key, Modifiers};
        let ctx = egui::Context::default();
        let mut app = ImpressApp::new();
        frame(&ctx, &mut app, vec![], false);
        ctx.memory_mut(|m| m.request_focus(egui::Id::new(("impress_text", 0usize, 0usize))));
        frame(
            &ctx,
            &mut app,
            vec![
                egui::Event::Text("追加".into()),
                key(Key::F5, Modifiers::NONE),
            ],
            false,
        );
        assert!(app.show.is_some());
        assert!(app.presentation.slides[0].title.text.contains("追加"));
        let edited = app.presentation.clone();
        frame(
            &ctx,
            &mut app,
            vec![
                egui::Event::Text("unwanted".into()),
                key(Key::Space, Modifiers::NONE),
            ],
            true,
        );
        assert_eq!(app.presentation, edited);
        frame(
            &ctx,
            &mut app,
            vec![key(Key::Escape, Modifiers::NONE)],
            true,
        );
        app.history(false);
        assert!(!app.presentation.slides[0].title.text.contains("追加"));
        assert!(!app.is_dirty());
    }

    #[test]
    fn restore_already_fullscreen_and_block_start_during_confirmation_or_empty_deck() {
        use egui::{Key, Modifiers};
        let ctx = egui::Context::default();
        let mut app = ImpressApp::new();
        app.presentation.set_active(2);
        frame(&ctx, &mut app, vec![key(Key::F5, Modifiers::NONE)], true);
        assert_eq!(app.show.unwrap().index, 0);
        let commands = frame(
            &ctx,
            &mut app,
            vec![key(Key::Escape, Modifiers::NONE)],
            true,
        );
        assert!(commands
            .iter()
            .any(|cmd| matches!(cmd, egui::ViewportCommand::Fullscreen(true))));
        assert_eq!(app.presentation.active, 2);
        app.pending_delete = Some(2);
        frame(&ctx, &mut app, vec![key(Key::F5, Modifiers::NONE)], true);
        assert!(app.show.is_none());
        app.pending_delete = None;
        app.pending_action = Some(DocumentAction::New);
        app.start_show(&ctx, false);
        assert!(app.show.is_none());
        app.pending_action = None;
        app.presentation.slides.clear();
        app.start_show(&ctx, false);
        assert!(app.show.is_none());
    }

    #[test]
    fn fitted_slide_has_centered_letterboxing_in_wide_and_tall_windows() {
        for size in [egui::vec2(1800.0, 700.0), egui::vec2(700.0, 1800.0)] {
            let area = egui::Rect::from_min_size(egui::pos2(20.0, 40.0), size);
            let slide = fitted_slide_rect(area);
            assert_eq!(slide.center(), area.center());
            assert!((slide.width() / slide.height() - 16.0 / 9.0).abs() < 0.0001);
            assert!(area.contains_rect(slide));
        }
    }
}

#[cfg(test)]
mod pdf_tests {
    use super::*;

    fn begin(ctx: &egui::Context, app: &mut ImpressApp, path: &std::path::Path) -> bool {
        let mut started = false;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            started = app.export_pdf_to(ui.ctx(), path);
        });
        output.textures_delta.clear();
        started
    }

    fn finish(app: &mut ImpressApp) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while app.pdf_export.is_some() {
            app.poll_pdf_export();
            assert!(
                std::time::Instant::now() < deadline,
                "PDF export did not finish"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn asynchronous_pdf_uses_snapshot_and_keeps_later_edits_selection_path_and_history() {
        let ctx = egui::Context::default();
        let mut app = ImpressApp::new();
        app.presentation.set_active(2);
        let dir = tempfile::tempdir().unwrap();
        let native = dir.path().join("native.json");
        app.file_path = Some(native.clone());
        let pdf = dir.path().join("slides.pdf");
        assert!(begin(&ctx, &mut app, &pdf));
        app.presentation.add_slide();
        let edited = app.presentation.clone();
        assert!(!begin(&ctx, &mut app, &dir.path().join("second.pdf")));
        finish(&mut app);
        assert!(app.last_error.is_none());
        assert_eq!(app.presentation, edited);
        assert_eq!(app.file_path, Some(native));
        assert_eq!(app.presentation.active, 3);
        assert!(app.is_dirty());
        let bytes = std::fs::read(pdf).unwrap();
        assert!(bytes.starts_with(b"%PDF"));
        assert!(String::from_utf8_lossy(&bytes).contains("/Count 3"));
        app.history(false);
        assert!(!app.is_dirty());
        assert_eq!(app.presentation.active, 2);
    }

    #[test]
    fn rejected_destination_and_failed_render_keep_old_file_and_unsaved_history() {
        let ctx = egui::Context::default();
        let mut app = ImpressApp::new();
        let mut slide = app.presentation.active_slide().unwrap().clone();
        slide.objects.push(SlideObject {
            bounds: Bounds::default(),
            kind: ObjectKind::Image {
                data: std::sync::Arc::new(b"corrupt".to_vec()),
            },
        });
        app.presentation.update_active_slide(slide, false);
        let original = app.presentation.clone();
        let dir = tempfile::tempdir().unwrap();
        let native = dir.path().join("native.json");
        std::fs::write(&native, b"original native").unwrap();
        app.file_path = Some(native.clone());
        assert!(!begin(&ctx, &mut app, &native));
        assert_eq!(std::fs::read(&native).unwrap(), b"original native");
        let pdf = dir.path().join("existing.pdf");
        std::fs::write(&pdf, b"original pdf").unwrap();
        assert!(begin(&ctx, &mut app, &pdf));
        finish(&mut app);
        assert!(app.last_error.is_some());
        assert_eq!(std::fs::read(pdf).unwrap(), b"original pdf");
        assert_eq!(app.presentation, original);
        assert_eq!(app.file_path, Some(native));
        app.history(false);
        assert!(!app.is_dirty());
    }
}

#[cfg(test)]
mod pptx_object_tests {
    use super::*;
    #[test]
    fn oversized_pptx_open_retains_unsaved_document_destination_and_history() {
        let dir = tempfile::tempdir().unwrap();
        let native = dir.path().join("original.rimpress.json");
        let oversized = dir.path().join("oversized.pptx");
        std::fs::File::create(&oversized)
            .unwrap()
            .set_len(64 * 1024 * 1024 + 1)
            .unwrap();
        let mut app = ImpressApp::new();
        app.file_path = Some(native.clone());
        assert!(app.save_file());
        app.presentation.duplicate_active_slide();
        let before = app.presentation.clone();
        app.open_path(&oversized);
        assert_eq!(app.presentation, before);
        assert_eq!(app.file_path, Some(native));
        assert!(app
            .last_error
            .as_ref()
            .unwrap()
            .contains("64 MiB file limit"));
        assert!(app.presentation.undo());
        assert!(!app.is_dirty());
        assert!(app.presentation.redo());
        assert_eq!(app.presentation.slides, before.slides);
        assert!(app.save_file());
    }

    #[test]
    fn queued_export_contains_text_entered_in_the_export_frame() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("final-text.pptx");
        let ctx = egui::Context::default();
        let mut app = ImpressApp::new();
        let input = |events| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1100.0, 800.0),
            )),
            events,
            ..Default::default()
        };
        ctx.run_ui(input(vec![]), |ui| app.document_ui(ui))
            .textures_delta
            .clear();
        ctx.memory_mut(|m| m.request_focus(egui::Id::new(("impress_text", 0usize, 0usize))));
        app.pending_pptx = Some(path.clone());
        ctx.run_ui(input(vec![egui::Event::Text("追加".into())]), |ui| {
            app.document_ui(ui)
        })
        .textures_delta
        .clear();
        assert!(app.presentation.slides[0].title.text.contains("追加"));
        assert_eq!(
            load_pptx_path(&path).unwrap().slides[0].title.text,
            app.presentation.slides[0].title.text
        );
        assert!(app.presentation.undo());
        assert!(!app.is_dirty());
    }

    #[test]
    fn object_export_keeps_native_destination_unsaved_edits_selection_and_history() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = ImpressApp::new();
        let native = dir.path().join("original.rimpress.json");
        app.file_path = Some(native.clone());
        assert!(app.save_file());
        let mut slide = app.presentation.active_slide().unwrap().clone();
        slide.objects.push(SlideObject::shape(ShapeKind::Ellipse));
        app.presentation.update_active_slide(slide, false);
        app.selected_object = 2;
        let before = app.presentation.clone();
        let path = dir.path().join("objects.pptx");
        assert!(app.export_pptx_to(&path));
        assert_eq!(load_pptx_path(&path).unwrap().slides, before.slides);
        assert_eq!(app.presentation, before);
        assert_eq!(app.file_path, Some(native.clone()));
        assert_eq!(app.selected_object, 2);
        assert!(app.presentation.undo());
        assert!(!app.is_dirty());
        assert!(app.presentation.redo());
        assert!(app.is_dirty());
        let bytes = std::fs::read(&native).unwrap();
        assert!(!app.export_pptx_to(&native));
        assert_eq!(std::fs::read(&native).unwrap(), bytes);
        assert!(app.is_dirty());
        assert_eq!(app.presentation.slides, before.slides);
        // A failed write must leave the redo branch available.
        app.presentation.undo();
        let parent = dir.path().join("blocked");
        std::fs::write(&parent, b"keep parent").unwrap();
        assert!(!app.export_pptx_to(&parent.join("failed.pptx")));
        assert!(!app.is_dirty());
        assert_eq!(std::fs::read(parent).unwrap(), b"keep parent");
        assert!(app.presentation.redo());
        assert_eq!(app.presentation.slides, before.slides);
        assert_eq!(app.file_path, Some(native));
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    use crate::{impress_recovery::ImpressSnapshot, recovery::Recovery};
    fn crash_copy(root: &std::path::Path) {
        let mut p = Presentation::demo();
        p.slides[0].notes = "日本語 notes".into();
        p.slides[0]
            .objects
            .push(SlideObject::shape(ShapeKind::Rectangle));
        let mut store = Recovery::new(root).unwrap();
        store
            .tick(
                &ImpressSnapshot::capture(&p),
                true,
                "original.pptx",
                std::time::Instant::now(),
            )
            .unwrap();
    }
    #[test]
    fn recovered_deck_uses_new_destination_and_failed_save_keeps_copy() {
        let dir = tempfile::tempdir().unwrap();
        crash_copy(dir.path());
        let original = dir.path().join("original.pptx");
        std::fs::write(&original, b"original bytes").unwrap();
        let mut app = ImpressApp::new();
        app.file_path = Some(original.clone());
        app.enable_recovery(Ok(dir.path().into()));
        assert!(app.restore_recovery(0, &egui::Context::default()));
        assert!(app.is_dirty() && app.file_path.is_none() && !app.presentation.can_undo());
        std::fs::write(dir.path().join("blocked"), b"file").unwrap();
        assert!(!app.save_json_to(&dir.path().join("blocked/deck.json")));
        assert!(app.recovery.as_ref().unwrap().has_snapshot);
        assert_eq!(std::fs::read(&original).unwrap(), b"original bytes");
        let target = dir.path().join("recovered.rimpress.json");
        assert!(app.save_json_to(&target));
        assert!(!app.is_dirty() && !app.recovery.as_ref().unwrap().has_snapshot);
        assert_eq!(
            load_json_path(&target).unwrap().slides[0].notes,
            "日本語 notes"
        );
    }
    #[test]
    fn dirty_current_deck_blocks_recovery_and_postponed_copy_survives_new() {
        let dir = tempfile::tempdir().unwrap();
        crash_copy(dir.path());
        let mut app = ImpressApp::new();
        app.enable_recovery(Ok(dir.path().into()));
        app.presentation.add_slide();
        let count = app.presentation.slides.len();
        assert!(!app.restore_recovery(0, &egui::Context::default()));
        assert_eq!(app.presentation.slides.len(), count);
        app.recovery.as_mut().unwrap().postpone();
        app.do_new_deck();
        drop(app);
        assert_eq!(
            Recovery::<ImpressSnapshot>::new(dir.path())
                .unwrap()
                .entries
                .len(),
            1
        );
    }
    #[test]
    fn background_copy_does_not_change_selection_or_undo_and_unavailable_storage_allows_save() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = ImpressApp::new();
        app.enable_recovery(Ok(dir.path().into()));
        app.presentation.add_slide();
        app.selected_object = 1;
        let before = serde_json::to_value(&app.presentation).unwrap();
        app.recovery_tick(&egui::Context::default());
        assert_eq!(serde_json::to_value(&app.presentation).unwrap(), before);
        assert_eq!(app.selected_object, 1);
        assert!(app.presentation.can_undo());
        let mut unavailable = ImpressApp::new();
        unavailable.enable_recovery(Err("no state directory".into()));
        assert!(unavailable.recovery.is_none());
        assert!(unavailable.save_json_to(&dir.path().join("normal.json")));
    }
}
