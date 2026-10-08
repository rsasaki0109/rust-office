//! Impress (presentation) application UI.

use std::path::PathBuf;

use eframe::App;
use egui::{self, Color32, RichText, Sense, Ui, Vec2};
use office_impress::{
    load_json_path, load_pptx_path, write_json_path, write_pptx_path, Presentation, Theme,
    SLIDE_HEIGHT_PT, SLIDE_WIDTH_PT,
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
            ui.menu_button("Insert", |ui| {
                if ui.button("New Slide").clicked() {
                    self.presentation.insert_slide_after_current();
                    self.set_status("Slide inserted");
                    ui.close();
                }
            });
            ui.menu_button("Theme", |ui| {
                for theme in Theme::builtins() {
                    let selected = self.presentation.theme.name == theme.name;
                    if ui.selectable_label(selected, theme.name.clone()).clicked() {
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
                    self.presentation.set_active(i);
                }
            }
            ui.separator();
            if ui.button("+ Slide").clicked() {
                self.presentation.add_slide();
            }
            if ui.button("Delete slide").clicked() {
                self.presentation.delete_active_slide();
                self.set_status("Slide deleted");
            }
        });
    }

    fn canvas(&self, ui: &mut Ui) {
        let theme = &self.presentation.theme;
        let Some(slide) = self.presentation.active_slide() else {
            return;
        };

        let avail = ui.available_size();
        let aspect = SLIDE_WIDTH_PT / SLIDE_HEIGHT_PT;
        let mut w = (avail.x - 16.0).max(120.0);
        let mut h = w / aspect;
        if h > avail.y - 16.0 {
            h = (avail.y - 16.0).max(80.0);
            w = h * aspect;
        }
        let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
        let bg = Color32::from_rgb(
            theme.background[0],
            theme.background[1],
            theme.background[2],
        );
        ui.painter().rect_filled(rect, 4.0, bg);
        ui.painter().rect_stroke(
            rect,
            4.0,
            egui::Stroke::new(1.0, Color32::from_gray(160)),
            egui::StrokeKind::Outside,
        );

        let title_color = Color32::from_rgb(
            theme.title_color[0],
            theme.title_color[1],
            theme.title_color[2],
        );
        let body_color = Color32::from_rgb(
            theme.body_color[0],
            theme.body_color[1],
            theme.body_color[2],
        );
        let scale = h / SLIDE_HEIGHT_PT;

        ui.painter().text(
            rect.min + Vec2::new(slide.title.x * w, slide.title.y * h),
            egui::Align2::LEFT_TOP,
            &slide.title.text,
            egui::FontId::proportional(theme.title_font_pt * scale),
            title_color,
        );
        ui.painter().text(
            rect.min + Vec2::new(slide.body.x * w, slide.body.y * h),
            egui::Align2::LEFT_TOP,
            &slide.body.text,
            egui::FontId::proportional(theme.body_font_pt * scale),
            body_color,
        );

        let accent = Color32::from_rgb(theme.accent[0], theme.accent[1], theme.accent[2]);
        ui.painter().rect_filled(
            egui::Rect::from_min_size(rect.min, Vec2::new(6.0, h)),
            0.0,
            accent,
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
        self.show_unsaved_dialog(&ctx);
        if self.pending_action.is_some() {
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

        let mut edited = false;
        egui::Panel::right("impress_edit")
            .resizable(true)
            .default_size(280.0)
            .frame(egui::Frame::new().fill(TOOLBAR_BG).inner_margin(8.0))
            .show(ui, |ui| {
                ui.label(RichText::new("Edit").strong());
                ui.separator();
                if let Some(slide) = self.presentation.active_slide_mut() {
                    ui.label("Title");
                    if ui
                        .add(
                            egui::TextEdit::multiline(&mut slide.title.text)
                                .desired_rows(2)
                                .desired_width(f32::INFINITY),
                        )
                        .changed()
                    {
                        edited = true;
                    }
                    ui.add_space(8.0);
                    ui.label("Body");
                    if ui
                        .add(
                            egui::TextEdit::multiline(&mut slide.body.text)
                                .desired_rows(10)
                                .desired_width(f32::INFINITY),
                        )
                        .changed()
                    {
                        edited = true;
                    }
                    ui.add_space(8.0);
                    ui.label("Notes");
                    if ui
                        .add(
                            egui::TextEdit::multiline(&mut slide.notes)
                                .desired_rows(3)
                                .desired_width(f32::INFINITY),
                        )
                        .changed()
                    {
                        edited = true;
                    }
                }
            });
        if edited {
            self.presentation.mark_dirty();
        }

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
