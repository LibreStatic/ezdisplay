//! Engine configuration (deserialized from JSON sent by Kotlin).

use serde::{Deserialize, Serialize};

/// Button-gesture preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    /// Barrel button forwarded as is (double click toggles local zoom).
    #[default]
    Raw,
    /// Right click, scroll and PC zoom with the barrel button.
    Desktop,
    /// Hold for eraser, otherwise raw.
    Drawing,
    /// Use [`InputConfig::custom`].
    Custom,
    /// Graphics-tablet style: hold presses the key combination, click right-clicks,
    /// double click toggles the local zoom.
    GraphicsTablet,
}

/// What a barrel-button trigger does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ButtonAction {
    /// Forward the barrel button to the host untouched.
    #[default]
    Raw,
    /// Send a right click.
    RightClick,
    /// Send a middle click.
    MiddleClick,
    /// Scroll the host (drag triggers only).
    Scroll,
    /// Zoom the host with Ctrl + wheel (drag triggers only).
    PcZoom,
    /// Zoom a shortcut-and-click zoom in steps (drag triggers only): every
    /// [`InputConfig::zoom_step_px`] of vertical pen travel taps
    /// [`KeyBindings::zoom_in`] (up) or [`KeyBindings::zoom_out`] (down) with a
    /// left click at the point where the drag started.
    StepZoom,
    /// Zoom the host with the plain mouse wheel in steps (drag triggers only):
    /// every [`InputConfig::zoom_step_px`] of vertical pen travel sends one
    /// wheel notch, up = zoom in, down = zoom out.
    WheelZoom,
    /// Pan the canvas of the host (contact drag only): the pan combination
    /// ([`KeyBindings::pan`], Space by default) is kept pressed while the tip
    /// touches, so a drag with the pen pans in many drawing apps.
    Pan,
    /// Toggle the local zoom of the view.
    LocalZoom,
    /// Switch to the eraser while held (hold trigger only).
    Eraser,
    /// Press a key combination ([`InputConfig::keys`]).
    Keys,
    /// Disabled.
    None,
}

/// A key combination: modifiers plus an optional key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct KeyCombo {
    /// Bit set of [`KeyCombo::CTRL`], [`SHIFT`](Self::SHIFT), [`ALT`](Self::ALT),
    /// [`META`](Self::META).
    pub mods: u8,
    /// HID keyboard-page usage id of the key, 0 = modifiers only.
    pub key: u16,
}

impl KeyCombo {
    /// Modifier: Ctrl.
    pub const CTRL: u8 = 1;
    /// Modifier: Shift.
    pub const SHIFT: u8 = 2;
    /// Modifier: Alt.
    pub const ALT: u8 = 4;
    /// Modifier: Meta (Windows / Super).
    pub const META: u8 = 8;

    /// No modifiers and no key.
    pub fn is_empty(&self) -> bool {
        self.mods == 0 && self.key == 0
    }

    /// Masks unknown modifier bits; a `key` that is not a letter, digit,
    /// punctuation, function or navigation usage (0x04..=0x73) or a modifier
    /// usage (0xE0..=0xE7) becomes 0.
    pub fn sanitized(self) -> KeyCombo {
        let valid = matches!(self.key, 0x04..=0x73 | 0xE0..=0xE7);
        KeyCombo {
            mods: self.mods & 0x0F,
            key: if valid { self.key } else { 0 },
        }
    }
}

/// Key combinations of the [`ButtonAction::Keys`] action, per trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeyBindings {
    /// Tapped on a click.
    pub click: KeyCombo,
    /// Tapped on a double click.
    pub double_click: KeyCombo,
    /// Kept pressed while the barrel button is held.
    pub hold: KeyCombo,
    /// Kept pressed while the barrel button is held after a double click.
    pub double_hold: KeyCombo,
    /// Tapped with a click for every upward step of [`ButtonAction::StepZoom`].
    pub zoom_in: KeyCombo,
    /// Tapped with a click for every downward step of [`ButtonAction::StepZoom`].
    pub zoom_out: KeyCombo,
    /// Kept pressed while the tip touches during [`ButtonAction::Pan`].
    pub pan: KeyCombo,
}

