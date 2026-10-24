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
    /// Toggle the local zoom of the view.
    LocalZoom,
    /// Switch to the eraser while held (hold trigger only).
    Eraser,
    /// Disabled.
    None,
}

/// Actions for every barrel-button trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ButtonMap {
    /// Press and release in hover.
    pub click: ButtonAction,
    /// Second press shortly after a click.
    pub double_click: ButtonAction,
    /// Held still in hover for the double-click time.
    pub hold: ButtonAction,
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
        let (click, double_click, hold, hold_drag_contact, hold_drag_hover) = match p {
            Profile::Raw | Profile::Custom => (Raw, LocalZoom, Raw, Raw, Raw),
            Profile::Desktop => (RightClick, LocalZoom, None, Scroll, PcZoom),
            Profile::Drawing => (Raw, LocalZoom, Eraser, Raw, Raw),
        };
        ButtonMap {
            click,
            double_click,
            hold,
            hold_drag_contact,
            hold_drag_hover,
        }
    }

    /// Replaces actions that are invalid for their trigger by `None`.
    ///
    /// Allowed: `click`: Raw, RightClick, MiddleClick, LocalZoom, None;
    /// `double_click`: RightClick, MiddleClick, LocalZoom, None (Raw becomes
    /// `None`, since a raw double click is two raw clicks); `hold`: as `click`
    /// plus Eraser; drag triggers: Raw, Scroll, PcZoom, None.
    fn sanitized(self) -> ButtonMap {
        use ButtonAction::*;
        let click_like = |a: ButtonAction| match a {
            Raw | RightClick | MiddleClick | LocalZoom | None => a,
            _ => None,
        };
        let hold = |a: ButtonAction| match a {
            Eraser => a,
            _ => click_like(a),
        };
        let drag = |a: ButtonAction| match a {
            Raw | Scroll | PcZoom | None => a,
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
            hold_drag_contact: drag(self.hold_drag_contact),
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
    /// Local zoom factor, clamped to 1.25..=4.0 on use.
    pub zoom_factor: f32,
    /// Double-click / hold time, clamped to 150..=800 on use.
    pub double_click_ms: u32,
    /// Movement tolerance in view pixels.
    pub slop_px: f32,
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
            zoom_factor: 2.0,
            double_click_ms: 300,
            slop_px: 24.0,
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

    /// The effective, sanitized button map for the selected profile.
    pub fn button_map(&self) -> ButtonMap {
        match self.profile {
            Profile::Custom => self.custom.sanitized(),
            p => ButtonMap::for_profile(p),
        }
    }
}

/// Config values resolved and clamped for use by the engine.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Params {
    pub map: ButtonMap,
    pub double_us: u64,
    pub slop: f32,
    pub zoom: f32,
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
            double_us: u64::from(c.double_click_ms.clamp(150, 800)) * 1000,
            slop: fin(c.slop_px, 24.0).max(0.0),
            zoom: fin(c.zoom_factor, 2.0).clamp(1.25, 4.0),
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
