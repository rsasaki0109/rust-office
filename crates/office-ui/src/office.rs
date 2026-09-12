//! Top-level app that switches between Writer, Calc, and Impress.

use eframe::App;
use egui::Ui;

use crate::app::{SwitchTo as WriterSwitch, WriterApp};
use crate::calc_app::{CalcApp, SwitchTo as CalcSwitch};
use crate::impress_app::{ImpressApp, SwitchTo as ImpressSwitch};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Writer,
    Calc,
    Impress,
}

pub struct OfficeApp {
    mode: Mode,
    writer: WriterApp,
    calc: CalcApp,
    impress: ImpressApp,
}

impl OfficeApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        Self {
            mode: Mode::Writer,
            writer: WriterApp::new(cc),
            calc: CalcApp::new(),
            impress: ImpressApp::new(),
        }
    }
}

impl App for OfficeApp {
    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        match self.mode {
            Mode::Writer => {
                self.writer.ui(ui, frame);
                match self.writer.take_switch() {
                    Some(WriterSwitch::Calc) => self.mode = Mode::Calc,
                    Some(WriterSwitch::Impress) => self.mode = Mode::Impress,
                    None => {}
                }
            }
            Mode::Calc => {
                self.calc.ui(ui, frame);
                match self.calc.take_switch() {
                    Some(CalcSwitch::Writer) => self.mode = Mode::Writer,
                    Some(CalcSwitch::Impress) => self.mode = Mode::Impress,
                    None => {}
                }
            }
            Mode::Impress => {
                self.impress.ui(ui, frame);
                match self.impress.take_switch() {
                    Some(ImpressSwitch::Writer) => self.mode = Mode::Writer,
                    Some(ImpressSwitch::Calc) => self.mode = Mode::Calc,
                    None => {}
                }
            }
        }
    }
}
