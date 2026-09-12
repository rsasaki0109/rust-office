//! Presentation engine for rust-office Impress.
//!
//! Slide model, themes, JSON interchange, and PPTX import/export.

mod json;
mod model;
mod pptx;
mod pptx_read;
mod theme;

pub use json::{is_impress_json_path, load_json_path, write_json_path, JsonError};
pub use model::{Presentation, Slide, TextBox, SLIDE_HEIGHT_PT, SLIDE_WIDTH_PT};
pub use pptx::{write_pptx_bytes, write_pptx_path, PptxError};
pub use pptx_read::{load_pptx_bytes, load_pptx_path};
pub use theme::Theme;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_round_trip_demo() {
        let p = Presentation::demo();
        let tmp = std::env::temp_dir().join("rust-office-impress-demo.rimpress.json");
        write_json_path(&p, &tmp).unwrap();
        let restored = load_json_path(&tmp).unwrap();
        assert_eq!(restored.slides.len(), 3);
        assert_eq!(restored.theme.name, "Ocean");
        assert!(restored.slides[0].title.text.contains("Impress"));
        let _ = std::fs::remove_file(tmp);
    }
}
