//! Native JSON interchange for presentations.

use std::fs;
use std::path::Path;

use thiserror::Error;

use crate::model::Presentation;

#[derive(Debug, Error)]
pub enum JsonError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

pub fn load_json_path(path: &Path) -> Result<Presentation, JsonError> {
    let text = fs::read_to_string(path)?;
    let mut p: Presentation = serde_json::from_str(&text)?;
    p.mark_clean();
    if p.slides.is_empty() {
        p.slides.push(crate::model::Slide::blank());
        p.active = 0;
    }
    if p.active >= p.slides.len() {
        p.active = 0;
    }
    Ok(p)
}

pub fn write_json_path(presentation: &Presentation, path: &Path) -> Result<(), JsonError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let text = serde_json::to_string_pretty(presentation)?;
    fs::write(path, text)?;
    Ok(())
}

pub fn is_impress_json_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("json"))
        && path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.contains("impress") || n.ends_with(".rimpress.json"))
}
