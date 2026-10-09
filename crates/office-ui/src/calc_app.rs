//! Calc (spreadsheet) application UI.

use std::path::PathBuf;

use eframe::App;
use egui::{self, Color32, Context, Key, RichText, Sense, Ui, Vec2};
use office_calc::{
    col_to_letters, load_csv_path, load_xlsx_path, write_csv_path, write_xlsx_path, CellAddr,
    CellRange, FormatChange, Workbook,
};

use crate::calc_clipboard::CopiedCells;
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

enum SheetDialog {
    Rename {
        index: usize,
        name: String,
        focus: bool,
        error: Option<String>,
    },
    Delete {
        index: usize,
    },
}

pub struct CalcApp {
    sheet_dialog: Option<SheetDialog>,
    copied_cells: Option<CopiedCells>,
    clipboard: Option<arboard::Clipboard>,
    copy_pending: bool,
    workbook: Workbook,
    file_path: Option<PathBuf>,
    active: CellAddr,
    selection_anchor: CellAddr,
    selection_end: CellAddr,
    dragging_range: bool,
    focus_formula: bool,
    scroll_to_active: bool,
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
        workbook.clear_history();
        let edit_buf = workbook.active_sheet().raw(CellAddr::new(0, 0)).to_owned();

        Self {
            sheet_dialog: None,
            copied_cells: None,
            clipboard: None,
            copy_pending: false,
            workbook,
            file_path: None,
            active: CellAddr::new(0, 0),
            selection_anchor: CellAddr::new(0, 0),
            selection_end: CellAddr::new(0, 0),
            dragging_range: false,
            focus_formula: false,
            scroll_to_active: false,
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
        self.focus_formula = true;
    }

    fn commit_edit(&mut self) {
        if self.editing {
            if self.edit_buf != self.workbook.active_sheet().raw(self.active) {
                self.workbook.set_cell(self.active, self.edit_buf.clone());
            }
            self.editing = false;
            self.focus_formula = false;
            self.set_status(format!("Edited {}", self.active.to_a1()));
        }
    }

    fn cancel_edit(&mut self) {
        self.editing = false;
        self.focus_formula = false;
        self.edit_buf = self.workbook.active_sheet().raw(self.active).to_owned();
    }

    fn selection(&self) -> CellRange {
        CellRange {
            start: self.selection_anchor,
            end: self.selection_end,
        }
        .normalize()
    }

    fn selection_label(&self) -> String {
        let range = self.selection();
        if range.start == range.end {
            range.start.to_a1()
        } else {
            format!("{}:{}", range.start.to_a1(), range.end.to_a1())
        }
    }

    fn select_cell(&mut self, addr: CellAddr, extend: bool) {
        self.commit_edit();
        self.active = addr;
        if !extend {
            self.selection_anchor = addr;
        }
        self.selection_end = addr;
        self.edit_buf = self.workbook.active_sheet().raw(addr).to_owned();
    }

    fn move_active(&mut self, dcol: i32, drow: i32, extend: bool) {
        let sheet = self.workbook.active_sheet();
        let col =
            (self.active.col as i64 + i64::from(dcol)).clamp(0, i64::from(sheet.cols) - 1) as u32;
        let row =
            (self.active.row as i64 + i64::from(drow)).clamp(0, i64::from(sheet.rows) - 1) as u32;
        self.select_cell(CellAddr::new(col, row), extend);
        self.scroll_to_active = true;
    }

    fn switch_sheet(&mut self, index: usize) {
        self.commit_edit();
        self.workbook.set_active_sheet(index);
        self.select_cell(CellAddr::new(0, 0), false);
        self.scroll_to_active = true;
    }

    fn add_sheet(&mut self) {
        self.commit_edit();
        let name = self.workbook.next_sheet_name();
        self.workbook.add_sheet(name);
        self.select_cell(CellAddr::new(0, 0), false);
        self.scroll_to_active = true;
    }

    fn open_sheet_dialog(&mut self, rename: bool) {
        self.commit_edit();
        self.sheet_dialog = Some(if rename {
            SheetDialog::Rename {
                index: self.workbook.active,
                name: self.workbook.active_sheet().name.clone(),
                focus: true,
                error: None,
            }
        } else {
            SheetDialog::Delete {
                index: self.workbook.active,
            }
        });
    }

    fn confirm_sheet_rename(
        &mut self,
        index: usize,
        name: &str,
    ) -> Result<(), office_calc::SheetError> {
        self.workbook.rename_sheet(index, name)?;
        self.set_status("Worksheet renamed");
        Ok(())
    }

    fn confirm_sheet_delete(&mut self, index: usize) {
        match self.workbook.delete_sheet(index) {
            Ok(()) => {
                self.reset_copy();
                self.select_cell(CellAddr::new(0, 0), false);
                self.scroll_to_active = true;
                self.set_status("Worksheet deleted — Undo restores its contents");
            }
            Err(error) => self.set_error(error.to_string()),
        }
    }

