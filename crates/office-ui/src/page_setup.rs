//! Section-scoped page setup draft. Nothing changes until Apply.
use egui::Context;
use office_core::{Document, PageStyle, Paragraph};

pub struct PageSetupDraft {
    pub section: usize,
    pub style: PageStyle,
    pub header: String,
    pub footer: String,
    inherit_header: bool,
    inherit_footer: bool,
}

impl PageSetupDraft {
    pub fn new(document: &Document, section: usize) -> Self {
        let current = &document.sections[section];
        Self {
            section,
            style: current.page_style.clone(),
            header: document
                .section_header(section)
                .map(|p| p.plain_text())
                .unwrap_or_default(),
            footer: document
                .section_footer(section)
                .map(|p| p.plain_text())
                .unwrap_or_default(),
            inherit_header: section > 0 && current.header.is_none(),
            inherit_footer: section > 0 && current.footer.is_none(),
        }
    }

    pub fn margins(&self, document: &Document) -> (Option<Paragraph>, Option<Paragraph>) {
        let value = |original: Option<&Paragraph>, text: &str, inherit: bool| {
            if inherit {
                None
            } else if let Some(original) = original.filter(|p| p.plain_text() == text) {
                Some(original.clone())
            } else if self.section == 0 && text.is_empty() {
                None
            } else {
                Some(Paragraph::from_text(text))
            }
        };
        (
            value(
                document.section_header(self.section),
                &self.header,
                self.inherit_header,
            ),
            value(
                document.section_footer(self.section),
                &self.footer,
                self.inherit_footer,
            ),
        )
    }

    pub fn show(&mut self, ctx: &Context, document: &Document) -> (bool, bool) {
        let mut apply = false;
        let mut cancel = false;
        egui::Modal::new(egui::Id::new("header_footer_dialog")).show(ctx, |ui| {
            ui.heading("Page / Section Setup");
            let mut selected = self.section;
            egui::ComboBox::from_id_salt("page_setup_section")
                .selected_text(format!("Section {}", self.section + 1))
                .show_ui(ui, |ui| {
                    for index in 0..document.sections.len() {
                        ui.selectable_value(&mut selected, index, format!("Section {}", index + 1));
                    }
                });
            ui.small("Selecting another section reloads its saved settings.");
            if selected != self.section {
                *self = Self::new(document, selected);
            }
            ui.horizontal(|ui| {
                ui.label("Paper");
                for (label, width, height) in [
                    ("A4", 595.28, 841.89),
                    ("Letter", 612.0, 792.0),
                    ("A5", 419.53, 595.28),
                ] {
                    if ui.button(label).clicked() {
                        let landscape = self.style.width > self.style.height;
                        (self.style.width, self.style.height) = if landscape {
                            (height, width)
                        } else {
                            (width, height)
                        };
                    }
                }
                if ui.button("Rotate").clicked() {
                    std::mem::swap(&mut self.style.width, &mut self.style.height);
                }
            });
            egui::Grid::new("page_setup_dimensions")
                .num_columns(4)
                .show(ui, |ui| {
                    edit_mm(ui, "Width", &mut self.style.width);
                    edit_mm(ui, "Height", &mut self.style.height);
                    ui.end_row();
                    edit_mm(ui, "Top margin", &mut self.style.margin_top);
                    edit_mm(ui, "Bottom margin", &mut self.style.margin_bottom);
                    ui.end_row();
                    edit_mm(ui, "Left margin", &mut self.style.margin_left);
                    edit_mm(ui, "Right margin", &mut self.style.margin_right);
                    ui.end_row();
                });
            ui.label("Header");
            if self.section > 0 {
                ui.checkbox(&mut self.inherit_header, "Use first section header");
            }
            ui.add_enabled(
                !self.inherit_header,
                egui::TextEdit::singleline(&mut self.header).desired_width(400.0),
            );
            ui.label("Footer (use {page} / {pages} for page numbers)");
            if self.section > 0 {
                ui.checkbox(&mut self.inherit_footer, "Use first section footer");
            }
            ui.add_enabled(
                !self.inherit_footer,
                egui::TextEdit::singleline(&mut self.footer).desired_width(400.0),
            );
            if !self.style.is_valid() {
                ui.colored_label(
                    egui::Color32::RED,
                    "Paper: 25.4–1016 mm. Margins must leave at least 12.7 mm of content.",
                );
            }
            ui.horizontal(|ui| {
                apply = ui
                    .add_enabled(self.style.is_valid(), egui::Button::new("Apply"))
                    .clicked();
                cancel = ui.button("Cancel").clicked();
            });
        });
        (apply, cancel)
    }
}

fn edit_mm(ui: &mut egui::Ui, label: &str, points: &mut f32) {
    ui.label(label);
    let mut mm = *points * 25.4 / 72.0;
    if ui
        .add(
            egui::DragValue::new(&mut mm)
                .speed(0.5)
                .suffix(" mm")
                .max_decimals(2),
        )
        .changed()
    {
        *points = mm * 72.0 / 25.4;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unchanged_draft_preserves_runs_and_distinguishes_blank_from_inherited() {
        let mut doc = Document::with_text("body");
        let mut header = Paragraph::from_text("  Header  ");
        header.runs[0].style.bold = true;
        doc.sections[0].header = Some(header.clone());
        doc.sections.push(doc.sections[0].clone());
        doc.sections[1].header = None;
        let mut draft = PageSetupDraft::new(&doc, 1);
        assert_eq!(draft.margins(&doc).0, None);
        draft.inherit_header = false;
        assert_eq!(draft.margins(&doc).0, Some(header.clone()));
        draft.header.clear();
        assert_eq!(draft.margins(&doc).0.unwrap().plain_text(), "");
        let first = PageSetupDraft::new(&doc, 0);
        assert_eq!(first.margins(&doc).0, Some(header));
        assert_eq!(first.style, doc.sections[0].page_style);
    }
}
