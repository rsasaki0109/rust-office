//! Office-like visual theme (menus, toolbar, canvas chrome).

use egui::{Color32, CornerRadius, Stroke, Theme, Visuals};

pub fn apply_office_theme(ctx: &egui::Context) {
    let mut style = (*ctx.style_of(Theme::Light)).clone();
    style.spacing.item_spacing = egui::vec2(6.0, 4.0);
    style.spacing.button_padding = egui::vec2(8.0, 4.0);
    style.spacing.window_margin = egui::Margin::same(0);
    style.visuals = office_visuals();
    ctx.set_style_of(Theme::Light, style);
    ctx.set_theme(Theme::Light);
}

fn office_visuals() -> Visuals {
    let mut v = Visuals::light();
    v.window_fill = Color32::from_rgb(245, 246, 248);
    v.panel_fill = Color32::from_rgb(245, 246, 248);
    v.faint_bg_color = Color32::from_rgb(232, 234, 238);
    v.widgets.inactive.bg_fill = Color32::from_rgb(245, 246, 248);
    v.widgets.hovered.bg_fill = Color32::from_rgb(226, 232, 240);
    v.widgets.active.bg_fill = Color32::from_rgb(200, 214, 232);
    v.selection.bg_fill = Color32::from_rgb(51, 112, 176);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(210, 214, 220));
    v.window_stroke = Stroke::NONE;
    v.window_corner_radius = CornerRadius::ZERO;
    v.menu_corner_radius = CornerRadius::same(2);
    v
}

pub const CANVAS_BG: Color32 = Color32::from_rgb(160, 168, 176);
pub const TOOLBAR_BG: Color32 = Color32::from_rgb(245, 246, 248);
pub const STATUS_BG: Color32 = Color32::from_rgb(236, 238, 242);
pub const ACCENT: Color32 = Color32::from_rgb(46, 95, 158);