impl Default for KeyBindings {
    fn default() -> Self {
        KeyBindings {
            click: KeyCombo::default(),
            double_click: KeyCombo::default(),
            // Ctrl + Space: a common drag-zoom shortcut in drawing apps.
            hold: KeyCombo {
                mods: KeyCombo::CTRL,
                key: 0x2C,
            },
            // Ctrl + Alt + Space: a common zoom-out shortcut (with a click) in drawing apps.
            double_hold: KeyCombo {
                mods: KeyCombo::CTRL | KeyCombo::ALT,
                key: 0x2C,
            },
            zoom_in: KeyCombo {
                mods: KeyCombo::CTRL,
                key: 0x2C,
            },
            zoom_out: KeyCombo {
                mods: KeyCombo::CTRL | KeyCombo::ALT,
                key: 0x2C,
            },
            // Space: the usual pan modifier (with a drag) in drawing apps.
            pan: KeyCombo { mods: 0, key: 0x2C },
        }
    }
}

impl KeyBindings {
    fn sanitized(self) -> KeyBindings {
        KeyBindings {
            click: self.click.sanitized(),
            double_click: self.double_click.sanitized(),
            hold: self.hold.sanitized(),
            double_hold: self.double_hold.sanitized(),
            zoom_in: self.zoom_in.sanitized(),
            zoom_out: self.zoom_out.sanitized(),
            pan: self.pan.sanitized(),
        }
    }
}

/// Actions for every barrel-button trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ButtonMap {
    /// Press and release in hover.
    pub click: ButtonAction,
    /// Second press shortly after a click.
    pub double_click: ButtonAction,
    /// Held still in hover for the double-click time. If a drag action (scroll
    /// or zoom) is set for the contact or hover drag, the hold is deferred: a
    /// drag started afterwards wins, otherwise the hold fires at the next
    /// motion, tip touch or barrel release. A Keys hold takes precedence over
    /// the drags.
    pub hold: ButtonAction,
    /// Second press held still in hover for the double-click time, or resolved
    /// by the tip touching or the pen moving. Eraser, Keys or None. Only
    /// reachable when the click and double click are both enabled and differ.
    /// While it is not `None`, the double click fires at the release of the
    /// second press instead of at the press.
    pub double_hold: ButtonAction,
    /// Held while the tip touches and drags.
    pub hold_drag_contact: ButtonAction,
    /// Held while the pen moves in hover.
    pub hold_drag_hover: ButtonAction,
}

impl Default for ButtonMap {
    fn default() -> Self {
        ButtonMap::for_profile(Profile::Raw)
    }
}

impl ButtonMap {
    /// The map of a preset profile. [`Profile::Custom`] gives the Raw map.
    pub fn for_profile(p: Profile) -> ButtonMap {
        use ButtonAction::*;
        let (click, double_click, hold, double_hold, hold_drag_contact, hold_drag_hover) = match p {
            Profile::Raw | Profile::Custom => (Raw, LocalZoom, Raw, None, Raw, Raw),
            Profile::Desktop => (RightClick, LocalZoom, None, None, Scroll, PcZoom),
            Profile::Drawing => (Raw, LocalZoom, Eraser, None, Raw, Raw),
            // The drag entries are unused: a Keys hold takes precedence.
            Profile::GraphicsTablet => (RightClick, LocalZoom, Keys, Keys, Raw, Raw),
        };
        ButtonMap {
            click,
            double_click,
            hold,
            double_hold,
            hold_drag_contact,
            hold_drag_hover,
        }
    }

    /// Replaces actions that are invalid for their trigger by `None`.
    ///
    /// Allowed: `click`: Raw, RightClick, MiddleClick, LocalZoom, Keys, None;
    /// `double_click`: RightClick, MiddleClick, LocalZoom, Keys, None (Raw
    /// becomes `None`, since a raw double click is two raw clicks); `hold`: as
    /// `click` plus Eraser; `double_hold`: Eraser, Keys, None; drag triggers: Raw, Scroll, PcZoom,
    /// StepZoom, WheelZoom, None; Pan is also allowed for the contact drag only.
    fn sanitized(self) -> ButtonMap {
        use ButtonAction::*;
        let click_like = |a: ButtonAction| match a {
            Raw | RightClick | MiddleClick | LocalZoom | Keys | None => a,
            _ => None,
        };
        let hold = |a: ButtonAction| match a {
            Eraser => a,
            _ => click_like(a),
        };
        let drag = |a: ButtonAction| match a {
            Raw | Scroll | PcZoom | StepZoom | WheelZoom | None => a,
            _ => None,
        };
        ButtonMap {
            click: click_like(self.click),
            // A raw double click is two raw clicks, which `None` already gives.
            double_click: match self.double_click {
                ButtonAction::Raw => ButtonAction::None,
                a => click_like(a),
            },
            hold: hold(self.hold),
            double_hold: match self.double_hold {
                Eraser | Keys => self.double_hold,
                _ => None,
            },
            hold_drag_contact: match self.hold_drag_contact {
                Pan => Pan,
                a => drag(a),
            },
            hold_drag_hover: drag(self.hold_drag_hover),
        }
    }
}

