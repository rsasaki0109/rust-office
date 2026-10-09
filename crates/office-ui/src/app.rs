//! Writer application state and UI chrome.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use eframe::App;
use egui::{
    self, Color32, Context, Key, KeyboardShortcut, Modifiers, PointerButton, Pos2, Rect, RichText,
    ScrollArea, Sense, TextureHandle, Ui, Vec2,
};
use office_core::{Alignment, DocumentEditor, EditFocus, NamedParagraphStyle, Selection};
use office_format::{load_document, save_document};
use office_render::{
    document_canvas_size, ensure_image_textures, image_from_path_fitted, ime_caret_screen_rect,
    layout_document, paint_document, write_document_layout_pdf_path, DocumentLayout, HitResult,
    VerticalDir,
};

use crate::print_sys::{self, PrintAction};
use crate::theme::{ACCENT, CANVAS_BG, STATUS_BG, TOOLBAR_BG};
use crate::unsaved::{self, Choice, DocumentAction as PendingAction};

const FONT_SIZES: &[f32] = &[
    8.0, 9.0, 10.0, 11.0, 12.0, 14.0, 16.0, 18.0, 20.0, 24.0, 28.0, 36.0,
];

/// Request to switch the suite app mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchTo {
    Calc,
    Impress,
    Quit,
}

pub struct WriterApp {
    editor: DocumentEditor,
    recovery: Option<crate::recovery::Recovery>,
    recovery_open: bool,
    recovery_selected: usize,
    recovery_delete: Option<usize>,
    file_path: Option<PathBuf>,
    zoom: f32,
    status_message: String,
    last_error: Option<String>,
    ime_preedit: String,
    /// Active conversion range within [`Self::ime_preedit`] (char indices).
    ime_active_range: Option<std::ops::Range<usize>>,
    dragging: bool,
    caret_blink_start: Instant,
    clipboard: Option<arboard::Clipboard>,
    focus_document: bool,
    /// Last frame's page layout (for visual-line caret movement).
    last_layout: Option<DocumentLayout>,
    margin_page: usize,
    /// Preferred screen X while moving with ↑/↓ (cleared on horizontal moves).
    preferred_caret_x: Option<f32>,
    /// When set, show the unsaved-changes modal.
    pending_action: Option<PendingAction>,
    /// Decoded GPU textures keyed by [`office_core::Image::cache_key`].
    image_textures: HashMap<String, TextureHandle>,
    /// When set, show the hyperlink URL modal; holds the draft URL.
    link_dialog: Option<String>,
    /// When set, show header/footer modal: (header draft, footer draft).
    header_footer_dialog: Option<crate::page_setup::PageSetupDraft>,
    /// When set, show the print dialog.
    print_dialog: Option<PrintDialog>,
    find_open: bool,
    find_query: String,
    find_replacement: String,
    find_focus_requested: bool,
    find_message: String,
    find_scroll_pending: bool,
    find_restore_focus: bool,
    /// Suite mode switch request.
    pub pending_switch: Option<SwitchTo>,
}

/// Print dialog state.
#[derive(Debug, Clone)]
struct PrintDialog {
    /// Estimated page count from on-screen layout (best-effort).
    layout_pages: usize,
}

