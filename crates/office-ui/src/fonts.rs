//! Load system CJK fonts so Japanese IME input is visible.

use egui::{FontData, FontDefinitions, FontFamily};

const CJK_CANDIDATES: &[&str] = &[
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Bold.ttc",
    "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
    "/usr/share/fonts/opentype/noto/NotoSerifCJK-Regular.ttc",
    // macOS
    "/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc",
    "/System/Library/Fonts/Hiragino Sans GB.ttc",
    "/Library/Fonts/Arial Unicode.ttf",
    // Windows
    "C:\\Windows\\Fonts\\msgothic.ttc",
    "C:\\Windows\\Fonts\\meiryo.ttc",
    "C:\\Windows\\Fonts\\YuGothR.ttc",
    "C:\\Windows\\Fonts\\msyh.ttc",
];

pub fn install_cjk_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();

    for path in CJK_CANDIDATES {
        if let Ok(bytes) = std::fs::read(path) {
            let name = "cjk_fallback".to_owned();
            fonts
                .font_data
                .insert(name.clone(), std::sync::Arc::new(FontData::from_owned(bytes)));

            fonts
                .families
                .entry(FontFamily::Proportional)
                .or_default()
                .push(name.clone());
            fonts
                .families
                .entry(FontFamily::Monospace)
                .or_default()
                .push(name);

            ctx.set_fonts(fonts);
            eprintln!("rust-office: loaded CJK font from {path}");
            return;
        }
    }

    eprintln!(
        "rust-office: no CJK font found; Japanese glyphs may render as tofu. \
         Install Noto Sans CJK or a system Japanese font."
    );
}
