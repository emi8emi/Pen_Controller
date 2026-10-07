//! The overlay's show/hide rules as a pure state machine.
//!
//! The shell (window code) reports what happened (pen came into range, a real mouse moved, a hotkey was
//! pressed) and gets back what to do, if anything. All decisions live here, so they can be tested without
//! a window, a pen or Windows.
//!
//! Rules (the same as the Tauri overlay's README):
//!   * The pen shows the overlay, unless auto-show is off, it is already up, or an ignored app has focus.
//!   * A real mouse move hides it, unless it was shown by hand (pinned).
//!   * Showing it by hand pins it. Showing by hand while it is already up pins it too.
//!   * Only a pinned overlay may take keyboard focus, and never in hands-off mode.
//!   * Auto-show and hands-off are never saved: a new `Lifecycle` always starts auto-on, hands-off-off.

/// What the shell should do now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Show { pinned: bool },
    Hide,
}

pub struct Lifecycle {
    visible: bool,
    pinned: bool,
    auto_show: bool,
    hands_off: bool,
    ignored: Vec<String>,
}

impl Lifecycle {
    /// `ignored`: program file names (e.g. "krita.exe") the pen must not summon the overlay over.
    pub fn new(ignored: &[&str]) -> Self {
        Lifecycle {
            visible: false,
            pinned: false,
            auto_show: true,
            hands_off: false,
            ignored: ignored.iter().map(|s| s.to_ascii_lowercase()).collect(),
        }
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }
    pub fn is_pinned(&self) -> bool {
        self.pinned
    }
    pub fn auto_show(&self) -> bool {
        self.auto_show
    }
    pub fn hands_off(&self) -> bool {
        self.hands_off
    }

    /// Whether the overlay window may take keyboard focus right now.
    pub fn can_take_focus(&self) -> bool {
        self.visible && self.pinned && !self.hands_off
    }

    /// The pen came into range or touched down. `foreground` is only called when the answer could matter
    /// (looking up the focused program is not free).
    pub fn pen_summon(&mut self, foreground: impl FnOnce() -> Option<String>) -> Option<Action> {
        if !self.auto_show || self.visible {
            return None;
        }
        if let Some(exe) = foreground() {
            if self.ignored.iter().any(|a| a.eq_ignore_ascii_case(&exe)) {
                return None;
            }
        }
        self.visible = true;
        self.pinned = false;
        Some(Action::Show { pinned: false })
    }

    /// A real mouse moved (pen-driven cursor movement must not be reported here).
    pub fn mouse_moved(&mut self) -> Option<Action> {
        if !self.visible || self.pinned {
            return None;
        }
        self.visible = false;
        Some(Action::Hide)
    }

    /// Show by hand (hotkey, tray, a client asking for it). Pins the overlay.
    pub fn show_pinned(&mut self) -> Option<Action> {
        if self.visible {
            self.pinned = true;
            return None;
        }
        self.visible = true;
        self.pinned = true;
        Some(Action::Show { pinned: true })
    }

    /// Hide by hand (Esc, the Hide button, the hotkey).
    pub fn hide(&mut self) -> Option<Action> {
        if !self.visible {
            return None;
        }
        self.visible = false;
        self.pinned = false;
        Some(Action::Hide)
    }

    /// The show/hide hotkey: hides if it is up, otherwise shows it pinned.
    pub fn toggle(&mut self) -> Option<Action> {
        if self.visible {
            self.hide()
        } else {
            self.show_pinned()
        }
    }

    pub fn set_auto_show(&mut self, on: bool) {
        self.auto_show = on;
    }