impl WriterApp {
    pub fn new() -> Self {
        let mut editor = DocumentEditor::default();
        let _ = editor.insert_text(
            "Welcome to rust-office Writer.\n\n\
             Start typing to edit this document.\n\
             Use the toolbar for Bold, Italic, Underline, and alignment.\n\
             File → Save As can write `.odt`, `.docx`, or `.roffice.json`.\n\n\
             Tip: keep typing past the first page — extra pages appear automatically \
             and the status bar shows Page N of M.",
        );
        editor.mark_clean();
        editor.set_selection(Selection::caret(office_core::DocPosition::zero()));

        Self {
            editor,
            recovery: None,
            recovery_open: false,
            recovery_selected: 0,
            recovery_delete: None,
            file_path: None,
            zoom: 1.0,
            status_message: "Ready".into(),
            last_error: None,
            ime_preedit: String::new(),
            ime_active_range: None,
            dragging: false,
            caret_blink_start: Instant::now(),
            clipboard: arboard::Clipboard::new().ok(),
            focus_document: true,
            last_layout: None,
            margin_page: 0,
            preferred_caret_x: None,
            pending_action: None,
            image_textures: HashMap::new(),
            link_dialog: None,
            header_footer_dialog: None,
            print_dialog: None,
            find_open: false,
            find_query: String::new(),
            find_replacement: String::new(),
            find_focus_requested: false,
            find_message: String::new(),
            find_scroll_pending: false,
            find_restore_focus: false,
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
            Err(e) => self.set_error(format!("Writer recovery unavailable: {e}")),
        }
    }
    pub(super) fn recovery_tick(&mut self, ctx: &Context) {
        let label = self
            .file_path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled Writer document".into());
        if let Some(store) = &mut self.recovery {
            let result = store.tick(
                self.editor.document(),
                self.editor.is_dirty(),
                &label,
                Instant::now(),
            );
            ctx.request_repaint_after(crate::recovery::INTERVAL);
            if let Err(e) = result {
                self.set_error(format!("Writer recovery copy failed: {e}"));
            }
        }
    }
    pub(super) fn clear_recovery(&mut self) {
        if let Some(store) = &mut self.recovery {
            if let Err(e) = store.clear() {
                self.set_error(format!("Writer recovery cleanup failed: {e}"));
            }
        }
    }
    fn open_recovery(&mut self) {
        let Some(store) = &mut self.recovery else {
            self.set_error("Writer recovery is unavailable");
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
    fn restore_recovery(&mut self, index: usize) -> bool {
        if self.editor.is_dirty() {
            self.set_error("Save the current document before recovering another copy");
            return false;
        }
        let Some(store) = &mut self.recovery else {
            return false;
        };
        match store.restore(index) {
            Ok(doc) => {
                let warning = store.cleanup_warning.take();
                self.editor.restore_unsaved_document(doc);
                self.file_path = None;
                self.image_textures.clear();
                self.last_layout = None;
                self.preferred_caret_x = None;
                self.ime_preedit.clear();
                self.ime_active_range = None;
                self.focus_document = true;
                self.find_open = false;
                self.recovery_open = false;
                self.recovery_delete = None;
                self.recovery.as_mut().unwrap().postpone();
                self.set_status("Recovered unsaved Writer document — use Save As to keep it");
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
    fn show_recovery_dialog(&mut self, ctx: &Context) {
        if !self.recovery_open {
            return;
        }
        let mut restore = None;
        let mut delete = None;
        let mut later = false;
        let response = egui::Modal::new(egui::Id::new("writer_recovery")).show(ctx, |ui| {
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
                ui.heading("Unsaved Writer documents found");
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
                if self.editor.is_dirty() {
                    ui.label("Save your current document before recovering another copy.");
                }
                if let Some(error) = &self.last_error {
                    ui.colored_label(Color32::RED, error);
                }
                let can_restore = count > 0 && !self.editor.is_dirty();
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
                if can_restore && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter)) {
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
            self.restore_recovery(index);
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
            .unwrap_or_else(|| "Untitled".into());
        let dirty = if self.editor.is_dirty() { " *" } else { "" };
        format!("{name}{dirty} — rust-office Writer")
    }

    fn set_status(&mut self, msg: impl Into<String>) {
        self.status_message = msg.into();
        self.last_error = None;
    }

    fn set_error(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        self.status_message = msg.clone();
        self.last_error = Some(msg);
    }

    fn new_file(&mut self) {
        if !self.request_or_run(PendingAction::New) {
            return;
        }
        self.do_new_file();
    }

    fn do_new_file(&mut self) {
        self.editor.new_document();
        self.clear_recovery();
        self.file_path = None;
        self.preferred_caret_x = None;
        self.set_status("New document");
    }

    fn open_file(&mut self) {
        if !self.request_or_run(PendingAction::Open) {
            return;
        }
        self.do_open_file();
    }

    fn do_open_file(&mut self) {
        let path = rfd::FileDialog::new()
            .add_filter("OpenDocument Text", &["odt"])
            .add_filter("Word Document", &["docx"])
            .add_filter("rust-office", &["json", "roffice.json"])
            .add_filter("JSON", &["json"])
            .pick_file();
        let Some(path) = path else {
            return;
        };
        self.open_path(&path);
    }

    fn open_path(&mut self, path: &std::path::Path) {
        match load_document(path) {
            Ok(doc) => {
                self.editor.replace_document(doc);
                self.clear_recovery();
                self.file_path = Some(path.to_path_buf());
                self.preferred_caret_x = None;
                self.set_status("Opened");
            }
            Err(e) => self.set_error(format!("Open failed: {e}")),
        }
    }

    fn insert_image(&mut self) {
        let path = rfd::FileDialog::new()
            .add_filter("Images", &["png", "jpg", "jpeg", "gif", "webp", "bmp"])
            .pick_file();
        let Some(path) = path else {
            return;
        };
        match image_from_path_fitted(&path, 360.0) {
            Ok(image) => match self.editor.insert_image(image) {
                Ok(()) => self.set_status("Image inserted"),
                Err(e) => self.set_error(format!("Insert image failed: {e}")),
            },
            Err(e) => self.set_error(format!("Could not load image: {e}")),
        }
    }

    fn insert_hyperlink(&mut self) {
        if self.editor.selection().is_collapsed() {
            self.set_status("Select text to add a hyperlink");
            return;
        }
        let selected = self.editor.selected_text();
        let guess = if selected.starts_with("http://") || selected.starts_with("https://") {
            selected
        } else if selected.is_empty() {
            "https://".into()
        } else {
            format!("https://{selected}")
        };
        self.link_dialog = Some(guess);
    }

    fn show_link_dialog(&mut self, ctx: &Context) {
        let Some(draft) = self.link_dialog.as_mut() else {
            return;
        };
        let mut apply = false;
        let mut clear_link = false;
        let mut cancel = false;
        egui::Modal::new(egui::Id::new("hyperlink_dialog")).show(ctx, |ui| {
            ui.heading("Hyperlink");
            ui.label("URL");
            ui.add(
                egui::TextEdit::singleline(draft)
                    .desired_width(360.0)
                    .hint_text("https://…"),
            );
            ui.horizontal(|ui| {
                if ui.button("OK").clicked() {
                    apply = true;
                }
                if ui.button("Remove Link").clicked() {
                    clear_link = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
        if apply {
            let url = self.link_dialog.take().unwrap_or_default();
            let url = url.trim().to_string();
            if url.is_empty() {
                self.set_status("Hyperlink cancelled (empty URL)");
            } else if let Err(e) = self.editor.set_hyperlink(Some(url)) {
                self.set_error(format!("Hyperlink failed: {e}"));
            } else {
                self.set_status("Hyperlink applied");
            }
        } else if clear_link {
            self.link_dialog = None;
            let _ = self.editor.set_hyperlink(None);
            self.set_status("Hyperlink removed");
        } else if cancel {
            self.link_dialog = None;
        }
    }

    fn open_hyperlink_url(&mut self, url: &str) {
        match open::that(url) {
            Ok(()) => self.set_status(format!("Opened {url}")),
            Err(e) => self.set_error(format!("Could not open link: {e}")),
        }
    }

    fn open_header_footer_dialog(&mut self) {
        self.header_footer_dialog = Some(crate::page_setup::PageSetupDraft::new(
            self.editor.document(),
            self.editor.current_section(),
        ));
    }

    fn show_header_footer_dialog(&mut self, ctx: &Context) {
        let Some(draft) = self.header_footer_dialog.as_mut() else {
            return;
        };
        let (apply, cancel) = draft.show(ctx, self.editor.document());
        if apply {
            let draft = self.header_footer_dialog.take().unwrap();
            let (header, footer) = draft.margins(self.editor.document());
            match self
                .editor
                .set_section_settings(draft.section, draft.style, header, footer)
            {
                Ok(()) => self.set_status("Section settings updated"),
                Err(error) => self.set_error(error.to_string()),
            }
        } else if cancel {
            self.header_footer_dialog = None;
        }
    }

    fn prune_image_textures(&mut self) {
        use office_core::Block;
        let live: std::collections::HashSet<String> = self
            .editor
            .document()
            .sections
            .iter()
            .flat_map(|section| &section.blocks)
            .filter_map(|b| match b {
                Block::Image(img) => Some(img.cache_key()),
                _ => None,
            })
            .collect();
        self.image_textures.retain(|k, _| live.contains(k));
    }

    fn request_quit(&mut self, _ctx: &Context) {
        // OfficeApp checks every open document, including inactive apps.
        self.pending_switch = Some(SwitchTo::Quit);
    }

    /// Returns `true` if the action may proceed immediately.
    fn request_or_run(&mut self, action: PendingAction) -> bool {
        if self.editor.is_dirty() {
            self.pending_action = Some(action);
            false
        } else {
            true
        }
    }

    fn finish_pending_after_discard(&mut self) {
        let Some(action) = self.pending_action.take() else {
            return;
        };
        match action {
            PendingAction::New => self.do_new_file(),
            PendingAction::Open => self.do_open_file(),
        }
    }

    fn show_unsaved_dialog(&mut self, ctx: &Context) {
        if self.pending_action.is_none() {
            return;
        }

        if let Some(choice) = unsaved::confirm(ctx, "writer_unsaved", &self.title()) {
            self.resolve_pending_action(choice);
        }
    }

    fn resolve_pending_action(&mut self, choice: Choice) {
        match choice {
            Choice::Save => {
                if self.save_file() {
                    self.finish_pending_after_discard();
                } else {
                    self.pending_action = None;
                }
            }
            Choice::Discard => self.finish_pending_after_discard(),
            Choice::Cancel => self.pending_action = None,
        }
    }

    pub(super) fn is_dirty(&self) -> bool {
        self.editor.is_dirty()
    }

    pub(super) fn cancel_pending_action(&mut self) {
        self.pending_action = None;
    }

    pub(super) fn save_file(&mut self) -> bool {
        if self.file_path.is_none() {
            return self.save_file_as();
        }
        let path = self.file_path.clone().unwrap();
        match save_document(self.editor.document(), &path) {
            Ok(()) => {
                self.editor.mark_clean();
                self.clear_recovery();
                self.set_status(format!("Saved {}", path.display()));
                true
            }
            Err(e) => {
                self.set_error(format!("Save failed: {e}"));
                false
            }
        }
    }

    fn save_file_as(&mut self) -> bool {
        let path = rfd::FileDialog::new()
            .add_filter("OpenDocument Text", &["odt"])
            .add_filter("Word Document", &["docx"])
            .add_filter("rust-office", &["roffice.json", "json"])
            .set_file_name("document.odt")
            .save_file();
        let Some(path) = path else {
            return false;
        };
        match save_document(self.editor.document(), &path) {
            Ok(()) => {
                self.editor.mark_clean();
                self.clear_recovery();
                self.file_path = Some(path.clone());
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
        let default_name = self
            .file_path
            .as_ref()
            .and_then(|p| p.file_stem())
            .map(|s| format!("{}.pdf", s.to_string_lossy()))
            .unwrap_or_else(|| "document.pdf".into());
        let path = rfd::FileDialog::new()
            .add_filter("PDF", &["pdf"])
            .set_file_name(&default_name)
            .save_file();
        let Some(path) = path else {
            return;
        };
        match write_document_layout_pdf_path(self.editor.document(), &path) {
            Ok(()) => self.set_status(format!("Exported PDF {}", path.display())),
            Err(e) => self.set_error(format!("PDF export failed: {e}")),
        }
    }

    fn open_print_dialog(&mut self, layout_pages: usize) {
        self.print_dialog = Some(PrintDialog {
            layout_pages: layout_pages.max(1),
        });
    }

    fn run_print(&mut self, preview_only: bool) {
        let stem = self
            .file_path
            .as_ref()
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "document".into());
        match print_sys::write_temp_pdf(self.editor.document(), &stem) {
            Ok(path) => {
                if preview_only {
                    match print_sys::open_pdf_preview(&path) {
                        Ok(()) => self.set_status(format!("Opened preview {}", path.display())),
                        Err(e) => self.set_error(format!("Preview failed: {e}")),
                    }
                } else {
                    match print_sys::send_pdf_to_printer(&path) {
                        Ok(PrintAction::Spooled) => {
                            self.set_status(format!("Sent to printer ({})", path.display()));
                        }
                        Ok(PrintAction::PreviewFallback) => {
                            self.set_status(
                                "No system printer command — opened PDF for manual print",
                            );
                        }
                        Err(e) => self.set_error(format!("Print failed: {e}")),
                    }
                }
            }
            Err(e) => self.set_error(format!("Print PDF failed: {e}")),
        }
        self.print_dialog = None;
    }

    fn show_print_dialog(&mut self, ctx: &Context) {
        let Some(dialog) = self.print_dialog.clone() else {
            return;
        };
        let mut open = true;
        let mut do_print = false;
        let mut do_preview = false;
        let mut cancel = false;
        egui::Window::new("Print")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(format!("Layout pages (on screen): {}", dialog.layout_pages));
                ui.label("Print uses layout-faithful PDF (same pages as the canvas).");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Print").clicked() {
                        do_print = true;
                    }
                    if ui.button("Preview PDF").clicked() {
                        do_preview = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        if !open || cancel {
            self.print_dialog = None;
        } else if do_print {
            self.run_print(false);
        } else if do_preview {
            self.run_print(true);
        }
    }

    fn clipboard_copy(&mut self) {
        let text = self.editor.copy_text();
        if text.is_empty() {
            return;
        }
        if let Some(cb) = self.clipboard.as_mut() {
            if let Err(e) = cb.set_text(text) {
                self.set_error(format!("Clipboard copy failed: {e}"));
            } else {
                self.set_status("Copied");
            }
        }
    }

    fn clipboard_cut(&mut self) {
        match self.editor.cut() {
            Ok(text) if !text.is_empty() => {
                if let Some(cb) = self.clipboard.as_mut() {
                    let _ = cb.set_text(text);
                }
                self.set_status("Cut");
            }
            Ok(_) => {}
            Err(e) => self.set_error(format!("Cut failed: {e}")),
        }
    }

    fn clipboard_paste(&mut self) {
        let text = self
            .clipboard
            .as_mut()
            .and_then(|cb| cb.get_text().ok())
            .unwrap_or_default();
        if text.is_empty() {
            return;
        }
        if let Err(e) = self.editor.paste_text(&text) {
            self.set_error(format!("Paste failed: {e}"));
        } else {
            self.set_status("Pasted");
            self.caret_blink_start = Instant::now();
        }
    }

    fn handle_shortcuts(&mut self, ctx: &Context) {
        let macros = [
            (KeyboardShortcut::new(Modifiers::COMMAND, Key::F), "find"),
            (KeyboardShortcut::new(Modifiers::COMMAND, Key::H), "find"),
            (KeyboardShortcut::new(Modifiers::COMMAND, Key::N), "new"),
            (KeyboardShortcut::new(Modifiers::COMMAND, Key::O), "open"),
            (KeyboardShortcut::new(Modifiers::COMMAND, Key::S), "save"),
            (KeyboardShortcut::new(Modifiers::COMMAND, Key::Z), "undo"),
            (
                KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z),
                "redo",
            ),
            (KeyboardShortcut::new(Modifiers::COMMAND, Key::Y), "redo"),
            (
                KeyboardShortcut::new(Modifiers::COMMAND, Key::A),
                "select_all",
            ),
            (KeyboardShortcut::new(Modifiers::COMMAND, Key::B), "bold"),
            (KeyboardShortcut::new(Modifiers::COMMAND, Key::I), "italic"),
            (
                KeyboardShortcut::new(Modifiers::COMMAND, Key::U),
                "underline",
            ),
            (KeyboardShortcut::new(Modifiers::COMMAND, Key::C), "copy"),
            (KeyboardShortcut::new(Modifiers::COMMAND, Key::X), "cut"),
            (KeyboardShortcut::new(Modifiers::COMMAND, Key::V), "paste"),
            (
                KeyboardShortcut::new(Modifiers::COMMAND, Key::Equals),
                "zoom_in",
            ),
            (
                KeyboardShortcut::new(Modifiers::COMMAND, Key::Minus),
                "zoom_out",
            ),
            (
                KeyboardShortcut::new(Modifiers::COMMAND, Key::Num0),
                "zoom_reset",
            ),
            (
                KeyboardShortcut::new(Modifiers::COMMAND, Key::Enter),
                "page_break",
            ),
            (KeyboardShortcut::new(Modifiers::COMMAND, Key::P), "print"),
        ];

        for (shortcut, action) in macros {
            if ctx.input_mut(|i| i.consume_shortcut(&shortcut)) {
                match action {
                    "find" => self.open_find(),
                    "new" => self.new_file(),
                    "open" => self.open_file(),
                    "save" => {
                        self.save_file();
                    }
                    "undo" => {
                        if self.editor.undo() {
                            self.set_status("Undo");
                        }
                    }
                    "redo" => {
                        if self.editor.redo() {
                            self.set_status("Redo");
                        }
                    }
                    "select_all" => self.editor.select_all(),
                    "bold" => self.editor.toggle_bold(),
                    "italic" => self.editor.toggle_italic(),
                    "underline" => self.editor.toggle_underline(),
                    "copy" => self.clipboard_copy(),
                    "cut" => self.clipboard_cut(),
                    "paste" => self.clipboard_paste(),
                    "zoom_in" => self.zoom = (self.zoom + 0.1).clamp(0.5, 3.0),
                    "zoom_out" => self.zoom = (self.zoom - 0.1).clamp(0.5, 3.0),
                    "zoom_reset" => self.zoom = 1.0,
                    "page_break" => {
                        if let Err(e) = self.editor.insert_page_break() {
                            self.set_error(format!("Page break failed: {e}"));
                        } else {
                            self.set_status("Page break inserted");
                        }
                    }
                    "print" => {
                        let pages = self
                            .last_layout
                            .as_ref()
                            .map(|l| l.pages.len())
                            .unwrap_or(1);
                        self.open_print_dialog(pages);
                    }
                    _ => {}
                }
            }
        }
    }

    fn open_find(&mut self) {
        self.find_open = true;
        self.find_focus_requested = true;
        self.find_message.clear();
        self.ime_preedit.clear();
        self.ime_active_range = None;
        self.dragging = false;
    }

    fn find_body(&mut self, forward: bool) {
        if let Some(selected) = self.editor.find_body(&self.find_query, forward) {
            let matches = self.editor.body_matches(&self.find_query);
            let index = matches
                .iter()
                .position(|found| *found == selected)
                .unwrap_or(0);
            self.find_message = format!("Match {} of {}", index + 1, matches.len());
            self.preferred_caret_x = None;
            self.find_scroll_pending = true;
        } else {
            self.find_message = if self.find_query.is_empty() {
                "Enter text to find".into()
            } else {
                "No matches in body paragraphs".into()
            };
        }
    }

    fn replace_found(&mut self) {
        match self
            .editor
            .replace_body_match(&self.find_query, &self.find_replacement)
        {
            Ok(true) => {
                self.find_body(true);
                self.find_message = format!("Replaced 1. {}", self.find_message);
                self.find_scroll_pending = true;
            }
            Ok(false) => {
                if self.find_query == self.find_replacement && !self.find_query.is_empty() {
                    self.find_message = "No changes: replacement is identical".into();
                } else {
                    self.find_body(true);
                }
            }
            Err(error) => self.find_message = error.to_string(),
        }
    }

    fn replace_all_found(&mut self) {
        match self
            .editor
            .replace_all_body(&self.find_query, &self.find_replacement)
        {
            Ok(count) => {
                self.find_message = format!("Replaced {count} matches in body paragraphs");
                if count > 0 {
                    self.preferred_caret_x = None;
                    self.find_scroll_pending = true;
                }
            }
            Err(error) => self.find_message = error.to_string(),
        }
    }

    fn show_find_dialog(&mut self, ctx: &Context) {
        if !self.find_open {
            return;
        }
        let mut close = false;
        let mut next = false;
        let mut previous = false;
        let mut replace = false;
        let mut replace_all = false;
        egui::Modal::new(egui::Id::new("find_replace_dialog")).show(ctx, |ui| {
            ui.set_min_width(420.0);
            ui.heading("Find and Replace");
            ui.label("Body paragraphs in all sections · Exact case · Literal text");
            ui.label("Tables, headers and footers are excluded. No paragraph breaks.");
            ui.horizontal(|ui| {
                ui.label("Find");
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.find_query)
                        .id(egui::Id::new("writer_find_query"))
                        .desired_width(335.0),
                );
                if self.find_focus_requested {
                    response.request_focus();
                    self.find_focus_requested = false;
                }
                if response.changed() {
                    self.find_message.clear();
                }
                if response.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter)) {
                    next = true;
                }
            });
            ui.horizontal(|ui| {
                ui.label("Replace");
                ui.add(
                    egui::TextEdit::singleline(&mut self.find_replacement)
                        .id(egui::Id::new("writer_find_replacement"))
                        .desired_width(315.0),
                );
            });
            ui.horizontal(|ui| {
                let ready = !self.find_query.is_empty();
                previous = ui
                    .add_enabled(ready, egui::Button::new("Previous"))
                    .clicked();
                next |= ui.add_enabled(ready, egui::Button::new("Next")).clicked();
                replace = ui
                    .add_enabled(ready, egui::Button::new("Replace"))
                    .clicked();
                replace_all = ui
                    .add_enabled(ready, egui::Button::new("Replace All"))
                    .clicked();
                close = ui.button("Close").clicked();
            });
            ui.label(&self.find_message);
            close |= ui.input(|input| input.key_pressed(Key::Escape));
        });
        if previous {
            self.find_body(false);
        }
        if next {
            self.find_body(true);
        }
        if replace {
            self.replace_found();
        }
        if replace_all {
            self.replace_all_found();
        }
        if close {
            self.find_open = false;
            // The modal still owns this frame's focus layer. Restore body focus
            // next frame, after that layer has gone away.
            self.find_restore_focus = true;
            ctx.request_repaint();
        }
    }

    fn restore_find_focus(&mut self, ctx: &Context) {
        if self.find_restore_focus && !self.find_open {
            if ctx.memory(|memory| memory.top_modal_layer().is_none()) {
                self.find_restore_focus = false;
                self.focus_document = true;
                ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("writer_document")));
            } else {
                ctx.request_repaint();
            }
        }
    }

    fn menu_bar(&mut self, ui: &mut Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("New\tCtrl+N").clicked() {
                    self.new_file();
                    ui.close();
                }
                if ui.button("Open…\tCtrl+O").clicked() {
                    self.open_file();
                    ui.close();
                }
                if ui.button("Save\tCtrl+S").clicked() {
                    self.save_file();
                    ui.close();
                }
                if ui.button("Save As…").clicked() {
                    self.save_file_as();
                    ui.close();
                }
                if ui.button("Recovery copies…").clicked() {
                    self.open_recovery();
                    ui.close();
                }
                if ui.button("Export PDF…").clicked() {
                    self.export_pdf();
                    ui.close();
                }
                if ui.button("Print…\tCtrl+P").clicked() {
                    let pages = self
                        .last_layout
                        .as_ref()
                        .map(|l| l.pages.len())
                        .unwrap_or(1);
                    self.open_print_dialog(pages);
                    ui.close();
                }
                ui.separator();
                if ui.button("New Spreadsheet (Calc)").clicked() {
                    self.pending_switch = Some(SwitchTo::Calc);
                    ui.close();
                }
                if ui.button("New Presentation (Impress)").clicked() {
                    self.pending_switch = Some(SwitchTo::Impress);
                    ui.close();
                }
                ui.separator();
                if ui.button("Quit").clicked() {
                    self.request_quit(ui.ctx());
                    ui.close();
                }
            });
            ui.menu_button("Edit", |ui| {
                if ui
                    .add_enabled(self.editor.can_undo(), egui::Button::new("Undo\tCtrl+Z"))
                    .clicked()
                {
                    self.editor.undo();
                    ui.close();
                }
                if ui
                    .add_enabled(self.editor.can_redo(), egui::Button::new("Redo\tCtrl+Y"))
                    .clicked()
                {
                    self.editor.redo();
                    ui.close();
                }
                ui.separator();
                if ui.button("Cut\tCtrl+X").clicked() {
                    self.clipboard_cut();
                    ui.close();
                }
                if ui.button("Copy\tCtrl+C").clicked() {
                    self.clipboard_copy();
                    ui.close();
                }
                if ui.button("Paste\tCtrl+V").clicked() {
                    self.clipboard_paste();
                    ui.close();
                }
                ui.separator();
                if ui.button("Select All\tCtrl+A").clicked() {
                    self.editor.select_all();
                    ui.close();
                }
                ui.separator();
                if ui.button("Find and Replace…\tCtrl+F / Ctrl+H").clicked() {
                    self.open_find();
                    ui.close();
                }
            });
            ui.menu_button("View", |ui| {
                if ui.button("Zoom In\tCtrl++").clicked() {
                    self.zoom = (self.zoom + 0.1).clamp(0.5, 3.0);
                    ui.close();
                }
                if ui.button("Zoom Out\tCtrl+-").clicked() {
                    self.zoom = (self.zoom - 0.1).clamp(0.5, 3.0);
                    ui.close();
                }
                if ui.button("Reset Zoom\tCtrl+0").clicked() {
                    self.zoom = 1.0;
                    ui.close();
                }
            });
            ui.menu_button("Insert", |ui| {
                if ui.button("Table 2×2").clicked() {
                    let _ = self.editor.insert_table(2, 2);
                    ui.close();
                }
                if ui.button("Table 3×3").clicked() {
                    let _ = self.editor.insert_table(3, 3);
                    ui.close();
                }
                ui.separator();
                if ui.button("Image…").clicked() {
                    self.insert_image();
                    ui.close();
                }
                if ui.button("Hyperlink…").clicked() {
                    self.insert_hyperlink();
                    ui.close();
                }
                ui.separator();
                if ui.button("Page Break\tCtrl+Enter").clicked() {
                    if let Err(e) = self.editor.insert_page_break() {
                        self.set_error(format!("Page break failed: {e}"));
                    } else {
                        self.set_status("Page break inserted");
                    }
                    ui.close();
                }
                if ui.button("Section Break").clicked() {
                    if let Err(e) = self.editor.insert_section_break() {
                        self.set_error(format!("Section break failed: {e}"));
                    } else {
                        self.set_status("Section break inserted");
                    }
                    ui.close();
                }
                if ui.button("Page / Section Setup…").clicked() {
                    self.open_header_footer_dialog();
                    ui.close();
                }
            });
            ui.menu_button("Format", |ui| {
                ui.menu_button("Paragraph Style", |ui| {
                    for &s in NamedParagraphStyle::ALL {
                        if ui.button(s.display_name()).clicked() {
                            let _ = self.editor.set_named_paragraph_style(s);
                            ui.close();
                        }
                    }
                });
                ui.separator();
                if ui.button("Bold\tCtrl+B").clicked() {
                    self.editor.toggle_bold();
                    ui.close();
                }
                if ui.button("Italic\tCtrl+I").clicked() {
                    self.editor.toggle_italic();
                    ui.close();
                }
                if ui.button("Underline\tCtrl+U").clicked() {
                    self.editor.toggle_underline();
                    ui.close();
                }
                ui.separator();
                if ui.button("Bulleted List").clicked() {
                    let _ = self.editor.toggle_bullet_list();
                    ui.close();
                }
                if ui.button("Numbered List").clicked() {
                    let _ = self.editor.toggle_numbered_list();
                    ui.close();
                }
                if ui.button("Increase Indent\tTab").clicked() {
                    let _ = self.editor.indent_list();
                    ui.close();
                }
                if ui.button("Decrease Indent\tShift+Tab").clicked() {
                    let _ = self.editor.outdent_list();
                    ui.close();
                }
                ui.separator();
                if ui.button("Align Left").clicked() {
                    let _ = self.editor.set_alignment(Alignment::Left);
                    ui.close();
                }
                if ui.button("Align Center").clicked() {
                    let _ = self.editor.set_alignment(Alignment::Center);
                    ui.close();
                }
                if ui.button("Align Right").clicked() {
                    let _ = self.editor.set_alignment(Alignment::Right);
                    ui.close();
                }
            });
            ui.menu_button("Tools", |ui| {
                ui.label(RichText::new("Spell check — future").weak());
            });
            ui.menu_button("Help", |ui| {
                ui.label("rust-office v0.1 — Writer");
                ui.label("A modern office suite written in Rust.");
            });
        });
    }

    fn toolbar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;

            toolbar_btn(ui, "New", || self.new_file());
            toolbar_btn(ui, "Open", || self.open_file());
            toolbar_btn(ui, "Save", || {
                self.save_file();
            });
            ui.separator();
            if ui
                .add_enabled(self.editor.can_undo(), egui::Button::new("Undo"))
                .clicked()
            {
                self.editor.undo();
            }
            if ui
                .add_enabled(self.editor.can_redo(), egui::Button::new("Redo"))
                .clicked()
            {
                self.editor.redo();
            }
            ui.separator();

            let style = self.editor.typing_style().clone();
            toggle_btn(ui, "B", style.bold, RichText::new("B").strong(), || {
                self.editor.toggle_bold()
            });
            toggle_btn(ui, "I", style.italic, RichText::new("I").italics(), || {
                self.editor.toggle_italic()
            });
            toggle_btn(
                ui,
                "U",
                style.underline,
                RichText::new("U").underline(),
                || self.editor.toggle_underline(),
            );
            ui.separator();

            let align = match self.editor.edit_focus() {
                EditFocus::Cell(addr) => self
                    .editor
                    .document()
                    .cell_paragraph(addr)
                    .map(|p| p.style.alignment)
                    .unwrap_or_default(),
                EditFocus::Header(section) => self
                    .editor
                    .document()
                    .section_header(section)
                    .map(|p| p.style.alignment)
                    .unwrap_or_default(),
                EditFocus::Footer(section) => self
                    .editor
                    .document()
                    .section_footer(section)
                    .map(|p| p.style.alignment)
                    .unwrap_or_default(),
                EditFocus::Body => self
                    .editor
                    .document()
                    .paragraph(self.editor.selection().focus.paragraph)
                    .map(|p| p.style.alignment)
                    .unwrap_or_default(),
            };
            toggle_btn(
                ui,
                "Left",
                align == Alignment::Left,
                RichText::new("⬅"),
                || {
                    let _ = self.editor.set_alignment(Alignment::Left);
                },
            );
            toggle_btn(
                ui,
                "Center",
                align == Alignment::Center,
                RichText::new("☰"),
                || {
                    let _ = self.editor.set_alignment(Alignment::Center);
                },
            );
            toggle_btn(
                ui,
                "Right",
                align == Alignment::Right,
                RichText::new("➡"),
                || {
                    let _ = self.editor.set_alignment(Alignment::Right);
                },
            );
            ui.separator();

            ui.separator();

            let mut named = self.editor.current_named_paragraph_style();
            egui::ComboBox::from_id_salt("para_style")
                .selected_text(named.display_name())
                .width(100.0)
                .show_ui(ui, |ui| {
                    for &s in NamedParagraphStyle::ALL {
                        ui.selectable_value(&mut named, s, s.display_name());
                    }
                });
            if named != self.editor.current_named_paragraph_style() {
                let _ = self.editor.set_named_paragraph_style(named);
            }

            ui.label("Size");
            let mut size = style.font_size;
            egui::ComboBox::from_id_salt("font_size")
                .selected_text(format!("{size}"))
                .width(56.0)
                .show_ui(ui, |ui| {
                    for &s in FONT_SIZES {
                        ui.selectable_value(&mut size, s, format!("{s}"));
                    }
                });
            if (size - style.font_size).abs() > f32::EPSILON {
                self.editor.set_font_size(size);
            }

            ui.separator();
            ui.label("Zoom");
            if ui.button("−").clicked() {
                self.zoom = (self.zoom - 0.1).clamp(0.5, 3.0);
            }
            ui.label(format!("{}%", (self.zoom * 100.0).round() as i32));
            if ui.button("+").clicked() {
                self.zoom = (self.zoom + 0.1).clamp(0.5, 3.0);
            }
        });
    }

    fn status_bar(&self, ui: &mut Ui) {
        let page_count = self
            .last_layout
            .as_ref()
            .map(DocumentLayout::page_count)
            .unwrap_or(1);
        let current_page = self
            .last_layout
            .as_ref()
            .map(|l| l.page_of(self.editor.selection().focus))
            .unwrap_or(1);
        ui.horizontal(|ui| {
            ui.label(format!("Page {current_page} of {page_count}"));
            ui.separator();
            match self.editor.edit_focus() {
                EditFocus::Cell(addr) => {
                    ui.label(format!("Cell R{}C{}", addr.row + 1, addr.col + 1));
                    ui.separator();
                }
                EditFocus::Header(section) => {
                    ui.label(format!("Header — Section {}", section + 1));
                    ui.separator();
                }
                EditFocus::Footer(section) => {
                    ui.label(format!("Footer — Section {}", section + 1));
                    ui.separator();
                }
                EditFocus::Body => {}
            }
            ui.label(format!("{} words", self.editor.document().word_count()));
            ui.separator();
            ui.label(format!("{}%", (self.zoom * 100.0).round() as i32));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let msg = if let Some(err) = &self.last_error {
                    RichText::new(err).color(Color32::from_rgb(160, 40, 40))
                } else {
                    RichText::new(&self.status_message).color(Color32::from_gray(80))
                };
                ui.label(msg);
            });
        });
    }

    fn document_canvas(&mut self, ui: &mut Ui) {
        let zoom = self.zoom;
        let max_width = self
            .editor
            .document()
            .sections
            .iter()
            .map(|s| s.page_style.width)
            .fold(0.0_f32, f32::max)
            * zoom;
        let margin = 32.0;
        let doc_id = egui::Id::new("writer_document");

        ScrollArea::both()
            .auto_shrink([false, false])
            .id_salt("doc_scroll")
            .show(ui, |ui| {
                // Probe layout to learn page count / canvas height, then place for real.
                let probe = layout_document(ui, self.editor.document(), Pos2::ZERO, zoom);
                let canvas = document_canvas_size(&probe);
                let desired = Vec2::new(
                    (max_width + margin * 2.0).max(ui.available_width()),
                    canvas.y + margin * 2.0,
                );
                let (response, painter) = ui.allocate_painter(desired, Sense::click_and_drag());
                let response = ui.interact(response.rect, doc_id, Sense::click_and_drag());

                painter.rect_filled(response.rect, 0.0, CANVAS_BG);

                let origin = Pos2::new(
                    response.rect.left()
                        + ((response.rect.width() - max_width) * 0.5).max(margin),
                    response.rect.top() + margin,
                );

                let layout = layout_document(ui, self.editor.document(), origin, zoom);
                if self.find_scroll_pending {
                    if let Some(rect) = layout.caret_rect(self.editor.selection().focus) {
                        ui.scroll_to_rect(rect, Some(egui::Align::Center));
                    }
                    self.find_scroll_pending = false;
                }
                ensure_image_textures(ui.ctx(), self.editor.document(), &mut self.image_textures);
                self.prune_image_textures();
                let texture_ids: HashMap<String, egui::TextureId> = self
                    .image_textures
                    .iter()
                    .map(|(k, h)| (k.clone(), h.id()))
                    .collect();

                if response.clicked() || response.drag_started() {
                    response.request_focus();
                    self.focus_document = true;
                    self.preferred_caret_x = None;
                    ui.memory_mut(|m| {
                        m.set_focus_lock_filter(
                            doc_id,
                            egui::EventFilter {
                                tab: true,
                                horizontal_arrows: true,
                                vertical_arrows: true,
                                escape: false,
                            },
                        );
                    });
                }

                if response.clicked_by(PointerButton::Primary)
                    || response.drag_started_by(PointerButton::Primary)
                {
                    if let Some(pos) = response.interact_pointer_pos() {
                        let ctrl_click = ui.input(|i| i.modifiers.command || i.modifiers.ctrl);
                        let hit = layout.hit_test_full(pos);
                        let url_to_open = if ctrl_click {
                            match hit {
                                HitResult::Body(doc_pos) => {
                                    self.editor.link_at(doc_pos).map(str::to_string)
                                }
                                _ => None,
                            }
                        } else {
                            None
                        };
                        if let Some(url) = url_to_open {
                            self.open_hyperlink_url(&url);
                        } else {
                            let extend = ui.input(|i| i.modifiers.shift);
                            let hit_section = layout.pages.iter().find(|p| p.page_rect.contains(pos))
                                .map(|p| p.section_index).unwrap_or(self.editor.current_section());
                            match hit {
                                HitResult::Cell { address, offset } => {
                                    if extend
                                        && matches!(
                                            self.editor.edit_focus(),
                                            EditFocus::Cell(a) if a == address
                                        )
                                    {
                                        let sel = self
                                            .editor
                                            .selection()
                                            .with_focus(office_core::DocPosition::new(0, offset));
                                        self.editor.set_selection(sel);
                                    } else {
                                        let _ = self.editor.focus_cell(address, offset);
                                    }
                                }
                                HitResult::Header(doc_pos) => {
                                    self.margin_page = layout.pages.iter().find(|p| p.page_rect.contains(pos)).map(|p| p.page_number).unwrap_or(0);
                                    if extend
                                        && matches!(self.editor.edit_focus(), EditFocus::Header(s) if s == hit_section)
                                    {
                                        let sel = self.editor.selection().with_focus(doc_pos);
                                        self.editor.set_selection(sel);
                                    } else {
                                        let _ = self.editor.focus_section_header(hit_section, doc_pos.offset);
                                    }
                                }
                                HitResult::Footer(doc_pos) => {
                                    self.margin_page = layout.pages.iter().find(|p| p.page_rect.contains(pos)).map(|p| p.page_number).unwrap_or(0);
                                    if extend
                                        && matches!(self.editor.edit_focus(), EditFocus::Footer(s) if s == hit_section)
                                    {
                                        let sel = self.editor.selection().with_focus(doc_pos);
                                        self.editor.set_selection(sel);
                                    } else {
                                        let _ = self.editor.focus_section_footer(hit_section, doc_pos.offset);
                                    }
                                }
                                HitResult::Body(doc_pos) => {
                                    self.editor.focus_body();
                                    if extend {
                                        let sel = self.editor.selection().with_focus(doc_pos);
                                        self.editor.set_selection(sel);
                                    } else {
                                        self.editor.set_selection(Selection::caret(doc_pos));
                                    }
                                }
                            }
                            self.dragging = true;
                            self.preferred_caret_x = None;
                            self.caret_blink_start = Instant::now();
                        }
                    }
                }
                if self.dragging {
                    if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
                        if response.rect.contains(pos) || response.dragged() {
                            let hit_section = layout.pages.iter().find(|p| p.page_rect.contains(pos))
                                .map(|p| p.section_index).unwrap_or(self.editor.current_section());
                            match layout.hit_test_full(pos) {
                                HitResult::Cell { address, offset }
                                    if matches!(
                                        self.editor.edit_focus(),
                                        EditFocus::Cell(a) if a == address
                                    ) =>
                                {
                                    let sel = self
                                        .editor
                                        .selection()
                                        .with_focus(office_core::DocPosition::new(0, offset));
                                    self.editor.set_selection(sel);
                                }
                                HitResult::Header(doc_pos)
                                    if matches!(self.editor.edit_focus(), EditFocus::Header(s) if s == hit_section) =>
                                {
                                    let sel = self.editor.selection().with_focus(doc_pos);
                                    self.editor.set_selection(sel);
                                }
                                HitResult::Footer(doc_pos)
                                    if matches!(self.editor.edit_focus(), EditFocus::Footer(s) if s == hit_section) =>
                                {
                                    let sel = self.editor.selection().with_focus(doc_pos);
                                    self.editor.set_selection(sel);
                                }
                                HitResult::Body(doc_pos) if self.editor.edit_focus().is_body() => {
                                    let sel = self.editor.selection().with_focus(doc_pos);
                                    self.editor.set_selection(sel);
                                }
                                _ => {}
                            }
                        }
                    }
                    if ui.input(|i| i.pointer.any_released()) {
                        self.dragging = false;
                    }
                }

                response.context_menu(|ui| {
                    if ui.button("Cut").clicked() {
                        self.clipboard_cut();
                        ui.close();
                    }
                    if ui.button("Copy").clicked() {
                        self.clipboard_copy();
                        ui.close();
                    }
                    if ui.button("Paste").clicked() {
                        self.clipboard_paste();
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Select All").clicked() {
                        self.editor.select_all();
                        ui.close();
                    }
                });

                self.focus_document = ui.memory(|m| m.has_focus(doc_id));

                let blink_on = self.caret_blink_start.elapsed().as_millis() % 1000 < 500;
                let show_caret = self.focus_document
                    && self.editor.selection().is_collapsed()
                    && blink_on
                    && self.ime_preedit.is_empty();

                paint_document(
                    ui,
                    &layout,
                    self.editor.selection(),
                    self.editor.selection().focus,
                    show_caret,
                    self.editor.edit_focus(),
                    &texture_ids,
                );

                if !self.ime_preedit.is_empty() {
                    let caret_opt = match self.editor.edit_focus() {
                        EditFocus::Body => layout.caret_rect(self.editor.selection().focus),
                        EditFocus::Cell(addr) => {
                            layout.cell_caret_rect(addr, self.editor.selection().focus)
                        }
                        EditFocus::Header(_) | EditFocus::Footer(_) => layout.margin_caret_rect(
                            self.editor.edit_focus(),
                            self.editor.selection().focus,
                            layout.pages.iter().find(|p| p.page_number == self.margin_page && p.section_index == self.editor.current_section())
                                .map(|p| p.page_number).unwrap_or(0),
                        ),
                    };
                    if let Some(caret) = caret_opt {
                        let pre_rect = paint_ime_preedit(
                            ui,
                            caret.min,
                            &self.ime_preedit,
                            self.ime_active_range.clone(),
                            self.editor.typing_style().font_size * zoom,
                        );
                        // Candidate window should track the composition, not just the caret.
                        if self.focus_document {
                            ui.ctx().output_mut(|o| {
                                o.ime = Some(egui::output::IMEOutput {
                                    purpose: egui::IMEPurpose::Normal,
                                    rect: pre_rect,
                                    cursor_rect: pre_rect,
                                    should_interrupt_composition: false,
                                });
                            });
                        }
                    }
                } else if self.focus_document {
                    let ime_rect = match self.editor.edit_focus() {
                        EditFocus::Body => {
                            ime_caret_screen_rect(&layout, self.editor.selection().focus)
                        }
                        EditFocus::Cell(addr) => {
                            layout.cell_caret_rect(addr, self.editor.selection().focus)
                        }
                        EditFocus::Header(_) | EditFocus::Footer(_) => layout.margin_caret_rect(
                            self.editor.edit_focus(),
                            self.editor.selection().focus,
                            layout.pages.iter().find(|p| p.page_number == self.margin_page && p.section_index == self.editor.current_section())
                                .map(|p| p.page_number).unwrap_or(0),
                        ),
                    };
                    if let Some(rect) = ime_rect {
                        ui.ctx().output_mut(|o| {
                            o.ime = Some(egui::output::IMEOutput {
                                purpose: egui::IMEPurpose::Normal,
                                rect,
                                cursor_rect: rect,
                                should_interrupt_composition: false,
                            });
                        });
                    }
                }

                let _ = painter;
                self.last_layout = Some(layout);
            });
    }

    fn apply_caret_move(&mut self, new_pos: office_core::DocPosition, extend: bool) {
        if extend {
            let sel = self.editor.selection().with_focus(new_pos);
            self.editor.set_selection(sel);
        } else {
            self.editor.set_selection(Selection::caret(new_pos));
        }
        self.caret_blink_start = Instant::now();
    }

    fn move_vertically_visual(&mut self, direction: VerticalDir, extend: bool) {
        if matches!(self.editor.edit_focus(), EditFocus::Cell(_)) {
            let forward = matches!(direction, VerticalDir::Down);
            let _ = self.editor.move_cell(forward);
            self.preferred_caret_x = None;
            self.caret_blink_start = Instant::now();
            return;
        }
        if self.editor.edit_focus().is_margin() {
            if matches!(direction, VerticalDir::Up) {
                self.editor.move_home(extend);
            } else {
                self.editor.move_end(extend);
            }
            self.preferred_caret_x = None;
            self.caret_blink_start = Instant::now();
            return;
        }
        let Some(layout) = self.last_layout.clone() else {
            match direction {
                VerticalDir::Up => self.editor.move_up(extend),
                VerticalDir::Down => self.editor.move_down(extend),
            }
            return;
        };
        let focus = self.editor.selection().focus;
        let preferred = self
            .preferred_caret_x
            .or_else(|| layout.caret_x(focus))
            .unwrap_or(0.0);
        self.preferred_caret_x = Some(preferred);
        let new_pos = layout.move_vertically(focus, preferred, direction);
        self.apply_caret_move(new_pos, extend);
    }

    fn move_line_home_end(&mut self, to_end: bool, extend: bool) {
        if self.editor.edit_focus().is_single_paragraph() {
            if to_end {
                self.editor.move_end(extend);
            } else {
                self.editor.move_home(extend);
            }
            self.preferred_caret_x = None;
            self.caret_blink_start = Instant::now();
            return;
        }
        let focus = self.editor.selection().focus;
        let new_pos = if let Some(layout) = self.last_layout.as_ref() {
            if to_end {
                layout.line_end(focus)
            } else {
                layout.line_home(focus)
            }
        } else if to_end {
            let len = self
                .editor
                .document()
                .paragraph(focus.paragraph)
                .map(|p| p.char_len())
                .unwrap_or(0);
            office_core::DocPosition::new(focus.paragraph, len)
        } else {
            office_core::DocPosition::new(focus.paragraph, 0)
        };
        self.preferred_caret_x = None;
        self.apply_caret_move(new_pos, extend);
    }

    fn handle_text_input(&mut self, ctx: &Context) {
        if !self.focus_document
            || self.pending_action.is_some()
            || self.link_dialog.is_some()
            || self.header_footer_dialog.is_some()
            || self.find_open
        {
            return;
        }

        let events: Vec<egui::Event> = ctx.input(|i| i.events.clone());
        for event in events {
            match event {
                egui::Event::Text(text) => {
                    // While composing, ignore Text (commit arrives via ImeEvent::Commit).
                    if !self.ime_preedit.is_empty() || text.is_empty() {
                        continue;
                    }
                    let _ = self.editor.insert_text(&text);
                    self.preferred_caret_x = None;
                    self.caret_blink_start = Instant::now();
                }
                egui::Event::Ime(ime) => match ime {
                    #[allow(deprecated)]
                    egui::ImeEvent::Enabled | egui::ImeEvent::Disabled => {
                        self.ime_preedit.clear();
                        self.ime_active_range = None;
                    }
                    egui::ImeEvent::Preedit {
                        text,
                        active_range_chars,
                    } => {
                        self.ime_preedit = text;
                        self.ime_active_range = active_range_chars;
                        self.caret_blink_start = Instant::now();
                    }
                    egui::ImeEvent::Commit(s) => {
                        self.ime_preedit.clear();
                        self.ime_active_range = None;
                        if !s.is_empty() {
                            let _ = self.editor.insert_text(&s);
                            self.preferred_caret_x = None;
                            self.caret_blink_start = Instant::now();
                        }
                    }
                    egui::ImeEvent::DeleteSurrounding {
                        before_chars,
                        after_chars,
                    } => {
                        for _ in 0..before_chars {
                            let _ = self.editor.delete_backward();
                        }
                        for _ in 0..after_chars {
                            let _ = self.editor.delete_forward();
                        }
                        self.preferred_caret_x = None;
                    }
                },
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    if modifiers.command || modifiers.ctrl || modifiers.mac_cmd {
                        continue;
                    }
                    if key == Key::Tab {
                        if modifiers.shift {
                            let _ = self.editor.outdent_list();
                        } else {
                            let _ = self.editor.indent_list();
                        }
                        self.preferred_caret_x = None;
                        self.caret_blink_start = Instant::now();
                        continue;
                    }
                    let extend = modifiers.shift;
                    match key {
                        Key::Backspace => {
                            let _ = self.editor.delete_backward();
                            self.preferred_caret_x = None;
                            self.caret_blink_start = Instant::now();
                        }
                        Key::Delete => {
                            let _ = self.editor.delete_forward();
                            self.preferred_caret_x = None;
                            self.caret_blink_start = Instant::now();
                        }
                        Key::Enter => {
                            let _ = self.editor.insert_text("\n");
                            self.preferred_caret_x = None;
                            self.caret_blink_start = Instant::now();
                        }
                        Key::Tab => {
                            let forward = !modifiers.shift;
                            let _ = self.editor.move_cell(forward);
                            self.preferred_caret_x = None;
                            self.caret_blink_start = Instant::now();
                        }
                        Key::ArrowLeft => {
                            self.preferred_caret_x = None;
                            self.editor.move_left(extend);
                            self.caret_blink_start = Instant::now();
                        }
                        Key::ArrowRight => {
                            self.preferred_caret_x = None;
                            self.editor.move_right(extend);
                            self.caret_blink_start = Instant::now();
                        }
                        Key::ArrowUp => self.move_vertically_visual(VerticalDir::Up, extend),
                        Key::ArrowDown => self.move_vertically_visual(VerticalDir::Down, extend),
                        Key::Home => self.move_line_home_end(false, extend),
                        Key::End => self.move_line_home_end(true, extend),
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }
}

/// Paint IME composition text with segmented underline + active-range highlight.
fn paint_ime_preedit(
    ui: &Ui,
    origin: Pos2,
    text: &str,
    active_range: Option<std::ops::Range<usize>>,
    font_size: f32,
) -> Rect {
    let galley = ui.fonts_mut(|f| {
        f.layout_no_wrap(
            text.to_string(),
            egui::FontId::proportional(font_size),
            Color32::BLACK,
        )
    });
    let pre_rect = Rect::from_min_size(origin, galley.size());
    let painter = ui.painter();
    painter.rect_filled(
        pre_rect,
        0.0,
        Color32::from_rgba_unmultiplied(255, 245, 180, 200),
    );
    painter.galley(origin, galley.clone(), Color32::BLACK);

    // Thin underline under the whole composition.
    let y = pre_rect.bottom() - 1.0;
    painter.line_segment(
        [
            Pos2::new(pre_rect.left(), y),
            Pos2::new(pre_rect.right(), y),
        ],
        egui::Stroke::new(1.0, Color32::from_gray(100)),
    );

    let char_count = text.chars().count();
    let range = active_range
        .filter(|r| r.start <= r.end && r.end <= char_count)
        .unwrap_or(0..char_count);
    let x0 = origin.x + preedit_char_x(&galley, range.start);
    let x1 = origin.x + preedit_char_x(&galley, range.end);
    if x1 > x0 {
        painter.line_segment(
            [Pos2::new(x0, y), Pos2::new(x1, y)],
            egui::Stroke::new(2.0, ACCENT),
        );
    }

    // Composition caret at the end of the active range.
    let caret_x = origin.x + preedit_char_x(&galley, range.end);
    painter.rect_filled(
        Rect::from_min_size(
            Pos2::new(caret_x, pre_rect.top()),
            Vec2::new(1.5, pre_rect.height().max(font_size)),
        ),
        0.0,
        Color32::from_rgb(20, 20, 20),
    );

    pre_rect
}

fn preedit_char_x(galley: &egui::Galley, char_idx: usize) -> f32 {
    let Some(row) = galley.rows.first() else {
        return 0.0;
    };
    if row.glyphs.is_empty() {
        return 0.0;
    }
    if char_idx == 0 {
        return row.glyphs[0].pos.x;
    }
    if char_idx >= row.glyphs.len() {
        let last = row.glyphs.last().unwrap();
        return last.pos.x + last.size().x;
    }
    row.glyphs[char_idx].pos.x
}

fn toolbar_btn(ui: &mut Ui, label: &str, mut on_click: impl FnMut()) {
    if ui.button(label).clicked() {
        on_click();
    }
}

fn toggle_btn(ui: &mut Ui, id: &str, selected: bool, label: RichText, mut on_click: impl FnMut()) {
    let mut btn = egui::Button::new(label);
    if selected {
        btn = btn.fill(Color32::from_rgb(200, 214, 232));
    }
    if ui.add(btn).on_hover_text(id).clicked() {
        on_click();
    }
}

impl App for WriterApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(self.title()));

        let recovery_was_open = self.recovery_open;
        self.show_recovery_dialog(&ctx);
        if recovery_was_open {
            ui.disable();
        }
        self.restore_find_focus(&ctx);

        // Focus the document canvas on first frame so typing works immediately.
        if self.focus_document && ctx.memory(|m| m.focused().is_none()) {
            ctx.memory_mut(|m| m.request_focus(egui::Id::new("writer_document")));
        }

        if ui.is_enabled()
            && self.pending_action.is_none()
            && self.link_dialog.is_none()
            && self.header_footer_dialog.is_none()
            && self.print_dialog.is_none()
            && !self.find_open
        {
            self.handle_shortcuts(&ctx);
            self.handle_text_input(&ctx);
        }
        self.show_unsaved_dialog(&ctx);
        if self.pending_action.is_some() {
            ui.disable();
        }
        self.show_link_dialog(&ctx);
        self.show_header_footer_dialog(&ctx);
        self.show_print_dialog(&ctx);
        self.show_find_dialog(&ctx);
        ctx.request_repaint_after(std::time::Duration::from_millis(500));

        egui::Panel::top("menu_panel")
            .frame(
                egui::Frame::new()
                    .fill(TOOLBAR_BG)
                    .inner_margin(egui::Margin::symmetric(6, 2))
                    .stroke(egui::Stroke::new(1.0, Color32::from_rgb(210, 214, 220))),
            )
            .show(ui, |ui| {
                self.menu_bar(ui);
            });

        egui::Panel::top("toolbar_panel")
            .frame(
                egui::Frame::new()
                    .fill(TOOLBAR_BG)
                    .inner_margin(egui::Margin::symmetric(8, 4))
                    .stroke(egui::Stroke::new(1.0, Color32::from_rgb(210, 214, 220))),
            )
            .show(ui, |ui| {
                self.toolbar(ui);
            });

        egui::Panel::bottom("status_panel")
            .exact_size(28.0)
            .frame(
                egui::Frame::new()
                    .fill(STATUS_BG)
                    .inner_margin(egui::Margin::symmetric(10, 4))
                    .stroke(egui::Stroke::new(1.0, Color32::from_rgb(210, 214, 220))),
            )
            .show(ui, |ui| {
                self.status_bar(ui);
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(CANVAS_BG).inner_margin(0.0))
            .show(ui, |ui| {
                self.document_canvas(ui);
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_from_find_returns_keyboard_input_to_body_after_modal_layer_expires() {
        let mut app = WriterApp::new();
        app.editor
            .replace_document(office_core::Document::with_text("original"));
        app.open_find();
        let ctx = Context::default();
        let escape = egui::Event::Key {
            key: Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        for events in [
            vec![],
            vec![escape],
            vec![],
            vec![],
            vec![egui::Event::Text("typed ".into())],
        ] {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| {
                    let ctx = ui.ctx().clone();
                    app.restore_find_focus(&ctx);
                    app.handle_text_input(&ctx);
                    app.show_find_dialog(&ctx);
                    egui::CentralPanel::default().show(ui, |ui| {
                        ui.interact(
                            ui.available_rect_before_wrap(),
                            egui::Id::new("writer_document"),
                            Sense::click_and_drag(),
                        );
                        app.focus_document =
                            ui.memory(|memory| memory.has_focus(egui::Id::new("writer_document")));
                    });
                },
            );
            output.textures_delta.clear();
        }
        assert!(!app.find_open);
        assert!(app.focus_document);
        assert_eq!(app.editor.document().plain_text(), "typed original");
        assert!(app.editor.undo());
        assert_eq!(app.editor.document().plain_text(), "original");
    }

    #[test]
    fn find_navigation_and_changed_query_do_not_replace_stale_selection() {
        let mut app = WriterApp::new();
        app.editor
            .replace_document(office_core::Document::with_text("日本語 old 日本語"));
        app.find_query = "日本語".into();
        app.open_find();
        app.find_body(true);
        assert_eq!(app.find_message, "Match 1 of 2");
        app.find_body(true);
        assert_eq!(app.find_message, "Match 2 of 2");
        app.find_body(false);
        assert_eq!(app.find_message, "Match 1 of 2");
        app.find_query = "old".into();
        app.find_replacement = "new".into();
        app.replace_found();
        assert_eq!(app.editor.document().plain_text(), "日本語 old 日本語");
        assert_eq!(app.editor.selected_text(), "old");
        assert!(!app.is_dirty());
        app.replace_found();
        assert_eq!(app.editor.document().plain_text(), "日本語 new 日本語");
        assert!(app.editor.undo());
        assert_eq!(app.editor.document().plain_text(), "日本語 old 日本語");
    }

    #[test]
    fn replace_all_saves_and_reloads_in_each_writer_format_and_undo_is_atomic() {
        let dir = tempfile::tempdir().unwrap();
        for extension in ["roffice.json", "docx", "odt"] {
            let mut app = WriterApp::new();
            app.editor
                .replace_document(office_core::Document::with_text("日本語 日本語\n日本語"));
            app.find_query = "日本語".into();
            app.find_replacement = "置換🙂".into();
            app.replace_all_found();
            assert_eq!(app.find_message, "Replaced 3 matches in body paragraphs");
            assert!(app.is_dirty());
            app.file_path = Some(dir.path().join(format!("replaced.{extension}")));
            assert!(app.save_file());
            assert_eq!(
                load_document(app.file_path.as_ref().unwrap())
                    .unwrap()
                    .plain_text(),
                "置換🙂 置換🙂\n置換🙂"
            );
            assert!(app.editor.undo());
            assert_eq!(app.editor.document().plain_text(), "日本語 日本語\n日本語");
            assert!(!app.editor.can_undo());
            assert!(app.editor.redo());
            assert_eq!(app.editor.document().plain_text(), "置換🙂 置換🙂\n置換🙂");
        }
    }

    #[test]
    fn find_dialog_input_never_types_into_document_and_no_match_preserves_state() {
        let mut app = WriterApp::new();
        app.editor
            .replace_document(office_core::Document::with_text("keep text"));
        app.find_query = "missing".into();
        let selection = app.editor.selection();
        app.find_body(true);
        assert_eq!(app.find_message, "No matches in body paragraphs");
        assert_eq!(app.editor.selection(), selection);
        let ctx = Context::default();
        ctx.begin_pass(egui::RawInput {
            events: vec![egui::Event::Text("search input".into())],
            ..Default::default()
        });
        app.open_find();
        app.handle_text_input(&ctx);
        let mut output = ctx.end_pass();
        output.textures_delta.clear();
        assert_eq!(app.editor.document().plain_text(), "keep text");
        assert!(!app.is_dirty());
        assert!(!app.editor.can_undo());
    }

    #[test]
    fn failed_docx_open_retains_document_selection_history_and_unsaved_edits() {
        assert_failed_writer_open_retains_state("docx", "word/document.xml");
    }

    #[test]
    fn failed_odt_open_retains_document_selection_history_and_unsaved_edits() {
        assert_failed_writer_open_retains_state("odt", "content.xml");
    }

    fn assert_failed_writer_open_retains_state(extension: &str, missing_part: &str) {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join(format!("broken.{extension}"));
        std::fs::write(&bad, b"PK\x05\x06\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0").unwrap();
        let mut app = WriterApp::new();
        app.editor.new_document();
        app.editor.insert_text("keep").unwrap();
        app.editor.insert_text(" edits").unwrap();
        app.file_path = Some(dir.path().join(format!("original.{extension}")));
        app.preferred_caret_x = Some(123.0);
        let text = app.editor.document().plain_text();
        let selection = app.editor.selection();
        let path = app.file_path.clone();
        app.open_path(&bad);
        assert_eq!(app.editor.document().plain_text(), text);
        assert_eq!(app.editor.selection(), selection);
        assert_eq!(app.file_path, path);
        assert_eq!(app.preferred_caret_x, Some(123.0));
        assert!(app.is_dirty());
        assert!(app.last_error.as_ref().unwrap().contains(missing_part));
        assert!(app.editor.undo());
        assert_eq!(app.editor.document().plain_text(), "keep");
        assert!(app.editor.redo());
        assert_eq!(app.editor.document().plain_text(), text);
        assert!(app.save_file());
        assert_eq!(
            load_document(app.file_path.as_ref().unwrap())
                .unwrap()
                .plain_text(),
            text
        );
    }

    #[test]
    fn successful_docx_open_replaces_document_and_resets_history_and_dirty_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("valid.docx");
        let mut editor = office_core::DocumentEditor::default();
        editor.insert_text("Loaded DOCX").unwrap();
        save_document(editor.document(), &path).unwrap();
        let mut app = WriterApp::new();
        app.editor.insert_text("old edits").unwrap();
        app.preferred_caret_x = Some(123.0);
        app.open_path(&path);
        assert_eq!(app.editor.document().plain_text(), "Loaded DOCX");
        assert_eq!(app.file_path, Some(path));
        assert_eq!(app.preferred_caret_x, None);
        assert!(!app.is_dirty());
        assert!(!app.editor.undo());
        assert!(app.last_error.is_none());
    }

    #[test]
    fn failed_save_does_not_replace_pending_new_document() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("destination.json");
        std::fs::create_dir(&path).unwrap();
        let mut app = WriterApp::new();
        app.file_path = Some(path.clone());
        app.editor.insert_text("keep edits").unwrap();
        let original = app.editor.document().plain_text();
        app.new_file();
        app.resolve_pending_action(Choice::Save);
        assert!(app.pending_action.is_none());
        assert_eq!(app.editor.document().plain_text(), original);
        assert_eq!(app.file_path, Some(path));
        assert!(app.is_dirty());
    }

    #[test]
    fn cancel_new_or_open_retains_writer_edits() {
        for action in [PendingAction::New, PendingAction::Open] {
            let mut app = WriterApp::new();
            app.editor.insert_text("keep edits").unwrap();
            let original = app.editor.document().plain_text();
            assert!(!app.request_or_run(action));
            app.resolve_pending_action(Choice::Cancel);
            assert!(app.pending_action.is_none());
            assert_eq!(app.editor.document().plain_text(), original);
            assert!(app.is_dirty());
        }
    }

    #[test]
    fn save_refuses_json_fallback_over_existing_unsupported_format() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("original.xlsx");
        std::fs::write(&path, b"original file").unwrap();
        let mut app = WriterApp::new();
        app.file_path = Some(path.clone());
        app.editor.insert_text("keep edits").unwrap();
        assert!(!app.save_file());
        assert_eq!(std::fs::read(&path).unwrap(), b"original file");
        assert!(app.is_dirty());
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    use office_core::Document;
    fn crashed_copy(root: &std::path::Path, doc: &Document) {
        let mut store = crate::recovery::Recovery::<office_core::Document>::new(root).unwrap();
        store
            .tick(doc, true, "original.docx", Instant::now())
            .unwrap();
    }
    #[test]
    fn restored_document_is_unsaved_has_no_original_destination_and_cleans_up_after_save() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("recovery");
        let doc = Document::with_text("日本語 recovered");
        crashed_copy(&root, &doc);
        let mut app = WriterApp::new();
        app.enable_recovery(Ok(root.clone()));
        assert!(app.recovery_open);
        assert!(app.restore_recovery(0));
        assert!(app.is_dirty());
        assert!(app.file_path.is_none());
        assert!(!app.editor.can_undo());
        assert_eq!(app.editor.document(), &doc);
        app.editor.insert_text("edit ").unwrap();
        app.editor.undo();
        assert_eq!(app.editor.document(), &doc);
        assert!(app.is_dirty());
        let restored = dir.path().join("restored.roffice.json");
        app.file_path = Some(restored.clone());
        assert!(app.save_file());
        assert_eq!(load_document(&restored).unwrap(), doc);
        drop(app);
        assert!(crate::recovery::Recovery::<office_core::Document>::new(&root)
            .unwrap()
            .entries
            .is_empty());
    }
    #[test]
    fn recovery_cannot_overwrite_current_unsaved_edits_and_postponing_keeps_the_copy() {
        let dir = tempfile::tempdir().unwrap();
        let doc = Document::with_text("other document");
        crashed_copy(dir.path(), &doc);
        let mut app = WriterApp::new();
        app.enable_recovery(Ok(dir.path().into()));
        app.editor.insert_text("keep this edit").unwrap();
        let before = app.editor.document().clone();
        assert!(!app.restore_recovery(0));
        assert_eq!(app.editor.document(), &before);
        assert!(app.is_dirty());
        app.recovery.as_mut().unwrap().postpone();
        drop(app);
        assert_eq!(
            crate::recovery::Recovery::<office_core::Document>::new(dir.path())
                .unwrap()
                .entries
                .len(),
            1
        );
    }
    #[test]
    fn unavailable_recovery_storage_does_not_block_editing_or_saving() {
        let dir = tempfile::tempdir().unwrap();
        let blocked = dir.path().join("blocked");
        std::fs::write(&blocked, b"keep").unwrap();
        let mut app = WriterApp::new();
        app.enable_recovery(Ok(blocked.clone()));
        assert!(app.recovery.is_none());
        app.editor.insert_text("still editable").unwrap();
        app.file_path = Some(dir.path().join("normal.roffice.json"));
        assert!(app.save_file());
        assert_eq!(std::fs::read(blocked).unwrap(), b"keep");
    }
}