    fn show_sheet_dialog(&mut self, ctx: &Context) {
        let Some(mut dialog) = self.sheet_dialog.take() else {
            return;
        };
        let mut close = false;
        egui::Modal::new(egui::Id::new("calc_sheet_dialog")).show(ctx, |ui| {
            match &mut dialog {
                SheetDialog::Rename {
                    index,
                    name,
                    focus,
                    error,
                } => {
                    ui.heading("Rename Worksheet");
                    ui.label("Use a unique name, up to 31 characters.");
                    let input = ui
                        .add(egui::TextEdit::singleline(name).id(egui::Id::new("calc_sheet_name")));
                    if *focus {
                        input.request_focus();
                        *focus = false;
                    }
                    if let Some(error) = error {
                        ui.colored_label(Color32::RED, error);
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Rename").clicked() {
                            match self.confirm_sheet_rename(*index, name) {
                                Ok(()) => close = true,
                                Err(e) => *error = Some(e.to_string()),
                            }
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                }
                SheetDialog::Delete { index } => {
                    ui.heading("Delete Worksheet?");
                    ui.label(format!(
                        "Delete \"{}\" and all its cells?",
                        self.workbook.sheets[*index].name
                    ));
                    ui.label("You can restore this worksheet with Undo.");
                    ui.horizontal(|ui| {
                        if ui.button("Delete Worksheet").clicked() {
                            self.confirm_sheet_delete(*index);
                            close = true;
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                }
            }
            if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Escape)) {
                close = true;
            }
        });
        if close {
            ctx.memory_mut(|memory| memory.surrender_focus(egui::Id::new("calc_sheet_name")));
            ctx.request_repaint();
        } else {
            self.sheet_dialog = Some(dialog);
        }
    }

    fn format_selection(&mut self, change: FormatChange) {
        self.commit_edit();
        match self.workbook.format_range(self.selection(), change) {
            Ok(_) => self.set_status(format!("Formatted {}", self.selection_label())),
            Err(error) => self.set_error(format!("Format failed: {error}")),
        }
    }

    fn clear_selection(&mut self) {
        self.commit_edit();
        self.workbook.clear_range(self.selection());
        self.edit_buf = self.workbook.active_sheet().raw(self.active).to_owned();
        self.set_status(format!("Cleared {}", self.selection_label()));
    }

    fn paste(&mut self, text: &str) {
        let html = if self.copied_cells.is_some() {
            self.clipboard
                .as_mut()
                .and_then(|clipboard| clipboard.get().html().ok())
        } else {
            None
        };
        self.paste_with_html(text, html.as_deref());
    }

    fn paste_with_html(&mut self, text: &str, html: Option<&str>) {
        self.commit_edit();
        let origin = self
            .copied_cells
            .as_ref()
            .and_then(|copy| copy.origin_for(text, html));
        // The model validates the entire table before changing any cells.
        match self
            .workbook
            .paste_tsv_with_origin(self.selection().start, text, origin)
        {
            Ok(range) => {
                self.editing = false;
                self.active = range.start;
                self.selection_anchor = range.start;
                self.selection_end = range.end;
                self.edit_buf = self.workbook.active_sheet().raw(self.active).to_owned();
                self.scroll_to_active = true;
                self.set_status(format!("Pasted {}", self.selection_label()));
            }
            Err(error) => self.set_error(format!("Paste failed: {error}")),
        }
    }

    fn copy_selection(&mut self, ctx: &Context, cut: bool) {
        self.commit_edit();
        match self.workbook.copy_tsv(self.selection()) {
            Ok(text) => {
                ctx.copy_text(text.clone());
                self.copied_cells = Some(CopiedCells::new(text, self.selection().start, cut));
                self.copy_pending = true;
                if cut {
                    self.clear_selection();
                } else {
                    self.set_status(format!("Copied {}", self.selection_label()));
                }
            }
            Err(error) => self.set_error(format!("Copy failed: {error}")),
        }
    }

    fn publish_copy(&mut self, ctx: &Context) {
        if !std::mem::take(&mut self.copy_pending) {
            return;
        }
        let Some(copy) = &self.copied_cells else {
            return;
        };
        let latest_is_copy = ctx.output(|output| {
            output.commands.iter().rev().find_map(|command| {
                if let egui::OutputCommand::CopyText(text) = command {
                    Some(text == &copy.text)
                } else {
                    None
                }
            }) == Some(true)
        });
        if !latest_is_copy {
            return;
        }
        if self.clipboard.is_none() {
            self.clipboard = arboard::Clipboard::new().ok();
        }
        if let Some(clipboard) = &mut self.clipboard {
            let html = copy.html();
            if clipboard
                .set_html(html.as_str(), Some(copy.text.as_str()))
                .is_ok()
            {
                // Prevent egui's later plain-text write from removing provenance.
                ctx.output_mut(|output| {
                    output
                        .commands
                        .retain(|command| !matches!(command, egui::OutputCommand::CopyText(_)));
                });
            }
        }
        // If native HTML is unavailable, egui still publishes the plain text;
        // a subsequent paste has no verified origin and preserves formulas.
    }

    fn reset_copy(&mut self) {
        self.copied_cells = None;
        self.copy_pending = false;
    }

    fn history(&mut self, redo: bool) {
        self.commit_edit();
        let previous_sheet = self.workbook.active;
        let changed = if redo {
            self.workbook.redo()
        } else {
            self.workbook.undo()
        };
        if changed {
            let sheet = self.workbook.active_sheet();
            let clamp = |addr: CellAddr| {
                CellAddr::new(addr.col.min(sheet.cols - 1), addr.row.min(sheet.rows - 1))
            };
            if previous_sheet == self.workbook.active {
                self.active = clamp(self.active);
                self.selection_anchor = clamp(self.selection_anchor);
                self.selection_end = clamp(self.selection_end);
                self.edit_buf = sheet.raw(self.active).to_owned();
            } else {
                self.select_cell(CellAddr::new(0, 0), false);
            }
            self.scroll_to_active = true;
            self.set_status(if redo { "Redo" } else { "Undo" });
        }
    }

    fn leave_formula(ctx: &Context) {
        ctx.memory_mut(|memory| memory.surrender_focus(egui::Id::new("calc_formula_input")));
    }

    fn new_sheet(&mut self) {
        self.request_action(DocumentAction::New);
    }

    fn do_new_sheet(&mut self) {
        self.reset_copy();
        self.workbook = Workbook::new();
        self.file_path = None;
        self.editing = false;
        self.select_cell(CellAddr::new(0, 0), false);
        self.scroll_to_active = true;
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
                self.reset_copy();
                self.workbook = wb;
                self.file_path = Some(path.clone());
                self.editing = false;
                self.select_cell(CellAddr::new(0, 0), false);
                self.scroll_to_active = true;
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
        if ext == "csv"
            && self
                .workbook
                .sheets
                .iter()
                .any(|sheet| sheet.has_formatting())
        {
            self.set_error(
                "Save failed: CSV does not store cell formatting. Choose XLSX to preserve it.",
            );
            return false;
        }
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
                        self.switch_sheet(i);
                        ui.close();
                    }
                }
                ui.separator();
                if ui.button("Rename Worksheet…").clicked() {
                    self.open_sheet_dialog(true);
                    ui.close();
                }
                if ui
                    .add_enabled(
                        self.workbook.sheet_count() > 1,
                        egui::Button::new("Delete Worksheet…"),
                    )
                    .clicked()
                {
                    self.open_sheet_dialog(false);
                    ui.close();
                }
                if ui.button("Add Sheet").clicked() {
                    self.add_sheet();
                    ui.close();
                }
            });
            ui.menu_button("Format", |ui| {
                let current = self.workbook.active_sheet().format(self.active);
                for (label, code) in [
                    ("General", "General"),
                    ("Number (2 decimals)", "0.00"),
                    ("Percentage (2 decimals)", "0.00%"),
                    ("Date (yyyy-mm-dd)", "yyyy-mm-dd"),
                ] {
                    if ui
                        .selectable_label(current.number_format == code, label)
                        .clicked()
                    {
                        self.format_selection(FormatChange::Number(code.into()));
                        ui.close();
                    }
                }
                ui.separator();
                if ui
                    .selectable_label(current.bold, "Bold    Ctrl+B")
                    .clicked()
                {
                    self.format_selection(FormatChange::Bold(!current.bold));
                    ui.close();
                }
                if ui
                    .selectable_label(current.italic, "Italic    Ctrl+I")
                    .clicked()
                {
                    self.format_selection(FormatChange::Italic(!current.italic));
                    ui.close();
                }
                ui.separator();
                if ui.button("Clear Formatting").clicked() {
                    self.format_selection(FormatChange::Clear);
                    ui.close();
                }
            });
            ui.menu_button("Edit", |ui| {
                if ui
                    .add_enabled(
                        self.workbook.can_undo() || self.is_dirty() && self.editing,
                        egui::Button::new("Undo    Ctrl+Z"),
                    )
                    .clicked()
                {
                    self.history(false);
                    Self::leave_formula(ui.ctx());
                    ui.close();
                }
                if ui
                    .add_enabled(
                        self.workbook.can_redo(),
                        egui::Button::new("Redo    Ctrl+Shift+Z"),
                    )
                    .clicked()
                {
                    self.history(true);
                    Self::leave_formula(ui.ctx());
                    ui.close();
                }
                ui.separator();
                for (label, command) in [
                    ("Copy    Ctrl+C", egui::ViewportCommand::RequestCopy),
                    ("Cut    Ctrl+X", egui::ViewportCommand::RequestCut),
                    ("Paste    Ctrl+V", egui::ViewportCommand::RequestPaste),
                ] {
                    if ui.button(label).clicked() {
                        self.commit_edit();
                        Self::leave_formula(ui.ctx());
                        ui.ctx().send_viewport_cmd(command);
                        ui.close();
                    }
                }
                ui.separator();
                if ui.button("Clear Selected Cells    Delete").clicked() {
                    self.clear_selection();
                    Self::leave_formula(ui.ctx());
                    ui.close();
                }
            });
        });
    }

    fn formula_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(self.selection_label()).strong().color(ACCENT));
            ui.separator();
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.edit_buf)
                    .id(egui::Id::new("calc_formula_input"))
                    .desired_width((ui.available_width() - 80.0).max(40.0))
                    .hint_text("value or =formula"),
            );
            if self.focus_formula {
                response.request_focus();
                self.focus_formula = false;
            }
            if response.gained_focus() || response.changed() {
                self.editing = true;
            }
            if response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                self.commit_edit();
                self.move_active(0, 1, false);
            }
            if ui.button("✓").clicked() {
                self.commit_edit();
                Self::leave_formula(ui.ctx());
            }
            if ui.button("✗").clicked() {
                self.cancel_edit();
                Self::leave_formula(ui.ctx());
            }
        });
    }

    fn grid(&mut self, ui: &mut Ui) {
        let cols = self.workbook.active_sheet().cols;
        let rows = self.workbook.active_sheet().rows;
        // Only paint the visible cells; imported/pasted sheets can exceed the
        // old 40-column/80-row display limit without creating millions of widgets.
        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .show_viewport(ui, |ui, viewport| {
                let size = Vec2::new(
                    HEADER_W + cols as f32 * COL_WIDTH,
                    (rows + 1) as f32 * ROW_HEIGHT,
                );
                let (grid_rect, _) = ui.allocate_exact_size(size, Sense::hover());
                let origin = grid_rect.min;
                let cells_rect = egui::Rect::from_min_max(
                    origin + Vec2::new(HEADER_W, ROW_HEIGHT),
                    grid_rect.max,
                );
                let response = ui.interact(
                    cells_rect,
                    egui::Id::new("calc_grid"),
                    Sense::click_and_drag(),
                );
                let cell_at = |pos: egui::Pos2| {
                    let offset = pos - origin - Vec2::new(HEADER_W, ROW_HEIGHT);
                    CellAddr::new(
                        (offset.x / COL_WIDTH).floor().clamp(0.0, (cols - 1) as f32) as u32,
                        (offset.y / ROW_HEIGHT)
                            .floor()
                            .clamp(0.0, (rows - 1) as f32) as u32,
                    )
                };
                if ui.is_enabled() {
                    if response.contains_pointer() && ui.input(|i| i.pointer.primary_pressed()) {
                        if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
                            self.select_cell(cell_at(pos), ui.input(|i| i.modifiers.shift));
                            Self::leave_formula(ui.ctx());
                            self.dragging_range = true;
                        }
                    }
                    if self.dragging_range && ui.input(|i| i.pointer.primary_down()) {
                        if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
                            self.select_cell(cell_at(pos), true);
                        }
                    } else {
                        self.dragging_range = false;
                    }
                    if response.double_clicked() {
                        self.begin_edit();
                    }
                }
                if self.scroll_to_active {
                    let min = origin
                        + Vec2::new(
                            HEADER_W + self.active.col as f32 * COL_WIDTH,
                            (self.active.row + 1) as f32 * ROW_HEIGHT,
                        );
                    ui.scroll_to_rect(
                        egui::Rect::from_min_size(min, Vec2::new(COL_WIDTH, ROW_HEIGHT)),
                        None,
                    );
                    self.scroll_to_active = false;
                }
                let first_col = ((viewport.min.x - HEADER_W) / COL_WIDTH).floor().max(0.0) as u32;
                let last_col =
                    (((viewport.max.x - HEADER_W) / COL_WIDTH).ceil().max(0.0) as u32).min(cols);
                let first_row = ((viewport.min.y - ROW_HEIGHT) / ROW_HEIGHT)
                    .floor()
                    .max(0.0) as u32;
                let last_row =
                    (((viewport.max.y - ROW_HEIGHT) / ROW_HEIGHT).ceil().max(0.0) as u32).min(rows);
                let painter = ui.painter();
                let selection = self.selection();
                for col in first_col..last_col {
                    let rect = egui::Rect::from_min_size(
                        origin + Vec2::new(HEADER_W + col as f32 * COL_WIDTH, 0.0),
                        Vec2::new(COL_WIDTH, ROW_HEIGHT),
                    );
                    painter.rect_filled(rect, 0.0, Color32::from_gray(230));
                    painter.text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        col_to_letters(col),
                        egui::FontId::proportional(12.0),
                        Color32::DARK_GRAY,
                    );
                }
                for row in first_row..last_row {
                    let top = origin.y + (row + 1) as f32 * ROW_HEIGHT;
                    let header = egui::Rect::from_min_size(
                        egui::pos2(origin.x, top),
                        Vec2::new(HEADER_W, ROW_HEIGHT),
                    );
                    painter.rect_filled(header, 0.0, Color32::from_gray(230));
                    painter.text(
                        header.center(),
                        egui::Align2::CENTER_CENTER,
                        (row + 1).to_string(),
                        egui::FontId::proportional(11.0),
                        Color32::DARK_GRAY,
                    );
                    for col in first_col..last_col {
                        let addr = CellAddr::new(col, row);
                        let rect = egui::Rect::from_min_size(
                            egui::pos2(origin.x + HEADER_W + col as f32 * COL_WIDTH, top),
                            Vec2::new(COL_WIDTH, ROW_HEIGHT),
                        );
                        let bg = if selection.contains(addr) {
                            Color32::from_rgb(210, 230, 255)
                        } else {
                            Color32::WHITE
                        };
                        painter.rect_filled(rect, 0.0, bg);
                        painter.rect_stroke(
                            rect,
                            0.0,
                            egui::Stroke::new(1.0, Color32::from_gray(200)),
                            egui::StrokeKind::Inside,
                        );
                        let style = self.workbook.active_sheet().format(addr);
                        let job = egui::text::LayoutJob::single_section(
                            self.workbook.active_sheet().display(addr),
                            egui::TextFormat {
                                font_id: egui::FontId::proportional(13.0),
                                color: Color32::BLACK,
                                italics: style.italic,
                                ..Default::default()
                            },
                        );
                        let galley = painter.layout_job(job);
                        let pos = rect.left_center() + Vec2::new(4.0, -galley.size().y / 2.0);
                        let clipped =
                            painter.with_clip_rect(rect.intersect(ui.clip_rect()).shrink(2.0));
                        clipped.galley(pos, galley.clone(), Color32::BLACK);
                        // Synthetic weight keeps CJK fallback glyphs visibly bold too.
                        if style.bold {
                            clipped.galley(pos + Vec2::new(0.45, 0.0), galley, Color32::BLACK);
                        }
                        if addr == self.active {
                            painter.rect_stroke(
                                rect,
                                0.0,
                                egui::Stroke::new(2.0, ACCENT),
                                egui::StrokeKind::Inside,
                            );
                        }
                    }
                }
            });
    }

    fn handle_keys(&mut self, ctx: &Context) {
        if self.sheet_dialog.is_some() || ctx.memory(|memory| memory.top_modal_layer().is_some()) {
            return;
        }
        let command = egui::Modifiers::COMMAND;
        if ctx.input_mut(|i| i.consume_key(command | egui::Modifiers::SHIFT, Key::S)) {
            self.save_file_as();
        } else if ctx.input_mut(|i| i.consume_key(command, Key::S)) {
            self.save_file();
        }
        if ctx.input_mut(|i| i.consume_key(command, Key::N)) {
            self.new_sheet();
        }
        if ctx.input_mut(|i| i.consume_key(command, Key::O)) {
            self.open_file();
        }
        if self.pending_action.is_some() {
            return;
        }
        if ctx.egui_wants_keyboard_input() {
            if ctx.input(|input| {
                input
                    .events
                    .iter()
                    .any(|event| matches!(event, egui::Event::Copy | egui::Event::Cut))
            }) {
                self.reset_copy();
            }
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape)) {
                self.cancel_edit();
                Self::leave_formula(ctx);
            }
            return;
        }
        if ctx.input_mut(|i| i.consume_key(command, Key::B)) {
            let bold = self.workbook.active_sheet().format(self.active).bold;
            self.format_selection(FormatChange::Bold(!bold));
            return;
        }
        if ctx.input_mut(|i| i.consume_key(command, Key::I)) {
            let italic = self.workbook.active_sheet().format(self.active).italic;
            self.format_selection(FormatChange::Italic(!italic));
            return;
        }
        let mut clipboard_events = Vec::new();
        ctx.input_mut(|i| {
            i.events.retain(|event| {
                if matches!(
                    event,
                    egui::Event::Copy | egui::Event::Cut | egui::Event::Paste(_)
                ) {
                    clipboard_events.push(event.clone());
                    false
                } else {
                    true
                }
            })
        });
        for event in clipboard_events {
            match event {
                egui::Event::Copy => self.copy_selection(ctx, false),
                egui::Event::Cut => self.copy_selection(ctx, true),
                egui::Event::Paste(text) => self.paste(&text),
                _ => {}
            }
        }
        let redo = ctx.input_mut(|i| {
            i.consume_key(command | egui::Modifiers::SHIFT, Key::Z)
                || i.consume_key(command, Key::Y)
        });
        if redo {
            self.history(true);
        } else if ctx.input_mut(|i| i.consume_key(command, Key::Z)) {
            self.history(false);
        } else if ctx.input_mut(|i| i.consume_key(command, Key::A)) {
            self.commit_edit();
            self.selection_anchor = CellAddr::new(0, 0);
            self.selection_end = CellAddr::new(
                self.workbook.active_sheet().cols - 1,
                self.workbook.active_sheet().rows - 1,
            );
        } else {
            let shift = ctx.input(|i| i.modifiers.shift);
            let modifiers = if shift {
                egui::Modifiers::SHIFT
            } else {
                egui::Modifiers::NONE
            };
            for (key, dx, dy) in [
                (Key::ArrowLeft, -1, 0),
                (Key::ArrowRight, 1, 0),
                (Key::ArrowUp, 0, -1),
                (Key::ArrowDown, 0, 1),
            ] {
                if ctx.input_mut(|i| i.consume_key(modifiers, key)) {
                    self.move_active(dx, dy, shift);
                    return;
                }
            }
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter)) {
                self.move_active(0, 1, false);
            } else if ctx.input_mut(|i| i.consume_key(modifiers, Key::Tab)) {
                self.move_active(if shift { -1 } else { 1 }, 0, false);
            } else if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::F2)) {
                self.begin_edit();
            } else if ctx.input_mut(|i| {
                i.consume_key(egui::Modifiers::NONE, Key::Delete)
                    || i.consume_key(egui::Modifiers::NONE, Key::Backspace)
            }) {
                self.clear_selection();
            } else if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape)) {
                self.cancel_edit();
                self.select_cell(self.active, false);
            } else {
                let mut text = String::new();
                ctx.input_mut(|i| {
                    i.events.retain(|event| {
                        if let egui::Event::Text(value) = event {
                            text.push_str(value);
                            false
                        } else {
                            true
                        }
                    })
                });
                if !text.is_empty() {
                    self.begin_edit();
                    self.edit_buf = text;
                }
            }
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
        self.show_sheet_dialog(&ctx);
        if self.pending_action.is_some() || self.sheet_dialog.is_some() {
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
        self.publish_copy(&ctx);
    }
}

