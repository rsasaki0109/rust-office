//! Clipboard provenance for formulas copied by this Calc session.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use office_calc::CellAddr;

static NEXT_COPY: AtomicU64 = AtomicU64::new(0);

pub(super) struct CopiedCells {
    pub text: String,
    source: Option<CellAddr>,
    marker: String,
}

impl CopiedCells {
    pub fn new(text: String, source: CellAddr, cut: bool) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let marker = format!(
            "<!--rust-office-calc-copy:{}:{}:{}-->",
            std::process::id(),
            timestamp,
            NEXT_COPY.fetch_add(1, Ordering::Relaxed)
        );
        Self {
            text,
            source: (!cut).then_some(source),
            marker,
        }
    }

    /// Native HTML accompanies the unchanged TSV plain-text representation.
    /// The marker matches a retained snapshot, rather than trusting arbitrary
    /// clipboard HTML to supply a source position.
    pub fn html(&self) -> String {
        let escaped = self
            .text
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        format!("{}<pre>{escaped}</pre>", self.marker)
    }

    pub fn origin_for(&self, text: &str, html: Option<&str>) -> Option<CellAddr> {
        if text == self.text && html.is_some_and(|html| html.contains(&self.marker)) {
            self.source
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_text_alone_cannot_claim_a_copy_origin() {
        let copy = CopiedCells::new("=A1\n".into(), CellAddr::new(1, 1), false);
        assert_eq!(copy.origin_for(&copy.text, None), None);
        assert_eq!(copy.origin_for(&copy.text, Some("<pre>=A1</pre>")), None);
        assert_eq!(copy.origin_for("different", Some(&copy.html())), None);
        assert_eq!(
            copy.origin_for(&copy.text, Some(&copy.html())),
            Some(CellAddr::new(1, 1))
        );
        let newer = CopiedCells::new(copy.text.clone(), CellAddr::new(2, 2), false);
        assert_eq!(newer.origin_for(&copy.text, Some(&copy.html())), None);
    }

    #[test]
    fn cut_keeps_formulas_verbatim_and_html_escapes_cell_text() {
        let cut = CopiedCells::new("<script>&=A1\n".into(), CellAddr::new(1, 1), true);
        assert_eq!(cut.origin_for(&cut.text, Some(&cut.html())), None);
        assert!(cut.html().contains("&lt;script&gt;&amp;=A1"));
    }
}
