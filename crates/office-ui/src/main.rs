//! rust-office — egui application entry point (Writer + Calc + Impress).

mod app;
mod calc_app;
mod fonts;
mod impress_app;
mod office;
mod print_sys;
mod theme;
mod unsaved;

use office::OfficeApp;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 800.0])
            .with_min_inner_size([720.0, 480.0])
            .with_title("rust-office"),
        ..Default::default()
    };

    eframe::run_native(
        "rust-office",
        options,
        Box::new(|cc| {
            fonts::install_cjk_fonts(&cc.egui_ctx);
            theme::apply_office_theme(&cc.egui_ctx);
            Ok(Box::new(OfficeApp::new(cc)))
        }),
    )
}
