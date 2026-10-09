//! Versioned native presentation snapshots, excluding runtime history.
use office_impress::Presentation;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImpressSnapshot {
    version: u32,
    deck: Presentation,
}
impl ImpressSnapshot {
    pub fn capture(presentation: &Presentation) -> Self {
        let mut deck = Presentation::new();
        deck.title = presentation.title.clone();
        deck.theme = presentation.theme.clone();
        deck.slides = presentation.slides.clone();
        deck.active = presentation.active;
        Self { version: 1, deck }
    }
    pub fn into_presentation(self) -> Presentation {
        let mut deck = self.deck;
        deck.mark_recovered();
        deck
    }
}
impl crate::recovery::RecoveryData for ImpressSnapshot {
    const PREFIX: &'static str = "impress-";
    fn validate(&self) -> Result<(), String> {
        if self.version != 1
            || self.deck.slides.is_empty()
            || self.deck.slides.len() > 1000
            || self.deck.active >= self.deck.slides.len()
        {
            return Err("Invalid Impress recovery version, slide count or active slide".into());
        }
        let mut objects = 0usize;
        for slide in &self.deck.slides {
            let count = slide.objects.len().saturating_add(2);
            objects = objects.saturating_add(count);
            if count > 1000 || objects > 10_000 {
                return Err("Impress recovery exceeds object limits".into());
            }
        }
        for font in [self.deck.theme.title_font_pt, self.deck.theme.body_font_pt] {
            if !font.is_finite() || !(1.0..=200.0).contains(&font) {
                return Err("Invalid Impress recovery theme font size".into());
            }
        }
        office_impress::validate_json_presentation(&self.deck).map_err(|e| e.to_string())
    }
    fn decode(bytes: &[u8]) -> Result<Self, String> {
        serde_json::from_slice(bytes).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recovery::{Recovery, RecoveryData};
    use office_impress::{ObjectKind, ShapeKind, SlideObject};

    fn deck() -> Presentation {
        let mut p = Presentation::demo();
        p.title = "日本語 recovery".into();
        p.slides[0].notes = "speaker notes 日本語".into();
        p.slides[0]
            .objects
            .push(SlideObject::shape(ShapeKind::Ellipse));
        let data = vec![
            137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 60, 0, 0, 0, 30,
            8, 2, 0, 0, 0, 255, 250, 234, 24, 0, 0, 0, 59, 73, 68, 65, 84, 120, 156, 237, 206, 81,
            9, 0, 32, 16, 64, 49, 91, 153, 196, 254, 41, 252, 55, 134, 239, 96, 176, 0, 91, 247,
            236, 113, 214, 247, 129, 116, 152, 180, 180, 116, 128, 180, 180, 116, 128, 180, 180,
            116, 128, 180, 180, 116, 128, 180, 180, 116, 192, 200, 244, 3, 53, 221, 147, 159, 23,
            171, 197, 0, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
        ];
        let mut picture = SlideObject::text();
        picture.kind = ObjectKind::Image {
            data: std::sync::Arc::new(data),
        };
        p.slides[0].objects.push(picture);
        p.active = 2;
        p.duplicate_active_slide();
        p
    }
    #[test]
    fn native_objects_images_notes_and_active_slide_survive_two_restarts() {
        let dir = tempfile::tempdir().unwrap();
        let p = deck();
        let expected = serde_json::to_value(&p).unwrap();
        let snapshot = ImpressSnapshot::capture(&p);
        assert!(p.can_undo());
        let mut first = Recovery::new(dir.path()).unwrap();
        first
            .tick(&snapshot, true, "original.pptx", std::time::Instant::now())
            .unwrap();
        assert!(Recovery::<ImpressSnapshot>::new(dir.path())
            .unwrap()
            .entries
            .is_empty());
        drop(first);
        let mut second = Recovery::<ImpressSnapshot>::new(dir.path()).unwrap();
        let restored = second.restore(0).unwrap().into_presentation();
        assert_eq!(serde_json::to_value(&restored).unwrap(), expected);
        assert!(restored.is_dirty() && !restored.can_undo());
        drop(second);
        let mut third = Recovery::<ImpressSnapshot>::new(dir.path()).unwrap();
        assert_eq!(
            serde_json::to_value(third.restore(0).unwrap().into_presentation()).unwrap(),
            expected
        );
        third.clear().unwrap();
    }
    #[test]
    fn invalid_copies_reject_version_selection_geometry_and_image_data() {
        let value = serde_json::to_value(ImpressSnapshot::capture(&deck())).unwrap();
        for case in [
            "version", "active", "empty", "bounds", "font", "image", "unknown",
        ] {
            let mut bad = value.clone();
            match case {
                "version" => bad["version"] = 2.into(),
                "active" => bad["deck"]["active"] = 999.into(),
                "empty" => bad["deck"]["slides"] = serde_json::json!([]),
                "bounds" => bad["deck"]["slides"][0]["title"]["w"] = (-1).into(),
                "font" => bad["deck"]["theme"]["title_font_pt"] = 0.into(),
                "image" => {
                    bad["deck"]["slides"][0]["objects"][1]["kind"]["data"] =
                        serde_json::json!([1, 2, 3])
                }
                _ => bad["future_field"] = true.into(),
            }
            assert!(
                ImpressSnapshot::decode(&serde_json::to_vec(&bad).unwrap())
                    .and_then(|s| s.validate())
                    .is_err(),
                "{case}"
            );
        }
    }
    #[test]
    fn excessive_slides_or_objects_leave_previous_copy_available() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Recovery::new(dir.path()).unwrap();
        let mut snapshot = ImpressSnapshot::capture(&deck());
        let now = std::time::Instant::now();
        store.tick(&snapshot, true, "old.json", now).unwrap();
        snapshot.deck.slides = vec![office_impress::Slide::blank(); 1001];
        assert!(store
            .tick(&snapshot, true, "old.json", now + crate::recovery::INTERVAL)
            .is_err());
        snapshot.deck.slides = vec![office_impress::Slide::blank(); 1];
        snapshot.deck.active = 0;
        snapshot.deck.slides[0].objects = vec![SlideObject::text(); 999];
        assert!(snapshot.validate().is_err());
        drop(store);
        let mut next = Recovery::<ImpressSnapshot>::new(dir.path()).unwrap();
        assert_eq!(next.restore(0).unwrap().deck.title, "日本語 recovery");
    }
    #[test]
    fn undo_to_recovered_baseline_stays_dirty_until_saved() {
        let mut p = ImpressSnapshot::capture(&deck()).into_presentation();
        p.add_slide();
        assert!(p.undo());
        assert!(p.is_dirty());
        p.mark_clean();
        assert!(!p.is_dirty());
        assert!(p.redo());
        assert!(p.is_dirty());
        assert!(p.undo());
        assert!(!p.is_dirty());
    }
}