    pub fn set_hands_off(&mut self, off: bool) {
        self.hands_off = off;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lc() -> Lifecycle {
        Lifecycle::new(&["krita.exe", "Photoshop.exe"])
    }

    fn nothing() -> Option<String> {
        None
    }

    #[test]
    fn starts_hidden_with_auto_on_and_hands_off_off() {
        let l = lc();
        assert!(!l.is_visible() && !l.is_pinned());
        assert!(l.auto_show());
        assert!(!l.hands_off());
    }

    #[test]
    fn the_pen_shows_it_and_a_mouse_move_hides_it() {
        let mut l = lc();
        assert_eq!(l.pen_summon(nothing), Some(Action::Show { pinned: false }));
        assert!(l.is_visible() && !l.is_pinned());
        assert_eq!(l.mouse_moved(), Some(Action::Hide));
        assert!(!l.is_visible());
    }

    #[test]
    fn a_second_summon_while_visible_does_nothing() {
        let mut l = lc();
        l.pen_summon(nothing);
        assert_eq!(l.pen_summon(nothing), None);
    }

    #[test]
    fn mouse_moves_while_hidden_do_nothing() {
        let mut l = lc();
        assert_eq!(l.mouse_moved(), None);
    }

    #[test]
    fn ignored_programs_block_the_pen_case_insensitively() {
        let mut l = lc();
        assert_eq!(l.pen_summon(|| Some("KRITA.EXE".to_string())), None);
        assert!(!l.is_visible());
        assert_eq!(l.pen_summon(|| Some("notepad.exe".to_string())), Some(Action::Show { pinned: false }));
    }

    #[test]
    fn the_foreground_lookup_is_skipped_when_it_cannot_matter() {
        let mut l = lc();
        l.set_auto_show(false);
        let mut called = false;
        assert_eq!(
            l.pen_summon(|| {
                called = true;
                None
            }),
            None
        );
        assert!(!called, "no lookup while auto-show is off");

        l.set_auto_show(true);
        l.pen_summon(nothing);
        let mut called = false;
        l.pen_summon(|| {
            called = true;
            None
        });
        assert!(!called, "no lookup while already visible");
    }

    #[test]
    fn auto_show_off_stops_the_pen_but_not_the_hotkey() {
        let mut l = lc();
        l.set_auto_show(false);
        assert_eq!(l.pen_summon(nothing), None);
        assert_eq!(l.toggle(), Some(Action::Show { pinned: true }));
    }

    #[test]
    fn a_pinned_overlay_ignores_the_mouse() {
        let mut l = lc();
        assert_eq!(l.toggle(), Some(Action::Show { pinned: true }));
        assert_eq!(l.mouse_moved(), None);
        assert!(l.is_visible());
        assert_eq!(l.toggle(), Some(Action::Hide));
        assert!(!l.is_visible() && !l.is_pinned());
    }

    #[test]
    fn showing_by_hand_while_the_pen_has_it_up_pins_it() {
        let mut l = lc();
        l.pen_summon(nothing);
        assert_eq!(l.show_pinned(), None); // already up: nothing to do for the window...
        assert!(l.is_pinned()); // ...but the mouse can no longer hide it
        assert_eq!(l.mouse_moved(), None);
    }

    #[test]
    fn hiding_clears_the_pin() {
        let mut l = lc();
        l.show_pinned();
        assert_eq!(l.hide(), Some(Action::Hide));
        assert_eq!(l.hide(), None);
        // the next pen summon is unpinned again
        assert_eq!(l.pen_summon(nothing), Some(Action::Show { pinned: false }));
        assert_eq!(l.mouse_moved(), Some(Action::Hide));
    }

    #[test]
    fn only_a_pinned_overlay_can_take_focus_and_never_in_hands_off() {
        let mut l = lc();
        assert!(!l.can_take_focus());
        l.pen_summon(nothing);
        assert!(!l.can_take_focus(), "a pen summon never takes focus");
        l.show_pinned();
        assert!(l.can_take_focus());
        l.set_hands_off(true);
        assert!(!l.can_take_focus(), "hands-off: never");
        l.set_hands_off(false);
        assert!(l.can_take_focus());
    }
}
