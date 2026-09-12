//! System print / preview helpers for Writer.

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

use office_core::Document;
use office_render::document_to_layout_pdf_bytes;

#[derive(Debug)]
pub enum PrintError {
    Pdf(String),
    Io(std::io::Error),
    Command(String),
}

impl fmt::Display for PrintError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PrintError::Pdf(s) => write!(f, "pdf error: {s}"),
            PrintError::Io(e) => write!(f, "io error: {e}"),
            PrintError::Command(s) => write!(f, "print command failed: {s}"),
        }
    }
}

impl From<std::io::Error> for PrintError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

/// Write `document` to a unique temp PDF path.
pub fn write_temp_pdf(document: &Document, stem: &str) -> Result<PathBuf, PrintError> {
    let bytes = document_to_layout_pdf_bytes(document).map_err(PrintError::Pdf)?;
    let mut path = std::env::temp_dir();
    let name = format!(
        "rust-office-{}-{}.pdf",
        sanitize_stem(stem),
        std::process::id()
    );
    path.push(name);
    std::fs::write(&path, bytes)?;
    Ok(path)
}

fn sanitize_stem(stem: &str) -> String {
    let s: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() {
        "document".into()
    } else {
        s
    }
}

/// Open PDF in the default viewer (print preview).
pub fn open_pdf_preview(path: &Path) -> Result<(), PrintError> {
    open::that(path).map_err(|e| PrintError::Command(e.to_string()))
}

/// Send PDF to the system print spooler when possible; otherwise open for preview.
pub fn send_pdf_to_printer(path: &Path) -> Result<PrintAction, PrintError> {
    let path_str = path.to_string_lossy().to_string();

    #[cfg(target_os = "linux")]
    {
        if try_command("lp", &[&path_str]) || try_command("lpr", &[&path_str]) {
            return Ok(PrintAction::Spooled);
        }
        open_pdf_preview(path)?;
        Ok(PrintAction::PreviewFallback)
    }
    #[cfg(target_os = "macos")]
    {
        if try_command("lpr", &[&path_str]) {
            return Ok(PrintAction::Spooled);
        }
        open_pdf_preview(path)?;
        Ok(PrintAction::PreviewFallback)
    }
    #[cfg(target_os = "windows")]
    {
        open_pdf_preview(path)?;
        Ok(PrintAction::PreviewFallback)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        open_pdf_preview(path)?;
        Ok(PrintAction::PreviewFallback)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrintAction {
    Spooled,
    PreviewFallback,
}

fn try_command(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use office_core::DocumentEditor;

    #[test]
    fn temp_pdf_starts_with_header() {
        let mut ed = DocumentEditor::default();
        ed.insert_text("Print me").unwrap();
        let path = write_temp_pdf(ed.document(), "test").unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.starts_with(b"%PDF"));
        let _ = std::fs::remove_file(path);
    }
}
