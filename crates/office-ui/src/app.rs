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

const FONT_SIZES: &[f32] = &[8.0, 9.0, 10.0, 11.0, 12.0, 14.0, 16.0, 18.0, 20.0, 24.0, 28.0, 36.0];

/// Action deferred until the user resolves an unsaved-changes prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingAction {
    New,
    Open,
    Quit,
}

/// Request to switch the suite app mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchTo {
    Calc,
    Impress,
}

pub struct WriterApp {
    editor: DocumentEditor,
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
    /// Preferred screen X while moving with ↑/↓ (cleared on horizontal moves).
    preferred_caret_x: Option<f32>,
    /// When set, show the unsaved-changes modal.
    pending_action: Option<PendingAction>,
    /// Decoded GPU textures keyed by [`office_core::Image::cache_key`].
    image_textures: HashMap<String, TextureHandle>,
    /// When set, show the hyperlink URL modal; holds the draft URL.
    link_dialog: Option<String>,
    /// When set, show header/footer modal: (header draft, footer draft).
    header_footer_dialog: Option<(String, String)>,
    /// When set, show the print dialog.
    print_dialog: Option<PrintDialog>,
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
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
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
            preferred_caret_x: None,
            pending_action: None,
            image_textures: HashMap::new(),
            link_dialog: None,
            header_footer_dialog: None,
            print_dialog: None,
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
        match load_document(&path) {
            Ok(doc) => {
                self.editor.replace_document(doc);
                self.file_path = Some(path);
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
        let header = self
            .editor
            .document()
            .header()
            .map(|p| p.plain_text())
            .unwrap_or_default();
        let footer = self
            .editor
            .document()
            .footer()
            .map(|p| p.plain_text())
            .unwrap_or_default();
        self.header_footer_dialog = Some((header, footer));
    }

    fn show_header_footer_dialog(&mut self, ctx: &Context) {
        let Some((header, footer)) = self.header_footer_dialog.as_mut() else {
            return;
        };
        let mut apply = false;
        let mut cancel = false;
        egui::Modal::new(egui::Id::new("header_footer_dialog")).show(ctx, |ui| {
            ui.heading("Header and Footer");
            ui.label("Header");
            ui.add(
                egui::TextEdit::singleline(header)
                    .desired_width(360.0)
                    .hint_text("Document title…"),
            );
            ui.add_space(8.0);
            ui.label("Footer (use {page} / {pages} for page numbers)");
            ui.add(
                egui::TextEdit::singleline(footer)
                    .desired_width(360.0)
                    .hint_text("Page {page} of {pages}"),
            );
            ui.horizontal(|ui| {
                if ui.button("OK").clicked() {
                    apply = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
        if apply {
            let (header, footer) = self.header_footer_dialog.take().unwrap_or_default();
            let _ = self.editor.set_header_text(header.trim());
            let _ = self.editor.set_footer_text(footer.trim());
            self.set_status("Header / footer updated");
        } else if cancel {
            self.header_footer_dialog = None;
        }
    }

    fn prune_image_textures(&mut self) {
        use office_core::Block;
        let live: std::collections::HashSet<String> = self
            .editor
            .document()
            .blocks()
            .iter()
            .filter_map(|b| match b {
                Block::Image(img) => Some(img.cache_key()),
                _ => None,
            })
            .collect();
        self.image_textures.retain(|k, _| live.contains(k));
    }

    fn request_quit(&mut self, ctx: &Context) {
        if !self.request_or_run(PendingAction::Quit) {
            return;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
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

    fn finish_pending_after_discard(&mut self, ctx: &Context) {
        let Some(action) = self.pending_action.take() else {
            return;
        };
        match action {
            PendingAction::New => self.do_new_file(),
            PendingAction::Open => self.do_open_file(),
            PendingAction::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
        }
    }

    fn finish_pending_after_save(&mut self, ctx: &Context) {
        self.save_file();
        if self.editor.is_dirty() {
            // Save cancelled or failed — keep the prompt dismissed but stay put.
            self.pending_action = None;
            return;
        }
        self.finish_pending_after_discard(ctx);
    }

    fn show_unsaved_dialog(&mut self, ctx: &Context) {
        if self.pending_action.is_none() {
            return;
        }

        #[derive(Clone, Copy)]
        enum Choice {
            Save,
            Discard,
            Cancel,
        }
        let mut choice = None::<Choice>;

        egui::Modal::new(egui::Id::new("unsaved_changes")).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.heading("Unsaved changes");
            ui.add_space(6.0);
            ui.label("Save changes to the current document?");
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    choice = Some(Choice::Save);
                }
                if ui.button("Don't Save").clicked() {
                    choice = Some(Choice::Discard);
                }
                if ui.button("Cancel").clicked() {
                    choice = Some(Choice::Cancel);
                }
            });
        });

        match choice {
            Some(Choice::Save) => self.finish_pending_after_save(ctx),
            Some(Choice::Discard) => self.finish_pending_after_discard(ctx),
            Some(Choice::Cancel) => self.pending_action = None,
            None => {}
        }
    }

    fn save_file(&mut self) {
        if self.file_path.is_none() {
            self.save_file_as();
            return;
        }
        let path = self.file_path.clone().unwrap();
        match save_document(self.editor.document(), &path) {
            Ok(()) => {
                self.editor.mark_clean();
                self.set_status(format!("Saved {}", path.display()));
            }
            Err(e) => self.set_error(format!("Save failed: {e}")),
        }
    }

    fn save_file_as(&mut self) {
        let path = rfd::FileDialog::new()
            .add_filter("OpenDocument Text", &["odt"])
            .add_filter("Word Document", &["docx"])
            .add_filter("rust-office", &["roffice.json", "json"])
            .set_file_name("document.odt")
            .save_file();
        let Some(path) = path else {
            return;
        };
        match save_document(self.editor.document(), &path) {
            Ok(()) => {
                self.editor.mark_clean();
                self.file_path = Some(path.clone());
                self.set_status(format!("Saved {}", path.display()));
            }
            Err(e) => self.set_error(format!("Save failed: {e}")),
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
                ui.label(format!(
                    "Layout pages (on screen): {}",
                    dialog.layout_pages
                ));
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
            (KeyboardShortcut::new(Modifiers::COMMAND, Key::N), "new"),
            (KeyboardShortcut::new(Modifiers::COMMAND, Key::O), "open"),
            (KeyboardShortcut::new(Modifiers::COMMAND, Key::S), "save"),
            (KeyboardShortcut::new(Modifiers::COMMAND, Key::Z), "undo"),
            (
                KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z),
                "redo",
            ),
            (KeyboardShortcut::new(Modifiers::COMMAND, Key::Y), "redo"),
            (KeyboardShortcut::new(Modifiers::COMMAND, Key::A), "select_all"),
            (KeyboardShortcut::new(Modifiers::COMMAND, Key::B), "bold"),
            (KeyboardShortcut::new(Modifiers::COMMAND, Key::I), "italic"),
            (KeyboardShortcut::new(Modifiers::COMMAND, Key::U), "underline"),
            (KeyboardShortcut::new(Modifiers::COMMAND, Key::C), "copy"),
            (KeyboardShortcut::new(Modifiers::COMMAND, Key::X), "cut"),
            (KeyboardShortcut::new(Modifiers::COMMAND, Key::V), "paste"),
            (
                KeyboardShortcut::new(Modifiers::COMMAND, Key::Equals),
                "zoom_in",
            ),
            (KeyboardShortcut::new(Modifiers::COMMAND, Key::Minus), "zoom_out"),
            (KeyboardShortcut::new(Modifiers::COMMAND, Key::Num0), "zoom_reset"),
            (
                KeyboardShortcut::new(Modifiers::COMMAND, Key::Enter),
                "page_break",
            ),
            (KeyboardShortcut::new(Modifiers::COMMAND, Key::P), "print"),
        ];

        for (shortcut, action) in macros {
            if ctx.input_mut(|i| i.consume_shortcut(&shortcut)) {
                match action {
                    "new" => self.new_file(),
                    "open" => self.open_file(),
                    "save" => self.save_file(),
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
                if ui.button("Header and Footer…").clicked() {
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
            toolbar_btn(ui, "Save", || self.save_file());
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
                EditFocus::Header => self
                    .editor
                    .document()
                    .header()
                    .map(|p| p.style.alignment)
                    .unwrap_or_default(),
                EditFocus::Footer => self
                    .editor
                    .document()
                    .footer()
                    .map(|p| p.style.alignment)
                    .unwrap_or_default(),
                EditFocus::Body => self
                    .editor
                    .document()
                    .paragraph(self.editor.selection().focus.paragraph)
                    .map(|p| p.style.alignment)
                    .unwrap_or_default(),
            };
            toggle_btn(ui, "Left", align == Alignment::Left, RichText::new("⬅"), || {
                let _ = self.editor.set_alignment(Alignment::Left);
            });
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
                EditFocus::Header => {
                    ui.label("Header");
                    ui.separator();
                }
                EditFocus::Footer => {
                    ui.label("Footer");
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
        let page = self.editor.document().page_style().clone();
        let page_size = Vec2::new(page.width, page.height) * zoom;
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
                    (page_size.x + margin * 2.0).max(ui.available_width()),
                    canvas.y + margin * 2.0,
                );
                let (response, painter) = ui.allocate_painter(desired, Sense::click_and_drag());
                let response = ui.interact(response.rect, doc_id, Sense::click_and_drag());

                painter.rect_filled(response.rect, 0.0, CANVAS_BG);

                let origin = Pos2::new(
                    response.rect.left()
                        + ((response.rect.width() - page_size.x) * 0.5).max(margin),
                    response.rect.top() + margin,
                );

                let layout = layout_document(ui, self.editor.document(), origin, zoom);
                ensure_image_textures(
                    ui.ctx(),
                    self.editor.document(),
                    &mut self.image_textures,
                );
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
                                    if extend && matches!(self.editor.edit_focus(), EditFocus::Header)
                                    {
                                        let sel = self.editor.selection().with_focus(doc_pos);
                                        self.editor.set_selection(sel);
                                    } else {
                                        self.editor.focus_header(doc_pos.offset);
                                    }
                                }
                                HitResult::Footer(doc_pos) => {
                                    if extend && matches!(self.editor.edit_focus(), EditFocus::Footer)
                                    {
                                        let sel = self.editor.selection().with_focus(doc_pos);
                                        self.editor.set_selection(sel);
                                    } else {
                                        self.editor.focus_footer(doc_pos.offset);
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
                                    if matches!(self.editor.edit_focus(), EditFocus::Header) =>
                                {
                                    let sel = self.editor.selection().with_focus(doc_pos);
                                    self.editor.set_selection(sel);
                                }
                                HitResult::Footer(doc_pos)
                                    if matches!(self.editor.edit_focus(), EditFocus::Footer) =>
                                {
                                    let sel = self.editor.selection().with_focus(doc_pos);
                                    self.editor.set_selection(sel);
                                }
                                HitResult::Body(doc_pos)
                                    if self.editor.edit_focus().is_body() =>
                                {
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
                        EditFocus::Header | EditFocus::Footer => layout.margin_caret_rect(
                            self.editor.edit_focus(),
                            self.editor.selection().focus,
                            1,
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
                        EditFocus::Header | EditFocus::Footer => layout.margin_caret_rect(
                            self.editor.edit_focus(),
                            self.editor.selection().focus,
                            1,
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
        [Pos2::new(pre_rect.left(), y), Pos2::new(pre_rect.right(), y)],
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

fn toggle_btn(
    ui: &mut Ui,
    id: &str,
    selected: bool,
    label: RichText,
    mut on_click: impl FnMut(),
) {
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

        // Intercept window close while dirty.
        if ctx.input(|i| i.viewport().close_requested()) && self.editor.is_dirty() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.pending_action = Some(PendingAction::Quit);
        }

        // Focus the document canvas on first frame so typing works immediately.
        if self.focus_document && ctx.memory(|m| m.focused().is_none()) {
            ctx.memory_mut(|m| m.request_focus(egui::Id::new("writer_document")));
        }

        if self.pending_action.is_none()
            && self.link_dialog.is_none()
            && self.header_footer_dialog.is_none()
            && self.print_dialog.is_none()
        {
            self.handle_shortcuts(&ctx);
            self.handle_text_input(&ctx);
        }
        self.show_unsaved_dialog(&ctx);
        self.show_link_dialog(&ctx);
        self.show_header_footer_dialog(&ctx);
        self.show_print_dialog(&ctx);
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
