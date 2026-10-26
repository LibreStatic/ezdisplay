//! Decisions for keeping the Windows touch keyboard from popping up while the client has a
//! hardware keyboard attached.
//!
//! Windows opens the touch keyboard when a text field is tapped, according to the per-user
//! DWORD `TouchKeyboardTapInvoke` (0 = never, 1 = when no keyboard is attached, 2 = always; the
//! value may be absent, which means the OS default). Keys injected by us do not count as an
//! attached keyboard, so while the client reports one we set the value to 0 and restore the
//! original afterwards. This module is the pure state machine and the encoding of the crash
//! marker; the registry access lives in `win::touch_keyboard`.

/// Key (under HKCU) that holds `TouchKeyboardTapInvoke`.
pub const TABLET_TIP_KEY: &str = r"Software\Microsoft\TabletTip\1.7";
/// The DWORD that decides when a tap opens the touch keyboard.
pub const TAP_INVOKE_VALUE: &str = "TouchKeyboardTapInvoke";
/// Our own key (under HKCU).
pub const OWN_KEY: &str = r"Software\LibreStatic\ezdisplay";
/// REG_SZ under [`OWN_KEY`] with the original value while we override it, see [`encode_saved`].
pub const SAVED_VALUE: &str = "SavedTouchKeyboardTapInvoke";
/// The value we write while a hardware keyboard is attached ("never").
pub const TAP_INVOKE_NEVER: u32 = 0;

const ABSENT: &str = "absent";

/// The marker text for an original value (`None` = the value did not exist).
pub fn encode_saved(original: Option<u32>) -> String {
    match original {
        Some(v) => v.to_string(),
        None => ABSENT.to_string(),
    }
}

/// Reads a marker written by [`encode_saved`]; `None` if it is malformed.
pub fn decode_saved(s: &str) -> Option<Option<u32>> {
    let s = s.trim();
    if s == ABSENT {
        Some(None)
    } else {
        s.parse().ok().map(Some)
    }
}

/// Tracks whether we currently override the setting and what to put back.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct TouchKeyboardGuard {
    /// `Some(original)` while overridden (`original` = `None` if the value was absent).
    overridden: Option<Option<u32>>,
}

impl TouchKeyboardGuard {
    /// A guard that overrides nothing.
    pub fn new() -> Self {
        Self::default()
    }

    /// True while the setting is overridden.
    pub fn is_overridden(&self) -> bool {
        self.overridden.is_some()
    }

    /// The client reports a hardware keyboard. `current` is the present registry value.
    ///
    /// Returns the original to persist before writing [`TAP_INVOKE_NEVER`], or `None` when
    /// nothing has to be done (already overridden, or the user already chose "never").
    pub fn attach(&mut self, current: Option<u32>) -> Option<Option<u32>> {
        if self.overridden.is_some() || current == Some(TAP_INVOKE_NEVER) {
            return None;
        }
        self.overridden = Some(current);
        Some(current)
    }

    /// The keyboard went away, the session ended or the sink is dropped.
    ///
    /// Returns the original to restore (`None` original = delete the value), or `None` when
    /// nothing is overridden. Calling it again returns `None`.
    pub fn detach(&mut self) -> Option<Option<u32>> {
        self.overridden.take()
    }

    /// Puts the guard back in the overridden state, to retry a restore that failed.
    pub fn restore_failed(&mut self, original: Option<u32>) {
        self.overridden = Some(original);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attach_saves_the_original_once() {
        let mut g = TouchKeyboardGuard::new();
        assert_eq!(g.attach(Some(1)), Some(Some(1)));
        assert!(g.is_overridden());
        // The registry now holds 0, a repeated attach must not overwrite the saved original.
        assert_eq!(g.attach(Some(0)), None);
        assert_eq!(g.detach(), Some(Some(1)));
        assert!(!g.is_overridden());
        assert_eq!(g.detach(), None, "restoring twice does nothing");
    }

    #[test]
    fn absent_value_is_restored_as_absent() {
        let mut g = TouchKeyboardGuard::new();
        assert_eq!(g.attach(None), Some(None));
        assert_eq!(g.detach(), Some(None));
    }

    #[test]
    fn user_choice_of_never_is_left_alone() {
        let mut g = TouchKeyboardGuard::new();
        assert_eq!(g.attach(Some(0)), None);
        assert!(!g.is_overridden());
        assert_eq!(g.detach(), None);
    }

    #[test]
    fn detach_without_attach_is_a_no_op() {
        assert_eq!(TouchKeyboardGuard::new().detach(), None);
    }

    #[test]
    fn can_attach_again_after_detach() {
        let mut g = TouchKeyboardGuard::new();
        g.attach(Some(2));
        g.detach();
        assert_eq!(g.attach(Some(2)), Some(Some(2)));
    }

    #[test]
    fn failed_restore_can_be_retried() {
        let mut g = TouchKeyboardGuard::new();
        g.attach(Some(1));
        let o = g.detach().unwrap();
        g.restore_failed(o);
        assert_eq!(g.detach(), Some(Some(1)));
    }

    #[test]
    fn marker_roundtrip() {
        for v in [None, Some(0), Some(1), Some(2), Some(u32::MAX)] {
            assert_eq!(decode_saved(&encode_saved(v)), Some(v));
        }
        assert_eq!(decode_saved("garbage"), None);
        assert_eq!(decode_saved(""), None);
        assert_eq!(decode_saved(" absent "), Some(None));
    }
}