fn writable_extension(path: &std::path::Path) -> Option<String> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    matches!(extension.as_str(), "xlsx" | "csv").then_some(extension)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting_commits_draft_preserves_formulas_and_reloads_from_xlsx() {
        let mut app = CalcApp::new();
        app.select_cell(CellAddr::new(2, 3), false);
        draft(&mut app, "=B2*C2+B3*C3");
        app.format_selection(FormatChange::Number("0.00".into()));
        app.format_selection(FormatChange::Bold(true));
        app.format_selection(FormatChange::Italic(true));
        assert!(!app.editing);
        assert_eq!(app.workbook.active_sheet().display(app.active), "5.20");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("formatted.xlsx");
        assert!(app.save_to(&path));
        let loaded = load_xlsx_path(&path).unwrap();
        assert_eq!(
            loaded.active_sheet().format(app.active),
            app.workbook.active_sheet().format(app.active)
        );
        assert_eq!(loaded.active_sheet().raw(app.active), "=B2*C2+B3*C3");
        assert_eq!(loaded.active_sheet().display(app.active), "5.20");
        app.history(false);
        assert!(app.is_dirty());
        app.history(true);
        assert!(!app.is_dirty());
    }

    #[test]
    fn csv_save_cannot_drop_styling_or_commit_a_pending_draft() {
        let mut app = CalcApp::new();
        app.format_selection(FormatChange::Bold(true));
        draft(&mut app, "keep this draft");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("original.csv");
        std::fs::write(&path, "original bytes").unwrap();
        assert!(!app.save_to(&path));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "original bytes");
        assert!(app.editing && app.is_dirty());
        assert_eq!(app.edit_buf, "keep this draft");
        assert!(app.file_path.is_none());
        assert!(app.last_error.as_deref().unwrap().contains("formatting"));
        assert!(app.workbook.can_undo());
    }

    #[test]
    fn formatting_shortcuts_apply_to_grid_and_stay_out_of_rename_modals() {
        let mut app = CalcApp::new();
        let ctx = Context::default();
        let bold = egui::Event::Key {
            key: Key::B,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        };
        let mut output = ctx.run_ui(
            egui::RawInput {
                events: vec![bold.clone()],
                ..Default::default()
            },
            |ui| app.handle_keys(ui.ctx()),
        );
        output.textures_delta.clear();
        assert!(app.workbook.active_sheet().format(app.active).bold);
        app.workbook.mark_clean();
        app.open_sheet_dialog(true);
        let mut output = ctx.run_ui(
            egui::RawInput {
                events: vec![bold],
                ..Default::default()
            },
            |ui| {
                app.handle_keys(ui.ctx());
                app.show_sheet_dialog(ui.ctx());
            },
        );
        output.textures_delta.clear();
        assert!(app.workbook.active_sheet().format(app.active).bold);
        assert!(!app.is_dirty());
    }

    #[test]
    fn sheet_management_round_trip_and_undo_preserve_drafts_and_formulas() {
        let mut app = CalcApp::new();
        draft(&mut app, "日本語");
        app.add_sheet();
        assert_eq!(app.workbook.sheets[0].raw(CellAddr::new(0, 0)), "日本語");
        draft(&mut app, "=1+2");
        app.open_sheet_dialog(true);
        app.confirm_sheet_rename(1, "売上 & <集計>").unwrap();
        app.sheet_dialog = None;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("managed.xlsx");
        assert!(app.save_to(&path));
        app.confirm_sheet_delete(1);
        assert_eq!(app.workbook.sheet_count(), 1);
        app.history(false);
        assert!(!app.is_dirty());
        assert_eq!(app.workbook.active_sheet().raw(CellAddr::new(0, 0)), "=1+2");
        app.history(true);
        assert!(app.save_to(&path));
        let loaded = load_xlsx_path(&path).unwrap();
        assert_eq!(loaded.sheet_count(), 1);
        assert_eq!(loaded.active_sheet().raw(CellAddr::new(0, 0)), "日本語");
        app.history(false);
        assert!(app.is_dirty());
        assert!(app.save_to(&path));
        let loaded = load_xlsx_path(&path).unwrap();
        assert_eq!(loaded.sheets[1].name, "売上 & <集計>");
        assert_eq!(loaded.sheets[1].raw(CellAddr::new(0, 0)), "=1+2");
        assert_eq!(loaded.sheets[1].display(CellAddr::new(0, 0)), "3");
        assert!(!loaded.can_undo() && !loaded.is_dirty());
    }

    #[test]
    fn cancelling_sheet_modals_and_typing_do_not_edit_cells() {
        let mut app = CalcApp::new();
        app.add_sheet();
        app.workbook.mark_clean();
        app.workbook.clear_history();
        let ctx = Context::default();
        for rename in [true, false] {
            app.open_sheet_dialog(rename);
            for events in [
                vec![],
                vec![egui::Event::Text("draft name".into())],
                vec![egui::Event::Key {
                    key: Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                vec![],
                vec![],
            ] {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        let ctx = ui.ctx().clone();
                        app.handle_keys(&ctx);
                        app.show_sheet_dialog(&ctx);
                    },
                );
                output.textures_delta.clear();
            }
            assert!(app.sheet_dialog.is_none());
            assert_eq!(app.workbook.sheet_count(), 2);
            assert_eq!(app.workbook.active_sheet().name, "Sheet2");
            assert_eq!(app.workbook.active_sheet().raw(CellAddr::new(0, 0)), "");
            assert!(!app.is_dirty() && !app.workbook.can_undo());
        }
    }

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

