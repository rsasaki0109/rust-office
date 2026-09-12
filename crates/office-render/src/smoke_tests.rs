//! Headless layout smoke tests (no window / DISPLAY required).

use egui::Pos2;
use office_core::{DocumentEditor, Selection};

use crate::{document_canvas_size, layout_document};

#[test]
fn smoke_layout_welcome_doc_single_page() {
    egui::__run_test_ui(|ui| {
        let mut ed = DocumentEditor::default();
        ed.insert_text("Hello rust-office.\nSecond line.").unwrap();
        let layout = layout_document(ui, ed.document(), Pos2::ZERO, 1.0);
        assert_eq!(layout.page_count(), 1);
        assert!(!layout.pages[0].lines.is_empty());
        let size = document_canvas_size(&layout);
        assert!(size.x > 100.0);
        assert!(size.y > 100.0);
    });
}

#[test]
fn smoke_layout_overflow_creates_pages() {
    egui::__run_test_ui(|ui| {
        let short = {
            let mut ed = DocumentEditor::default();
            ed.insert_text("Short.").unwrap();
            layout_document(ui, ed.document(), Pos2::ZERO, 1.0)
        };
        let long = {
            let mut ed = DocumentEditor::default();
            // ~120 short paragraphs (~16pt each + space_after) exceed A4 content height (~700pt).
            let body = "Line of text for pagination smoke.\n".repeat(120);
            ed.insert_text(&body).unwrap();
            layout_document(ui, ed.document(), Pos2::ZERO, 1.0)
        };
        assert_eq!(short.page_count(), 1);
        assert!(
            long.page_count() >= 2,
            "expected multi-page layout, got {} (canvas {:?})",
            long.page_count(),
            document_canvas_size(&long)
        );
        assert!(document_canvas_size(&long).y > document_canvas_size(&short).y);
    });
}

#[test]
fn smoke_layout_nested_list_indent() {
    egui::__run_test_ui(|ui| {
        let mut ed = DocumentEditor::default();
        ed.insert_text("Parent\nChild").unwrap();
        ed.select_all();
        ed.toggle_bullet_list().unwrap();
        ed.set_selection(Selection::caret(office_core::DocPosition::new(1, 0)));
        ed.indent_list().unwrap();
        let layout = layout_document(ui, ed.document(), Pos2::ZERO, 1.0);
        let markers: Vec<_> = layout
            .pages
            .iter()
            .flat_map(|p| p.lines.iter())
            .filter_map(|l| l.list_marker.as_ref())
            .collect();
        assert!(markers.len() >= 2);
        // Nested child should sit further right than the parent line.
        let lines: Vec<_> = layout.pages.iter().flat_map(|p| p.lines.iter()).collect();
        let parent = lines.iter().find(|l| l.paragraph == 0).unwrap();
        let child = lines.iter().find(|l| l.paragraph == 1).unwrap();
        assert!(
            child.rect.left() > parent.rect.left(),
            "nested item should be indented (parent {} child {})",
            parent.rect.left(),
            child.rect.left()
        );
    });
}

#[test]
fn smoke_layout_list_and_table() {
    egui::__run_test_ui(|ui| {
        let mut ed = DocumentEditor::default();
        ed.insert_text("Item one").unwrap();
        ed.toggle_bullet_list().unwrap();
        ed.set_selection(Selection::caret(office_core::DocPosition::new(0, 8)));
        ed.insert_text("\nItem two").unwrap();
        ed.focus_body();
        // Move caret after list and insert table.
        let last = ed.document().paragraph_count().saturating_sub(1);
        let len = ed.document().paragraph(last).map(|p| p.char_len()).unwrap_or(0);
        ed.set_selection(Selection::caret(office_core::DocPosition::new(last, len)));
        ed.insert_text("\n").unwrap();
        // Exit list if still in one.
        let _ = ed.toggle_bullet_list();
        ed.insert_table(2, 2).unwrap();

        let layout = layout_document(ui, ed.document(), Pos2::ZERO, 1.0);
        assert!(layout.page_count() >= 1);
        let has_table = layout
            .pages
            .iter()
            .any(|p| p.decorations.iter().any(|d| matches!(d, crate::PageDecoration::Table { .. })));
        assert!(has_table, "expected table decoration in layout");
        let has_marker = layout
            .pages
            .iter()
            .flat_map(|p| &p.lines)
            .any(|l| l.list_marker.is_some());
        assert!(has_marker, "expected list marker on a layout line");
    });
}

#[test]
fn smoke_layout_explicit_page_break() {
    egui::__run_test_ui(|ui| {
        let mut ed = DocumentEditor::default();
        ed.insert_text("Page one").unwrap();
        ed.insert_page_break().unwrap();
        ed.insert_text("Page two").unwrap();
        ed.set_header_text("Hdr").unwrap();
        ed.set_footer_text("Page {page} of {pages}").unwrap();
        let layout = layout_document(ui, ed.document(), Pos2::ZERO, 1.0);
        assert!(
            layout.page_count() >= 2,
            "expected explicit page break to create 2+ pages, got {}",
            layout.page_count()
        );
        assert!(!layout.pages[0].header_lines.is_empty());
        assert!(!layout.pages[0].footer_lines.is_empty());
        // Field expansion: page 1 footer should contain "1".
        let footer_has_digit = layout.pages[0]
            .footer_lines
            .iter()
            .any(|l| l.galley.text().contains('1'));
        assert!(footer_has_digit, "footer should expand {{page}}");
    });
}

#[test]
fn smoke_hit_test_header_band() {
    egui::__run_test_ui(|ui| {
        let mut ed = DocumentEditor::default();
        ed.insert_text("Body").unwrap();
        ed.set_header_text("Hdr").unwrap();
        let layout = layout_document(ui, ed.document(), Pos2::new(10.0, 10.0), 1.0);
        let page = &layout.pages[0];
        let band = page.header_band();
        let hit = layout.hit_test_full(band.center());
        assert!(
            matches!(hit, crate::HitResult::Header(_)),
            "expected header hit, got {hit:?}"
        );
    });
}

#[test]
fn smoke_hit_test_body_and_table() {
    egui::__run_test_ui(|ui| {
        let mut ed = DocumentEditor::default();
        ed.insert_text("Click me").unwrap();
        ed.insert_table(1, 1).unwrap();
        let layout = layout_document(ui, ed.document(), Pos2::new(10.0, 10.0), 1.0);
        assert!(!layout.pages.is_empty());
        let page = &layout.pages[0];
        if let Some(line) = page.lines.first() {
            let hit = layout.hit_test_full(line.rect.center());
            assert!(
                matches!(hit, crate::HitResult::Body(_)),
                "expected body hit, got {hit:?}"
            );
        }
        if let Some(crate::PageDecoration::Table { cells, .. }) = page
            .decorations
            .iter()
            .find(|d| matches!(d, crate::PageDecoration::Table { .. }))
        {
            if let Some(cell) = cells.first() {
                let hit = layout.hit_test_full(cell.rect.center());
                assert!(
                    matches!(hit, crate::HitResult::Cell { .. }),
                    "expected cell hit, got {hit:?}"
                );
            }
        }
    });
}
