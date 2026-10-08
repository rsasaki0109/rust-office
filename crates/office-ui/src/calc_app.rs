//! Calc (spreadsheet) application UI.

use std::path::PathBuf;

use eframe::App;
use egui::{self, Color32, Context, Key, RichText, Sense, Ui, Vec2};
use office_calc::{
    col_to_letters, load_csv_path, load_xlsx_path, write_csv_path, write_xlsx_path, CellAddr,
    Workbook,
};

use crate::theme::{ACCENT, CANVAS_BG, STATUS_BG, TOOLBAR_BG};
use crate::unsaved::{self, Choice, DocumentAction};

const COL_WIDTH: f32 = 88.0;
const ROW_HEIGHT: f32 = 24.0;
const HEADER_W: f32 = 40.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchTo {
    Writer,
    Impress,
    Quit,
}

pub struct CalcApp {
    workbook: Workbook,
    file_path: Option<PathBuf>,
    active: CellAddr,
    /// Draft text in the formula bar / in-cell editor.
    edit_buf: String,
    editing: bool,
    status_message: String,
    last_error: Option<String>,
    pending_action: Option<DocumentAction>,
    pub pending_switch: Option<SwitchTo>,
}

impl CalcApp {
    pub fn new() -> Self {
        let mut workbook = Workbook::new();
        // Demo sheet so first launch is useful.
        workbook.set_cell(CellAddr::new(0, 0), "Item");
        workbook.set_cell(CellAddr::new(1, 0), "Qty");
        workbook.set_cell(CellAddr::new(2, 0), "Price");
        workbook.set_cell(CellAddr::new(0, 1), "Apples");
        workbook.set_cell(CellAddr::new(1, 1), "3");
        workbook.set_cell(CellAddr::new(2, 1), "1.2");
        workbook.set_cell(CellAddr::new(0, 2), "Oranges");
        workbook.set_cell(CellAddr::new(1, 2), "2");
        workbook.set_cell(CellAddr::new(2, 2), "0.8");
        workbook.set_cell(CellAddr::new(0, 3), "Total");
        workbook.set_cell(CellAddr::new(1, 3), "=SUM(B2:B3)");
        workbook.set_cell(CellAddr::new(2, 3), "=B2*C2+B3*C3");
        workbook.mark_clean();
        let edit_buf = workbook.active_sheet().raw(CellAddr::new(0, 0)).to_owned();

        Self {
            workbook,
            file_path: None,
            active: CellAddr::new(0, 0),
            edit_buf,
            editing: false,
            status_message: "Ready — Calc MVP (CSV/XLSX, SUM/AVERAGE/MIN/MAX/IF/COUNT, +−*/)"
                .into(),
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
            .unwrap_or_else(|| "Untitled".into());
        let dirty = if self.is_dirty() { " *" } else { "" };
        format!("rust-office — Calc — {name}{dirty}")
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

    fn begin_edit(&mut self) {
        self.edit_buf = self.workbook.active_sheet().raw(self.active).to_string();
        self.editing = true;
    }

    fn commit_edit(&mut self) {
        if self.editing {
            if self.edit_buf != self.workbook.active_sheet().raw(self.active) {
                self.workbook.set_cell(self.active, self.edit_buf.clone());
            }
            self.editing = false;
            self.set_status(format!("Edited {}", self.active.to_a1()));
        }
    }

    fn cancel_edit(&mut self) {
        self.editing = false;
        self.edit_buf.clear();
    }

    fn select_cell(&mut self, addr: CellAddr) {
        if self.editing {
            self.commit_edit();
        }
        self.active = addr;
        self.edit_buf = self.workbook.active_sheet().raw(addr).to_string();
    }

    fn move_active(&mut self, dcol: i32, drow: i32) {
        if self.editing {
            self.commit_edit();
        }
        let sheet = self.workbook.active_sheet();
        let col = (self.active.col as i32 + dcol).clamp(0, sheet.cols as i32 - 1) as u32;
        let row = (self.active.row as i32 + drow).clamp(0, sheet.rows as i32 - 1) as u32;
        self.select_cell(CellAddr::new(col, row));
    }

    fn new_sheet(&mut self) {
        self.request_action(DocumentAction::New);
    }

    fn do_new_sheet(&mut self) {
        self.workbook = Workbook::new();
        self.file_path = None;
        self.active = CellAddr::new(0, 0);
        self.editing = false;
        self.edit_buf.clear();
        self.set_status("New spreadsheet");
    }

    fn open_file(&mut self) {
        self.request_action(DocumentAction::Open);
    }

    fn do_open_file(&mut self) {
        let path = rfd::FileDialog::new()
            .add_filter("Excel", &["xlsx", "xlsm", "xls"])
            .add_filter("CSV", &["csv"])
            .add_filter("OpenDocument Spreadsheet", &["ods"])
            .pick_file();
        let Some(path) = path else {
            return;
        };
        let result = match path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str()
        {
            "csv" => load_csv_path(&path).map_err(|e| e.to_string()),
            _ => load_xlsx_path(&path).map_err(|e| e.to_string()),
        };
        match result {
            Ok(wb) => {
                self.workbook = wb;
                self.file_path = Some(path.clone());
                self.active = CellAddr::new(0, 0);
                self.editing = false;
                self.edit_buf = self.workbook.active_sheet().raw(self.active).to_string();
                self.set_status(format!("Opened {}", path.display()));
            }
            Err(e) => self.set_error(format!("Open failed: {e}")),
        }
    }

    pub(super) fn is_dirty(&self) -> bool {
        self.workbook.is_dirty()
            || (self.editing && self.edit_buf != self.workbook.active_sheet().raw(self.active))
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
            DocumentAction::New => self.do_new_sheet(),
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

    fn show_unsaved_dialog(&mut self, ctx: &Context) {
        if self.pending_action.is_some() {
            if let Some(choice) = unsaved::confirm(ctx, "calc_unsaved", &self.title()) {
                self.resolve_pending_action(choice);
            }
        }
    }

    pub(super) fn save_file(&mut self) -> bool {
        if let Some(path) = self.file_path.clone() {
            if writable_extension(&path).is_some() {
                return self.save_to(&path);
            }
        }
        // Imported XLS/ODS/XLSM files need an explicit supported destination.
        self.save_file_as()
    }

    fn save_file_as(&mut self) -> bool {
        let path = rfd::FileDialog::new()
            .add_filter("Excel", &["xlsx"])
            .add_filter("CSV", &["csv"])
            .set_file_name("sheet.xlsx")
            .save_file();
        let Some(path) = path else {
            return false;
        };
        self.save_to(&path)
    }

    fn save_to(&mut self, path: &std::path::Path) -> bool {
        let Some(ext) = writable_extension(path) else {
            self.set_error("Save failed: choose a .xlsx or .csv file");
            return false;
        };
        if ext == "csv" && self.workbook.sheet_count() > 1 {
            self.set_error(
                "Save failed: CSV only stores one sheet. Choose XLSX to save all sheets.",
            );
            return false;
        }
        // Include the formula-bar draft, but retain it unchanged if saving fails.
        let mut workbook = self.workbook.clone();
        if self.editing && self.edit_buf != workbook.active_sheet().raw(self.active) {
            workbook.set_cell(self.active, self.edit_buf.clone());
        }
        let result = if ext == "csv" {
            write_csv_path(&workbook, path).map_err(|e| e.to_string())
        } else {
            write_xlsx_path(&workbook, path).map_err(|e| e.to_string())
        };
        match result {
            Ok(()) => {
                self.workbook = workbook;
                self.workbook.mark_clean();
                self.editing = false;
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

    fn menu_bar(&mut self, ui: &mut Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("New Spreadsheet").clicked() {
                    self.new_sheet();
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
                    self.save_file_as();
                    ui.close();
                }
                ui.separator();
                if ui.button("Switch to Writer").clicked() {
                    self.pending_switch = Some(SwitchTo::Writer);
                    ui.close();
                }
                if ui.button("Switch to Impress").clicked() {
                    self.pending_switch = Some(SwitchTo::Impress);
                    ui.close();
                }
                ui.separator();
                if ui.button("Quit").clicked() {
                    self.pending_switch = Some(SwitchTo::Quit);
                    ui.close();
                }
            });
            ui.menu_button("Sheet", |ui| {
                let names: Vec<String> = self
                    .workbook
                    .sheets
                    .iter()
                    .map(|s| s.name.clone())
                    .collect();
                for (i, name) in names.iter().enumerate() {
                    let selected = i == self.workbook.active;
                    if ui.selectable_label(selected, name).clicked() {
                        self.workbook.set_active_sheet(i);
                        self.select_cell(CellAddr::new(0, 0));
                        ui.close();
                    }
                }
                ui.separator();
                if ui.button("Add Sheet").clicked() {
                    let n = self.workbook.sheet_count() + 1;
                    self.workbook.add_sheet(format!("Sheet{n}"));
                    self.select_cell(CellAddr::new(0, 0));
                    ui.close();
                }
            });
            ui.menu_button("Edit", |ui| {
                if ui.button("Clear Active Cell").clicked() {
                    self.workbook.set_cell(self.active, "");
                    self.edit_buf.clear();
                    self.editing = false;
                    ui.close();
                }
            });
        });
    }

    fn formula_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(self.active.to_a1()).strong().color(ACCENT));
            ui.separator();
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.edit_buf)
                    .desired_width(ui.available_width() - 80.0)
                    .hint_text("value or =formula"),
            );
            if response.gained_focus() {
                self.editing = true;
            }
            if response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                self.commit_edit();
            }
            if ui.button("✓").clicked() {
                self.commit_edit();
            }
            if ui.button("✗").clicked() {
                self.cancel_edit();
                self.edit_buf = self.workbook.active_sheet().raw(self.active).to_string();
            }
        });
    }

    fn grid(&mut self, ui: &mut Ui) {
        let cols = self.workbook.active_sheet().cols.min(40);
        let rows = self.workbook.active_sheet().rows.min(80);
        let active = self.active;

        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.allocate_exact_size(Vec2::new(HEADER_W, ROW_HEIGHT), Sense::hover());
                    for c in 0..cols {
                        let label = col_to_letters(c);
                        let (rect, _) = ui
                            .allocate_exact_size(Vec2::new(COL_WIDTH, ROW_HEIGHT), Sense::hover());
                        ui.painter().rect_filled(rect, 0.0, Color32::from_gray(230));
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            label,
                            egui::FontId::proportional(12.0),
                            Color32::DARK_GRAY,
                        );
                    }
                });

                for r in 0..rows {
                    ui.horizontal(|ui| {
                        let (hrect, _) =
                            ui.allocate_exact_size(Vec2::new(HEADER_W, ROW_HEIGHT), Sense::hover());
                        ui.painter()
                            .rect_filled(hrect, 0.0, Color32::from_gray(230));
                        ui.painter().text(
                            hrect.center(),
                            egui::Align2::CENTER_CENTER,
                            format!("{}", r + 1),
                            egui::FontId::proportional(11.0),
                            Color32::DARK_GRAY,
                        );

                        for c in 0..cols {
                            let addr = CellAddr::new(c, r);
                            let display = self.workbook.active_sheet().display(addr);
                            let is_active = addr == active;
                            let (rect, response) = ui.allocate_exact_size(
                                Vec2::new(COL_WIDTH, ROW_HEIGHT),
                                Sense::click(),
                            );
                            let bg = if is_active {
                                Color32::from_rgb(210, 230, 255)
                            } else {
                                Color32::WHITE
                            };
                            ui.painter().rect_filled(rect, 0.0, bg);
                            ui.painter().rect_stroke(
                                rect,
                                0.0,
                                egui::Stroke::new(1.0, Color32::from_gray(200)),
                                egui::StrokeKind::Inside,
                            );
                            ui.painter().text(
                                rect.left_center() + Vec2::new(4.0, 0.0),
                                egui::Align2::LEFT_CENTER,
                                display,
                                egui::FontId::proportional(13.0),
                                Color32::BLACK,
                            );
                            if response.clicked() {
                                self.select_cell(addr);
                            }
                            if response.double_clicked() {
                                self.select_cell(addr);
                                self.begin_edit();
                            }
                        }
                    });
                }
            });
    }

    fn handle_keys(&mut self, ctx: &Context) {
        let escape = ctx.input(|i| i.key_pressed(Key::Escape));
        let left = ctx.input(|i| i.key_pressed(Key::ArrowLeft));
        let right = ctx.input(|i| i.key_pressed(Key::ArrowRight));
        let up = ctx.input(|i| i.key_pressed(Key::ArrowUp));
        let down = ctx.input(|i| i.key_pressed(Key::ArrowDown));
        let enter = ctx.input(|i| i.key_pressed(Key::Enter));
        let f2 = ctx.input(|i| i.key_pressed(Key::F2));
        let delete = ctx.input(|i| i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace));
        let wants_text = ctx.egui_wants_keyboard_input();

        if wants_text && self.editing {
            if escape {
                self.cancel_edit();
                self.edit_buf = self.workbook.active_sheet().raw(self.active).to_string();
            }
            return;
        }
        if left {
            self.move_active(-1, 0);
        } else if right {
            self.move_active(1, 0);
        } else if up {
            self.move_active(0, -1);
        } else if down || enter {
            self.move_active(0, 1);
        } else if f2 {
            self.begin_edit();
        } else if delete {
            self.workbook.set_cell(self.active, "");
            self.edit_buf.clear();
        }
    }
}