#[cfg(test)]
mod range_tests {
    use super::*;

    fn key(key: Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    fn input(
        app: &mut CalcApp,
        ctx: &Context,
        mut events: Vec<egui::Event>,
        modifiers: egui::Modifiers,
    ) -> egui::FullOutput {
        events.insert(0, egui::Event::ModifiersChanged(modifiers));
        ctx.run_ui(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ui| app.handle_keys(ui.ctx()),
        )
    }

    #[test]
    fn clipboard_events_copy_paste_cut_and_undo_a_whole_range() {
        let mut app = CalcApp::new();
        let ctx = Context::default();
        app.select_cell(CellAddr::new(0, 0), false);
        app.select_cell(CellAddr::new(2, 3), true);
        let expected = app.workbook.copy_tsv(app.selection()).unwrap();
        let output = input(
            &mut app,
            &ctx,
            vec![egui::Event::Copy],
            egui::Modifiers::NONE,
        );
        assert!(output
            .platform_output
            .commands
            .contains(&egui::OutputCommand::CopyText(expected.clone())));
        output.drop_without_applying_deltas();
        assert!(!app.is_dirty());
        app.select_cell(CellAddr::new(4, 0), false);
        input(
            &mut app,
            &ctx,
            vec![egui::Event::Paste(expected.clone())],
            egui::Modifiers::NONE,
        )
        .drop_without_applying_deltas();
        assert_eq!(app.selection_label(), "E1:G4");
        assert_eq!(
            app.workbook.active_sheet().raw(CellAddr::new(6, 3)),
            "=B2*C2+B3*C3"
        );
        input(
            &mut app,
            &ctx,
            vec![egui::Event::Cut],
            egui::Modifiers::NONE,
        )
        .drop_without_applying_deltas();
        assert_eq!(app.workbook.active_sheet().raw(CellAddr::new(6, 3)), "");
        app.history(false);
        assert_eq!(app.workbook.copy_tsv(app.selection()).unwrap(), expected);
        app.history(false);
        assert!(!app.is_dirty());
        assert_eq!(app.workbook.active_sheet().raw(CellAddr::new(4, 0)), "");
    }

    #[test]
    fn shift_arrows_expand_and_contract_across_the_anchor() {
        let mut app = CalcApp::new();
        let ctx = Context::default();
        app.select_cell(CellAddr::new(2, 2), false);
        for key_code in [Key::ArrowLeft, Key::ArrowUp, Key::ArrowUp] {
            input(
                &mut app,
                &ctx,
                vec![key(key_code, egui::Modifiers::SHIFT)],
                egui::Modifiers::SHIFT,
            )
            .drop_without_applying_deltas();
        }
        assert_eq!(app.selection_label(), "B1:C3");
        for key_code in [Key::ArrowDown, Key::ArrowDown, Key::ArrowDown] {
            input(
                &mut app,
                &ctx,
                vec![key(key_code, egui::Modifiers::SHIFT)],
                egui::Modifiers::SHIFT,
            )
            .drop_without_applying_deltas();
        }
        assert_eq!(app.selection_label(), "B3:C4");
        input(
            &mut app,
            &ctx,
            vec![key(Key::ArrowRight, egui::Modifiers::NONE)],
            egui::Modifiers::NONE,
        )
        .drop_without_applying_deltas();
        assert_eq!(app.selection_label(), "C4");
    }

    #[test]
    fn redo_shortcut_does_not_trigger_undo() {
        let mut app = CalcApp::new();
        let ctx = Context::default();
        app.paste("changed");
        let command = egui::Modifiers::CTRL | egui::Modifiers::COMMAND;
        input(&mut app, &ctx, vec![key(Key::Z, command)], command).drop_without_applying_deltas();
        assert!(!app.is_dirty());
        let redo = command | egui::Modifiers::SHIFT;
        input(&mut app, &ctx, vec![key(Key::Z, redo)], redo).drop_without_applying_deltas();
        assert_eq!(app.edit_buf, "changed");
        assert!(app.is_dirty());
    }

    #[test]
    fn xlsx_save_reload_and_undo_preserve_pasted_formulas_and_saved_state() {
        let mut app = CalcApp::new();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("range.xlsx");
        app.select_cell(CellAddr::new(0, 60), false);
        app.paste("りんご\t3\t=SUM(B61:B62)\r\nみかん\t2\t\r\n");
        assert!(app.save_to(&path));
        let restored = load_xlsx_path(&path).unwrap();
        assert_eq!(restored.active_sheet().raw(CellAddr::new(0, 60)), "りんご");
        assert_eq!(
            restored.active_sheet().raw(CellAddr::new(2, 60)),
            "=SUM(B61:B62)"
        );
        assert_eq!(restored.active_sheet().display(CellAddr::new(2, 60)), "5");
        assert!(!restored.is_dirty() && !restored.can_undo());
        app.history(false);
        assert!(app.is_dirty());
        app.history(true);
        assert!(!app.is_dirty());
        assert_eq!(app.file_path.as_deref(), Some(path.as_path()));
    }

    #[test]
    fn malformed_paste_preserves_selection_values_and_history() {
        let mut app = CalcApp::new();
        app.select_cell(CellAddr::new(2, 3), false);
        let selection = app.selection();
        app.paste("\"unterminated");
        assert!(app.last_error.as_ref().unwrap().contains("Paste failed"));
        assert_eq!(app.selection(), selection);
        assert_eq!(app.edit_buf, "=B2*C2+B3*C3");
        assert!(!app.is_dirty() && !app.workbook.can_undo());
    }

    #[test]
    fn sheet_changes_commit_drafts_to_the_original_sheet() {
        let mut app = CalcApp::new();
        app.begin_edit();
        app.edit_buf = "first draft".into();
        app.add_sheet();
        assert_eq!(
            app.workbook.sheets[0].raw(CellAddr::new(0, 0)),
            "first draft"
        );
        assert_eq!(app.edit_buf, "");
        app.begin_edit();
        app.edit_buf = "second draft".into();
        app.switch_sheet(0);
        assert_eq!(
            app.workbook.sheets[1].raw(CellAddr::new(0, 0)),
            "second draft"
        );
        app.history(false);
        assert_eq!(app.workbook.active, 1);
        assert_eq!(app.edit_buf, "");
        app.history(false);
        assert_eq!(app.workbook.sheet_count(), 1);
        assert_eq!(app.edit_buf, "first draft");
        app.history(false);
        assert_eq!(app.edit_buf, "Item");
        assert!(!app.is_dirty());
    }

    #[test]
    fn clipboard_stays_in_the_formula_bar_after_saving_a_draft() {
        let mut app = CalcApp::new();
        let ctx = Context::default();
        let dir = tempfile::tempdir().unwrap();
        app.begin_edit();
        app.edit_buf = "saved".into();
        for _ in 0..2 {
            ctx.run_ui(egui::RawInput::default(), |ui| app.formula_bar(ui))
                .drop_without_applying_deltas();
        }
        assert!(ctx.egui_wants_keyboard_input());
        assert!(app.save_to(&dir.path().join("formula.xlsx")));
        ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Paste("more\ttext".into())],
                ..Default::default()
            },
            |ui| {
                app.handle_keys(ui.ctx());
                app.formula_bar(ui);
            },
        )
        .drop_without_applying_deltas();
        assert!(app.edit_buf.contains("more"));
        assert_eq!(
            app.workbook.active_sheet().raw(CellAddr::new(0, 0)),
            "saved"
        );
        assert_eq!(app.workbook.active_sheet().raw(CellAddr::new(1, 0)), "Qty");
        assert_eq!(app.selection_label(), "A1");
        assert!(app.is_dirty());
    }

    #[test]
    fn draft_saved_in_formula_bar_can_be_undone_and_redone() {
        let mut app = CalcApp::new();
        let dir = tempfile::tempdir().unwrap();
        app.begin_edit();
        app.edit_buf = "saved draft".into();
        assert!(app.save_to(&dir.path().join("draft.xlsx")));
        app.history(false);
        assert_eq!(app.edit_buf, "Item");
        assert!(app.is_dirty());
        app.history(true);
        assert_eq!(app.edit_buf, "saved draft");
        assert!(!app.is_dirty());
    }
}

