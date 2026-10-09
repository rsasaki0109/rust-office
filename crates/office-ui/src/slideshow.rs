//! Transient slide-show navigation, independent of document selection and history.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Navigation {
    Next,
    Previous,
    First,
    Last,
    Exit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SlideShow {
    pub index: usize,
    pub was_fullscreen: bool,
}

impl SlideShow {
    pub fn new(index: usize, count: usize, was_fullscreen: bool) -> Option<Self> {
        (count > 0).then(|| Self {
            index: index.min(count - 1),
            was_fullscreen,
        })
    }

    /// Returns false when presentation should end; endpoints never wrap.
    pub fn navigate(&mut self, navigation: Navigation, count: usize) -> bool {
        if count == 0 || navigation == Navigation::Exit {
            return false;
        }
        self.index = match navigation {
            Navigation::Next => self.index.saturating_add(1).min(count - 1),
            Navigation::Previous => self.index.saturating_sub(1).min(count - 1),
            Navigation::First => 0,
            Navigation::Last => count - 1,
            Navigation::Exit => unreachable!(),
        };
        true
    }
}

pub(crate) fn keyboard_navigation(ctx: &egui::Context) -> Option<Navigation> {
    use egui::{Key, Modifiers};
    ctx.input_mut(|input| {
        for (keys, navigation) in [
            (&[Key::Escape][..], Navigation::Exit),
            (&[Key::Home][..], Navigation::First),
            (&[Key::End][..], Navigation::Last),
            (
                &[Key::ArrowLeft, Key::ArrowUp, Key::PageUp, Key::Backspace][..],
                Navigation::Previous,
            ),
            (
                &[
                    Key::ArrowRight,
                    Key::ArrowDown,
                    Key::PageDown,
                    Key::Space,
                    Key::Enter,
                ][..],
                Navigation::Next,
            ),
        ] {
            if keys
                .iter()
                .any(|&key| input.consume_key(Modifiers::NONE, key))
            {
                return Some(navigation);
            }
        }
        None
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_single_empty_and_invalid_start_do_not_wrap_or_panic() {
        assert!(SlideShow::new(0, 0, false).is_none());
        let mut show = SlideShow::new(100, 3, true).unwrap();
        assert_eq!(show.index, 2);
        assert!(show.navigate(Navigation::Next, 3));
        assert_eq!(show.index, 2);
        assert!(show.navigate(Navigation::First, 3));
        assert!(show.navigate(Navigation::Previous, 3));
        assert_eq!(show.index, 0);
        assert!(show.navigate(Navigation::Last, 3));
        assert_eq!(show.index, 2);
        assert!(!show.navigate(Navigation::Exit, 3));
        assert!(!show.navigate(Navigation::Next, 0));
        let mut single = SlideShow::new(0, 1, false).unwrap();
        for navigation in [
            Navigation::Next,
            Navigation::Previous,
            Navigation::First,
            Navigation::Last,
        ] {
            assert!(single.navigate(navigation, 1));
            assert_eq!(single.index, 0);
        }
    }
}