impl Default for CalcApp {
    fn default() -> Self {
        Self::new()
    }
}

impl App for CalcApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(self.title()));
        if self.pending_action.is_none() && ui.is_enabled() {
            self.handle_keys(&ctx);
        }
        self.show_unsaved_dialog(&ctx);
        if self.pending_action.is_some() {
            ui.disable();
        }

        egui::Panel::top("calc_menu")
            .frame(egui::Frame::new().fill(TOOLBAR_BG).inner_margin(4.0))
            .show(ui, |ui| {
                self.menu_bar(ui);
            });

        egui::Panel::top("calc_formula")
            .frame(egui::Frame::new().fill(TOOLBAR_BG).inner_margin(6.0))
            .show(ui, |ui| {
                self.formula_bar(ui);
            });

        egui::Panel::bottom("calc_status")
            .frame(egui::Frame::new().fill(STATUS_BG).inner_margin(6.0))
            .show(ui, |ui| {
                let sheet_name = self.workbook.active_sheet().name.clone();
                let cols = self.workbook.active_sheet().cols;
                let rows = self.workbook.active_sheet().rows;
                let color = if self.last_error.is_some() {
                    Color32::from_rgb(180, 40, 40)
                } else {
                    Color32::DARK_GRAY
                };
                ui.horizontal(|ui| {
                    ui.colored_label(color, &self.status_message);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(format!("{sheet_name} · {cols}×{rows}"));
                    });
                });
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(CANVAS_BG).inner_margin(4.0))
            .show(ui, |ui| {
                self.grid(ui);
            });
    }
}