/// Input engine configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InputConfig {
    /// Gesture preset.
    pub profile: Profile,
    /// Map used when `profile == Custom`.
    pub custom: ButtonMap,
    /// Combinations used by triggers whose action is [`ButtonAction::Keys`].
    pub keys: KeyBindings,
    /// Local zoom factor, clamped to 1.25..=4.0 on use.
    pub zoom_factor: f32,
    /// Double-click / hold time, clamped to 150..=800 on use.
    pub double_click_ms: u32,
    /// Movement tolerance in view pixels.
    pub slop_px: f32,
    /// View pixels of vertical pen travel per [`ButtonAction::StepZoom`] step,
    /// clamped to 20..=400 on use.
    pub zoom_step_px: f32,
    /// View pixels of pen travel per scroll notch (120 units).
    pub scroll_px_per_notch: f32,
    /// Reject fingers while the pen is near.
    pub palm_rejection: bool,
    /// Milliseconds fingers stay rejected after the pen left range.
    pub palm_grace_ms: u32,
    /// Milliseconds a hover exit is delayed before the pen is reported out of range.
    pub hover_exit_delay_ms: u32,
}

impl Default for InputConfig {
    fn default() -> Self {
        InputConfig {
            profile: Profile::Raw,
            custom: ButtonMap::default(),
            keys: KeyBindings::default(),
            zoom_factor: 2.0,
            double_click_ms: 300,
            slop_px: 24.0,
            zoom_step_px: 80.0,
            scroll_px_per_notch: 48.0,
            palm_rejection: true,
            palm_grace_ms: 300,
            hover_exit_delay_ms: 40,
        }
    }
}

impl InputConfig {
    /// Parses a JSON config. Unknown fields are ignored; unknown enum strings
    /// are an error (the caller keeps its old config).
    pub fn from_json(s: &str) -> Result<Self, String> {
        serde_json::from_str(s).map_err(|e| e.to_string())
    }

    /// The effective, sanitized button map for the selected profile. A Keys
    /// trigger whose (sanitized) combination is empty becomes `None`.
    pub fn button_map(&self) -> ButtonMap {
        let mut m = match self.profile {
            Profile::Custom => self.custom.sanitized(),
            p => ButtonMap::for_profile(p),
        };
        let k = self.keys.sanitized();
        for (a, combo) in [
            (&mut m.click, k.click),
            (&mut m.double_click, k.double_click),
            (&mut m.hold, k.hold),
            (&mut m.double_hold, k.double_hold),
        ] {
            if *a == ButtonAction::Keys && combo.is_empty() {
                *a = ButtonAction::None;
            }
        }
        // A step zoom needs at least one direction to press.
        if k.zoom_in.is_empty() && k.zoom_out.is_empty() {
            for a in [&mut m.hold_drag_contact, &mut m.hold_drag_hover] {
                if *a == ButtonAction::StepZoom {
                    *a = ButtonAction::None;
                }
            }
        }
        // A pan needs a combination to press.
        if k.pan.is_empty() && m.hold_drag_contact == ButtonAction::Pan {
            m.hold_drag_contact = ButtonAction::None;
        }
        m
    }
}

/// Config values resolved and clamped for use by the engine.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Params {
    pub map: ButtonMap,
    pub keys: KeyBindings,
    pub double_us: u64,
    pub slop: f32,
    pub zoom: f32,
    pub zoom_step: f32,
    pub scroll_n: f32,
    pub palm: bool,
    pub grace_us: u64,
    pub exit_us: u64,
}

impl Params {
    pub fn from_config(c: &InputConfig) -> Params {
        let fin = |v: f32, d: f32| if v.is_finite() { v } else { d };
        Params {
            map: c.button_map(),
            keys: c.keys.sanitized(),
            double_us: u64::from(c.double_click_ms.clamp(150, 800)) * 1000,
            slop: fin(c.slop_px, 24.0).max(0.0),
            zoom: fin(c.zoom_factor, 2.0).clamp(1.25, 4.0),
            zoom_step: fin(c.zoom_step_px, 80.0).clamp(20.0, 400.0),
            scroll_n: {
                let n = fin(c.scroll_px_per_notch, 48.0);
                if n > 0.0 { n } else { 48.0 }
            },
            palm: c.palm_rejection,
            grace_us: u64::from(c.palm_grace_ms) * 1000,
            exit_us: u64::from(c.hover_exit_delay_ms) * 1000,
        }
    }
}
