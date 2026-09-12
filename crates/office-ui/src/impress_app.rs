//! Impress (presentation) application UI.

use std::path::PathBuf;

use eframe::App;
use egui::{self, Color32, RichText, Sense, Ui, Vec2};
use office_impress::{
    load_json_path, load_pptx_path, write_json_path, write_pptx_path, Presentation, Theme,
    SLIDE_HEIGHT_PT, SLIDE_WIDTH_PT,
};

use crate::theme::{CANVAS_BG, STATUS_BG, TOOLBAR_BG};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchTo {
    Writer,
    Calc,
}

pub struct ImpressApp {
    presentation: Presentation,
    file_path: Option<PathBuf>,
    status_message: String,
    last_error: Option<String>,
    pub pending_switch: Option<SwitchTo>,
}

impl ImpressApp {
    pub fn new() -> Self {
        Self {
            presentation: Presentation::demo(),
            file_path: None,
            status_message: "Ready — Impress MVP (themes, slides, JSON, PPTX open/export)".into(),
            last_error: None,
            pending_switch: None,
        }
    }

    pub fn take_switch(&mut self) -> Option<SwitchTo> {
        self.pending_switch.take()
    }

    fn title(&self) -> String {
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
        self.presentation = Presentation::new();
        self.file_path = None;
        self.set_status("New presentation");
    }

    fn open_file(&mut self) {
        let path = rfd::FileDialog::new()
            .add_filter("PowerPoint", &["pptx"])
            .add_filter("rust-office Impress", &["rimpress.json", "json"])
            .pick_file();
        let Some(path) = path else {
            return;
        };
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let result = if ext == "pptx" {
            load_pptx_path(&path).map_err(|e| e.to_string())
        } else {
            load_json_path(&path).map_err(|e| e.to_string())
        };
        match result {
            Ok(p) => {
                self.presentation = p;
                self.file_path = Some(path.clone());
                self.set_status(format!("Opened {}", path.display()));
            }
            Err(e) => self.set_error(format!("Open failed: {e}")),
        }
    }

    fn save_json(&mut self) {
        if let Some(path) = self.file_path.clone() {
            self.save_json_to(&path);
        } else {
            self.save_json_as();
        }
    }

    fn save_json_as(&mut self) {
        let path = rfd::FileDialog::new()
            .add_filter("rust-office Impress", &["rimpress.json", "json"])
            .set_file_name("deck.rimpress.json")
            .save_file();
        let Some(path) = path else {
            return;
        };
        self.save_json_to(&path);
        self.file_path = Some(path);
    }

    fn save_json_to(&mut self, path: &std::path::Path) {
        match write_json_path(&self.presentation, path) {
            Ok(()) => {
                self.presentation.mark_clean();
                self.set_status(format!("Saved {}", path.display()));
            }
            Err(e) => self.set_error(format!("Save failed: {e}")),
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
                    self.save_json();
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
                    if ui
                        .selectable_label(selected, theme.name.clone())
                        .clicked()
                    {
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
        let bg = Color32::from_rgb(theme.background[0], theme.background[1], theme.background[2]);
        ui.painter().rect_filled(rect, 4.0, bg);
        ui.painter().rect_stroke(
            rect,
            4.0,
            egui::Stroke::new(1.0, Color32::from_gray(160)),
            egui::StrokeKind::Outside,
        );

        let title_color =
            Color32::from_rgb(theme.title_color[0], theme.title_color[1], theme.title_color[2]);
        let body_color =
            Color32::from_rgb(theme.body_color[0], theme.body_color[1], theme.body_color[2]);
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
        ui.painter()
            .rect_filled(egui::Rect::from_min_size(rect.min, Vec2::new(6.0, h)), 0.0, accent);
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