#[cfg(test)]
mod reference_tests {
    use super::*;

    fn copy(app: &mut CalcApp, cut: bool) -> (String, String) {
        let ctx = Context::default();
        ctx.run_ui(egui::RawInput::default(), |ui| {
            app.copy_selection(ui.ctx(), cut)
        })
        .drop_without_applying_deltas();
        let copied = app.copied_cells.as_ref().unwrap();
        (copied.text.clone(), copied.html())
    }

    #[test]
    fn copied_formulas_recalculate_and_survive_history_save_and_reload() {
        let mut app = CalcApp::new();
        let dir = tempfile::tempdir().unwrap();
        app.select_cell(CellAddr::new(0, 0), false);
        app.select_cell(CellAddr::new(2, 3), true);
        let (text, html) = copy(&mut app, false);
        app.select_cell(CellAddr::new(4, 0), false);
        app.paste_with_html(&text, Some(&html));
        let total = CellAddr::new(6, 3);
        assert_eq!(app.workbook.active_sheet().raw(total), "=F2*G2+F3*G3");
        assert_eq!(app.workbook.active_sheet().display(total), "5.2");
        app.history(false);
        assert!(!app.is_dirty());
        app.history(true);
        let path = dir.path().join("copied.xlsx");
        assert!(app.save_to(&path));
        let loaded = load_xlsx_path(&path).unwrap();
        assert_eq!(loaded.active_sheet().raw(total), "=F2*G2+F3*G3");
        assert_eq!(loaded.active_sheet().display(total), "5.2");
        app.history(false);
        assert!(app.is_dirty());
        app.history(true);
        assert!(!app.is_dirty());
    }

