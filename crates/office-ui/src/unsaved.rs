//! Shared confirmation UI for replacing or closing edited documents.

use egui::Context;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentAction {
    New,
    Open,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Save,
    Discard,
    Cancel,
}

pub fn confirm(ctx: &Context, id: &str, document: &str) -> Option<Choice> {
    let mut choice = None;
    let response = egui::Modal::new(egui::Id::new(id)).show(ctx, |ui| {
        ui.set_width(380.0);
        ui.heading("Unsaved changes");
        ui.add_space(6.0);
        ui.label(format!("Save changes to {document}?"));
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
    choice.or_else(|| response.should_close().then_some(Choice::Cancel))
}
