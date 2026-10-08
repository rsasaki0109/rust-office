//! Top-level app that switches between Writer, Calc, and Impress.

use eframe::App;
use egui::Ui;

use crate::app::{SwitchTo as WriterSwitch, WriterApp};
use crate::calc_app::{CalcApp, SwitchTo as CalcSwitch};
use crate::impress_app::{ImpressApp, SwitchTo as ImpressSwitch};
use crate::unsaved::{self, Choice};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Writer,
    Calc,
    Impress,
}

#[derive(Default)]
struct CloseConfirmation {
    // Discard decisions belong to this close attempt only. Cancelling a later
    // prompt must not mark earlier documents clean or erase their contents.
    discarded: Vec<Mode>,
}

impl CloseConfirmation {
    fn next(&self, dirty: &[Mode]) -> Option<Mode> {
        dirty
            .iter()
            .copied()
            .find(|mode| !self.discarded.contains(mode))
    }
}

pub struct OfficeApp {
    mode: Mode,
    writer: WriterApp,
    calc: CalcApp,
    impress: ImpressApp,
    closing: Option<CloseConfirmation>,
    close_authorized: bool,
}

impl OfficeApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        Self {
            mode: Mode::Writer,
            writer: WriterApp::new(),
            calc: CalcApp::new(),
            impress: ImpressApp::new(),
            closing: None,
            close_authorized: false,
        }
    }

    fn dirty_modes(&self) -> Vec<Mode> {
        let mut dirty = Vec::new();
        for mode in [self.mode, Mode::Writer, Mode::Calc, Mode::Impress] {
            let edited = match mode {
                Mode::Writer => self.writer.is_dirty(),
                Mode::Calc => self.calc.is_dirty(),
                Mode::Impress => self.impress.is_dirty(),
            };
            if edited && !dirty.contains(&mode) {
                dirty.push(mode);
            }
        }
        dirty
    }

    fn request_close(&mut self, ctx: &egui::Context) {
        if self.dirty_modes().is_empty() {
            self.close_authorized = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if self.closing.is_none() {
                self.writer.cancel_pending_action();
                self.calc.cancel_pending_action();
                self.impress.cancel_pending_action();
                self.closing = Some(CloseConfirmation::default());
            }
        }
    }

    fn show_close_dialog(&mut self, ctx: &egui::Context) {
        let Some(closing) = &self.closing else {
            return;
        };
        let Some(mode) = closing.next(&self.dirty_modes()) else {
            self.closing = None;
            self.close_authorized = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        };
        let title = match mode {
            Mode::Writer => self.writer.title(),
            Mode::Calc => self.calc.title(),
            Mode::Impress => self.impress.title(),
        };
        match unsaved::confirm(ctx, "suite_unsaved", &title) {
            Some(Choice::Save) => {
                let saved = match mode {
                    Mode::Writer => self.writer.save_file(),
                    Mode::Calc => self.calc.save_file(),
                    Mode::Impress => self.impress.save_file(),
                };
                if !saved {
                    self.mode = mode; // Show the document and any save error.
                    self.closing = None;
                }
                ctx.request_repaint();
            }
            Some(Choice::Discard) => {
                self.closing.as_mut().unwrap().discarded.push(mode);
                ctx.request_repaint();
            }
            Some(Choice::Cancel) => self.closing = None,
            None => {}
        }
    }
}

impl App for OfficeApp {
    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let close_requested =
            ctx.input(|i| i.viewport().close_requested()) && !self.close_authorized;
        if close_requested {
            // Process this frame's final edits before checking for unsaved work.
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
        if self.closing.is_some() {
            ui.disable();
        }
        match self.mode {
            Mode::Writer => {
                self.writer.ui(ui, frame);
                match self.writer.take_switch() {
                    Some(WriterSwitch::Calc) => self.mode = Mode::Calc,
                    Some(WriterSwitch::Impress) => self.mode = Mode::Impress,
                    Some(WriterSwitch::Quit) => self.request_close(&ctx),
                    None => {}
                }
            }
            Mode::Calc => {
                self.calc.ui(ui, frame);
                match self.calc.take_switch() {
                    Some(CalcSwitch::Writer) => self.mode = Mode::Writer,
                    Some(CalcSwitch::Impress) => self.mode = Mode::Impress,
                    Some(CalcSwitch::Quit) => self.request_close(&ctx),
                    None => {}
                }
            }
            Mode::Impress => {
                self.impress.ui(ui, frame);
                match self.impress.take_switch() {
                    Some(ImpressSwitch::Writer) => self.mode = Mode::Writer,
                    Some(ImpressSwitch::Calc) => self.mode = Mode::Calc,
                    Some(ImpressSwitch::Quit) => self.request_close(&ctx),
                    None => {}
                }
            }
        }
        if close_requested {
            self.request_close(&ctx);
        }
        self.show_close_dialog(&ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_checks_inactive_documents_and_allows_cancelling_discard() {
        let dirty = [Mode::Calc, Mode::Writer, Mode::Impress];
        let mut closing = CloseConfirmation::default();
        assert_eq!(closing.next(&dirty), Some(Mode::Calc));
        closing.discarded.push(Mode::Calc);
        assert_eq!(closing.next(&dirty), Some(Mode::Writer));
        // Cancel and retry: the previously discarded Calc is still unsaved.
        assert_eq!(CloseConfirmation::default().next(&dirty), Some(Mode::Calc));
        closing.discarded.extend([Mode::Writer, Mode::Impress]);
        assert_eq!(closing.next(&dirty), None);
    }
}