    #[test]
    fn repeated_paste_uses_the_captured_original_not_edited_source_cells() {
        let mut app = CalcApp::new();
        app.select_cell(CellAddr::new(2, 3), false);
        let (text, html) = copy(&mut app, false);
        app.workbook.set_cell(app.active, "=100");
        for (destination, expected) in [
            (CellAddr::new(3, 4), "=C3*D3+C4*D4"),
            (CellAddr::new(4, 5), "=D4*E4+D5*E5"),
        ] {
            app.select_cell(destination, false);
            app.paste_with_html(&text, Some(&html));
            assert_eq!(app.workbook.active_sheet().raw(destination), expected);
        }
    }

    #[test]
    fn external_identical_text_and_cut_pastes_keep_formulas_verbatim() {
        for cut in [false, true] {
            let mut app = CalcApp::new();
            app.select_cell(CellAddr::new(2, 3), false);
            let (text, html) = copy(&mut app, cut);
            app.select_cell(CellAddr::new(4, 5), false);
            app.paste_with_html(&text, if cut { Some(&html) } else { None });
            assert_eq!(app.workbook.active_sheet().raw(app.active), "=B2*C2+B3*C3");
        }
    }

    #[test]
    fn new_workbooks_and_formula_bar_copy_invalidate_range_provenance() {
        let mut app = CalcApp::new();
        app.select_cell(CellAddr::new(2, 3), false);
        let (text, html) = copy(&mut app, false);
        app.do_new_sheet();
        app.select_cell(CellAddr::new(4, 5), false);
        app.paste_with_html(&text, Some(&html));
        assert_eq!(app.workbook.active_sheet().raw(app.active), "=B2*C2+B3*C3");
        copy(&mut app, false);
        let ctx = Context::default();
        app.begin_edit();
        for _ in 0..2 {
            ctx.run_ui(egui::RawInput::default(), |ui| app.formula_bar(ui))
                .drop_without_applying_deltas();
        }
        ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Copy],
                ..Default::default()
            },
            |ui| {
                app.handle_keys(ui.ctx());
                app.formula_bar(ui);
            },
        )
        .drop_without_applying_deltas();
        assert!(app.copied_cells.is_none());
    }

    #[test]
    fn copied_ref_errors_survive_xlsx_save_reload() {
        let mut app = CalcApp::new();
        app.select_cell(CellAddr::new(1, 1), false);
        app.paste("=SUM(A1:B2)+$A$1");
        let (text, html) = copy(&mut app, false);
        app.select_cell(CellAddr::new(0, 0), false);
        app.paste_with_html(&text, Some(&html));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ref-error.xlsx");
        assert!(app.save_to(&path));
        let restored = load_xlsx_path(&path).unwrap();
        assert_eq!(
            restored.active_sheet().raw(CellAddr::new(0, 0)),
            "=SUM(#REF!)+$A$1"
        );
        assert_eq!(
            restored.active_sheet().display(CellAddr::new(0, 0)),
            "#REF!"
        );
    }
}