fn writable_extension(path: &std::path::Path) -> Option<String> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    matches!(extension.as_str(), "xlsx" | "csv").then_some(extension)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(app: &mut CalcApp, value: &str) {
        app.begin_edit();
        app.edit_buf = value.into();
    }

    #[test]
    fn saving_includes_uncommitted_cell_and_records_actual_destination() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sheet.csv");
        let mut app = CalcApp::new();
        draft(&mut app, "unfinished cell edit");
        assert!(app.is_dirty());
        assert!(app.save_to(&path));
        let loaded = load_csv_path(&path).unwrap();
        assert_eq!(
            loaded.active_sheet().raw(app.active),
            "unfinished cell edit"
        );
        assert_eq!(app.file_path.as_deref(), Some(path.as_path()));
        assert!(!app.is_dirty());
        assert!(!app.editing);
    }

    #[test]
    fn failed_save_preserves_destination_and_uncommitted_edit() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("original.csv");
        let bad = dir.path().join("destination.csv");
        std::fs::write(&original, "old contents").unwrap();
        std::fs::create_dir(&bad).unwrap();
        let mut app = CalcApp::new();
        app.file_path = Some(original.clone());
        draft(&mut app, "keep draft");
        assert!(!app.save_to(&bad));
        assert_eq!(app.file_path, Some(original.clone()));
        assert_eq!(std::fs::read_to_string(original).unwrap(), "old contents");
        assert_eq!(app.edit_buf, "keep draft");
        assert!(app.editing && app.is_dirty());
        assert_eq!(app.workbook.active_sheet().raw(app.active), "Item");
    }

    #[test]
    fn unsupported_destination_is_not_rewritten_or_changed_to_xlsx() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("original.ods");
        std::fs::write(&path, "original file").unwrap();
        let mut app = CalcApp::new();
        draft(&mut app, "new value");
        assert!(!app.save_to(&path));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "original file");
        assert!(!path.with_extension("xlsx").exists());
        assert!(app.file_path.is_none() && app.is_dirty());
    }

    #[test]
    fn csv_save_cannot_mark_a_multi_sheet_workbook_clean() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("existing.csv");
        std::fs::write(&path, "original file").unwrap();
        let mut app = CalcApp::new();
        app.workbook.add_sheet("Keep this sheet");
        assert!(!app.save_to(&path));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "original file");
        assert_eq!(app.workbook.sheet_count(), 2);
        assert!(app.is_dirty());
        assert!(app.file_path.is_none());
    }

    #[test]
    fn cancel_new_or_open_preserves_formula_bar_draft() {
        for action in [DocumentAction::New, DocumentAction::Open] {
            let mut app = CalcApp::new();
            draft(&mut app, "keep draft");
            app.request_action(action);
            assert_eq!(app.pending_action, Some(action));
            app.resolve_pending_action(Choice::Cancel);
            assert!(app.pending_action.is_none());
            assert_eq!(app.edit_buf, "keep draft");
            assert_eq!(app.workbook.active_sheet().raw(app.active), "Item");
            assert!(app.is_dirty());
        }
    }

    #[test]
    fn save_failure_does_not_execute_pending_new() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("destination.csv");
        std::fs::create_dir(&path).unwrap();
        let mut app = CalcApp::new();
        app.file_path = Some(path);
        draft(&mut app, "keep draft");
        app.new_sheet();
        app.resolve_pending_action(Choice::Save);
        assert!(app.pending_action.is_none());
        assert_eq!(app.edit_buf, "keep draft");
        assert_eq!(app.workbook.active_sheet().raw(app.active), "Item");
        assert!(app.is_dirty());
    }

    #[test]
    fn unchanged_cell_focus_does_not_require_saving() {
        let mut app = CalcApp::new();
        app.begin_edit();
        assert!(!app.is_dirty());
        app.commit_edit();
        assert!(!app.is_dirty());
    }
}
