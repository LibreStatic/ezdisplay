//! Input injection on Windows: synthetic pen and touch pointer devices
//! (`CreateSyntheticPointerDevice` / `InjectSyntheticPointerInput`, Windows 10 1809+) and
//! `SendInput` for pointer buttons, scrolling and the mouse pen mode.
//!
//! The decisions (which frame, which flags) live in [`crate::input_map`]; this module only
//! converts them into Win32 structs and injects them.

use super::display;
use super::util::err_str;
use crate::input_map::{
    self, KeyTracker, Modifier, MouseButton, MouseOrKey, MousePenTracker, PenFrame, PenTracker,
    RateLimit, Rect, TouchContactOut, TouchTracker, mask,
};
use ezd_host_core::{InputBackend, InputSink, InputTarget, PenMode};
use ezd_proto::{InputStatus, Key, PenSample, PointerButton, Scroll, TouchFrame};
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{E_INVALIDARG, POINT, RECT};
use windows::Win32::UI::Controls::{
    CreateSyntheticPointerDevice, DestroySyntheticPointerDevice, HSYNTHETICPOINTERDEVICE,
    POINTER_FEEDBACK_DEFAULT, POINTER_FEEDBACK_MODE, POINTER_FEEDBACK_NONE, POINTER_TYPE_INFO,
    POINTER_TYPE_INFO_0,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MOUSE_EVENT_FLAGS,
    MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN,
    MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEINPUT, SendInput,
    VIRTUAL_KEY, VK_CONTROL, VK_SHIFT,
};
use windows::Win32::UI::Input::Pointer::{
    InjectSyntheticPointerInput, POINTER_FLAGS, POINTER_INFO, POINTER_PEN_INFO, POINTER_TOUCH_INFO,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, POINTER_INPUT_TYPE, PT_PEN, PT_TOUCH, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
    SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

/// Contacts of the synthetic pen device.
const PEN_CONTACTS: u32 = 1;
/// How often the monitor rectangle is looked up again while nothing forces it.
const RECT_REFRESH: Duration = Duration::from_secs(1);
/// Minimum interval between two warnings of the same kind.
const WARN_EVERY: Duration = Duration::from_secs(5);

/// An owned synthetic pointer device.
struct Device(HSYNTHETICPOINTERDEVICE);

// SAFETY: the handle is an opaque token of the calling process. The sink that owns it is
// created and used only on the session's input thread; it is `Send` only because
// `Box<dyn InputSink>` must be.
unsafe impl Send for Device {}

impl Device {
    fn create(
        kind: POINTER_INPUT_TYPE,
        max_contacts: u32,
        feedback: POINTER_FEEDBACK_MODE,
    ) -> Result<Device, String> {
        // SAFETY: plain Win32 call without pointers.
        unsafe { CreateSyntheticPointerDevice(kind, max_contacts, feedback) }
            .map(Device)
            .map_err(|e| err_str(&e))
    }

    fn pen() -> Result<Device, String> {
        Device::create(PT_PEN, PEN_CONTACTS, POINTER_FEEDBACK_DEFAULT)
    }

    fn touch() -> Result<Device, String> {
        Device::create(PT_TOUCH, input_map::MAX_TOUCH as u32, POINTER_FEEDBACK_NONE)
    }

    fn inject(&self, infos: &[POINTER_TYPE_INFO]) -> windows::core::Result<()> {
        // SAFETY: `self.0` is a live device and `infos` a valid slice of initialised structs.
        unsafe { InjectSyntheticPointerInput(self.0, infos) }
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        // SAFETY: the handle was created by `CreateSyntheticPointerDevice` and is destroyed once.
        unsafe { DestroySyntheticPointerDevice(self.0) }
    }
}

/// The `input` section of `ezd-host diag`: whether both synthetic devices can be created.
pub fn diag_json() -> Value {
    let show = |r: Result<Device, String>| match r {
        Ok(_) => json!("ok"),
        Err(e) => json!(e),
    };
    json!({
        "pen": show(Device::pen()),
        "touch": show(Device::touch()),
        "note": "needs Windows 10 1809+; input to elevated (administrator) windows is blocked unless the host runs elevated",
    })
}

fn pointer_info(kind: POINTER_INPUT_TYPE, id: u32, flags: u32, pos: (i32, i32)) -> POINTER_INFO {
    POINTER_INFO {
        pointerType: kind,
        pointerId: id,
        pointerFlags: POINTER_FLAGS(flags),
        ptPixelLocation: POINT { x: pos.0, y: pos.1 },
        ..Default::default()
    }
}

fn pen_info(f: &PenFrame, screen: Rect) -> POINTER_TYPE_INFO {
    let pos = input_map::to_screen(screen, f.x, f.y);
    POINTER_TYPE_INFO {
        r#type: PT_PEN,
        Anonymous: POINTER_TYPE_INFO_0 {
            penInfo: POINTER_PEN_INFO {
                pointerInfo: pointer_info(PT_PEN, 0, f.flags, pos),
                penFlags: f.pen_flags,
                penMask: mask::PEN_PRESSURE | mask::PEN_TILT_X | mask::PEN_TILT_Y,
                pressure: f.pressure,
                rotation: 0,
                tiltX: f.tilt_x,
                tiltY: f.tilt_y,
            },
        },
    }
}

fn touch_info(c: &TouchContactOut, screen: Rect) -> POINTER_TYPE_INFO {
    let pos = input_map::to_screen(screen, c.x, c.y);
    let a = input_map::TOUCH_HALF_AREA;
    POINTER_TYPE_INFO {
        r#type: PT_TOUCH,
        Anonymous: POINTER_TYPE_INFO_0 {
            touchInfo: POINTER_TOUCH_INFO {
                pointerInfo: pointer_info(PT_TOUCH, u32::from(c.id), c.flags, pos),
                touchFlags: 0,
                touchMask: mask::TOUCH_CONTACTAREA | mask::TOUCH_ORIENTATION | mask::TOUCH_PRESSURE,
                rcContact: RECT {
                    left: pos.0 - a,
                    top: pos.1 - a,
                    right: pos.0 + a,
                    bottom: pos.1 + a,
                },
                orientation: input_map::TOUCH_ORIENTATION,
                pressure: input_map::TOUCH_PRESSURE,
                ..Default::default()
            },
        },
    }
}

fn inject_touches(
    dev: &Device,
    screen: Rect,
    contacts: &[TouchContactOut],
) -> windows::core::Result<()> {
    let infos: Vec<_> = contacts.iter().map(|c| touch_info(c, screen)).collect();
    dev.inject(&infos)
}

/// The virtual screen (all monitors) in desktop pixels.
fn virtual_screen() -> Rect {
    // SAFETY: plain Win32 calls without pointers.
    unsafe {
        Rect {
            x: GetSystemMetrics(SM_XVIRTUALSCREEN),
            y: GetSystemMetrics(SM_YVIRTUALSCREEN),
            w: GetSystemMetrics(SM_CXVIRTUALSCREEN),
            h: GetSystemMetrics(SM_CYVIRTUALSCREEN),
        }
    }
}

fn mouse_input(dx: i32, dy: i32, data: u32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: data,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn key_input(key: Modifier, down: bool) -> INPUT {
    let vk = match key {
        Modifier::Ctrl => VK_CONTROL,
        Modifier::Shift => VK_SHIFT,
    };
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if down {
                    KEYBD_EVENT_FLAGS(0)
                } else {
                    KEYEVENTF_KEYUP
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn scan_input(scan: u16, extended: bool, down: bool) -> INPUT {
    let mut flags = KEYEVENTF_SCANCODE;
    if extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if !down {
        flags |= KEYEVENTF_KEYUP;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn to_input(ev: &MouseOrKey, virt: Rect) -> INPUT {
    match *ev {
        MouseOrKey::Move { x, y } => {
            let (dx, dy) = input_map::to_virtual_desk(x, y, virt);
            mouse_input(
                dx,
                dy,
                0,
                MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
            )
        }
        MouseOrKey::Button { button, down } => {
            let flags = match (button, down) {
                (MouseButton::Left, true) => MOUSEEVENTF_LEFTDOWN,
                (MouseButton::Left, false) => MOUSEEVENTF_LEFTUP,
                (MouseButton::Right, true) => MOUSEEVENTF_RIGHTDOWN,
                (MouseButton::Right, false) => MOUSEEVENTF_RIGHTUP,
                (MouseButton::Middle, true) => MOUSEEVENTF_MIDDLEDOWN,
                (MouseButton::Middle, false) => MOUSEEVENTF_MIDDLEUP,
            };
            mouse_input(0, 0, 0, flags)
        }
        MouseOrKey::Wheel(d) => mouse_input(0, 0, d as u32, MOUSEEVENTF_WHEEL),
        MouseOrKey::HWheel(d) => mouse_input(0, 0, d as u32, MOUSEEVENTF_HWHEEL),
        MouseOrKey::Key { key, down } => key_input(key, down),
        MouseOrKey::Scan {
            scan,
            extended,
            down,
        } => scan_input(scan, extended, down),
    }
}

/// Creates the input sinks of a session.
pub struct WinInputBackend;

impl WinInputBackend {
    /// A backend; devices are created per session, in `create`.
    pub fn new() -> Self {
        WinInputBackend
    }
}

impl Default for WinInputBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl InputBackend for WinInputBackend {
    fn create(&self, target: &InputTarget) -> Result<Box<dyn InputSink>, String> {
        let out = target
            .output
            .as_ref()
            .ok_or_else(|| "the virtual display is unknown".to_string())?;
        let screen = match display::current_mode(&out.name) {
            Some((w, h, _, x, y)) => Rect {
                x,
                y,
                w: w as i32,
                h: h as i32,
            },
            None => Rect {
                x: out.origin.0,
                y: out.origin.1,
                w: out.mode.width as i32,
                h: out.mode.height as i32,
            },
        };
        let mut notes = Vec::new();
        let pen_dev = if target.pen_mode == PenMode::Tablet {
            Device::pen()
                .map_err(|e| {
                    notes.push(format!(
                        "synthetic pen device unavailable: {e} (needs Windows 10 1809+)"
                    ));
                })
                .ok()
        } else {
            None
        };
        let touch_dev = Device::touch()
            .map_err(|e| {
                notes.push(format!(
                    "synthetic touch device unavailable: {e} (needs Windows 10 1809+)"
                ));
            })
            .ok();
        tracing::info!(
            "input: monitor {} at {},{} {}x{}; pen {}, touch {}",
            out.name,
            screen.x,
            screen.y,
            screen.w,
            screen.h,
            match (target.pen_mode, pen_dev.is_some()) {
                (PenMode::Mouse, _) => "as mouse",
                (_, true) => "device",
                (_, false) => "unavailable",
            },
            if touch_dev.is_some() {
                "device"
            } else {
                "unavailable"
            },
        );
        Ok(Box::new(WinInputSink {
            gdi_name: out.name.clone(),
            screen,
            screen_checked: Instant::now(),
            pen_mode: target.pen_mode,
            pen_dev,
            touch_dev,
            pen: PenTracker::default(),
            mouse_pen: MousePenTracker::default(),
            touch: TouchTracker::default(),
            keys: KeyTracker::new(),
            notes,
            warn_inject: RateLimit::new(WARN_EVERY),
            warn_send: RateLimit::new(WARN_EVERY),
            split_mixed: false,
        }))
    }
}

struct WinInputSink {
    gdi_name: String,
    /// The virtual display in desktop pixels.
    screen: Rect,
    screen_checked: Instant,
    pen_mode: PenMode,
    pen_dev: Option<Device>,
    touch_dev: Option<Device>,
    pen: PenTracker,
    mouse_pen: MousePenTracker,
    touch: TouchTracker,
    keys: KeyTracker,
    notes: Vec<String>,
    warn_inject: RateLimit,
    warn_send: RateLimit,
    split_mixed: bool,
}

impl WinInputSink {
    /// Looks the monitor rectangle up again: always when `force`, else at most once a second.
    fn refresh_screen(&mut self, force: bool) {
        if !force && self.screen_checked.elapsed() < RECT_REFRESH {
            return;
        }
        self.screen_checked = Instant::now();
        if let Some((w, h, _, x, y)) = display::current_mode(&self.gdi_name) {
            let r = Rect {
                x,
                y,
                w: w as i32,
                h: h as i32,
            };
            if r != self.screen {
                tracing::debug!("input: monitor {} moved to {r:?}", self.gdi_name);
                self.screen = r;
            }
        }
    }

    fn warn_inject_failed(limit: &mut RateLimit, what: &str, e: &windows::core::Error) {
        if let Some(n) = limit.allow(Instant::now()) {
            tracing::warn!(
                "input: injecting {what} failed: {} ({n} similar suppressed)",
                err_str(e)
            );
        }
    }

    fn send(&mut self, events: &[MouseOrKey]) {
        if events.is_empty() {
            return;
        }
        let virt = virtual_screen();
        let inputs: Vec<INPUT> = events.iter().map(|e| to_input(e, virt)).collect();
        // SAFETY: `inputs` is a valid slice of initialised INPUT structs.
        let sent = unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };
        if (sent as usize) < inputs.len() {
            if let Some(n) = self.warn_send.allow(Instant::now()) {
                let e = windows::core::Error::from_thread();
                tracing::warn!(
                    "input: SendInput injected {sent} of {} events: {} ({n} similar suppressed); \
                     input to elevated (administrator) windows is blocked by UIPI unless the host \
                     runs elevated",
                    inputs.len(),
                    err_str(&e)
                );
            }
        }
    }

    fn inject_pen_frames(&mut self, frames: &[PenFrame]) {
        let Some(dev) = &self.pen_dev else { return };
        for f in frames {
            if let Err(e) = dev.inject(&[pen_info(f, self.screen)]) {
                Self::warn_inject_failed(&mut self.warn_inject, "pen", &e);
            }
        }
    }

    fn inject_touch_contacts(&mut self, contacts: &[TouchContactOut]) {
        let Some(dev) = &self.touch_dev else { return };
        if contacts.is_empty() {
            return;
        }
        // Once Windows rejected a mixed injection, split all of them up front.
        if self.split_mixed
            && let Some((updates, transitions)) = input_map::split_mixed(contacts)
        {
            for part in [updates, transitions] {
                if let Err(e) = inject_touches(dev, self.screen, &part) {
                    Self::warn_inject_failed(&mut self.warn_inject, "touch", &e);
                }
            }
            return;
        }
        let Err(e) = inject_touches(dev, self.screen, contacts) else {
            return;
        };
        if e.code() == E_INVALIDARG
            && let Some((updates, transitions)) = input_map::split_mixed(contacts)
        {
            tracing::warn!(
                "input: Windows rejected a touch injection mixing DOWN/UP and UPDATE contacts \
                 (E_INVALIDARG); splitting such injections from now on"
            );
            self.split_mixed = true;
            for part in [updates, transitions] {
                if let Err(e) = inject_touches(dev, self.screen, &part) {
                    Self::warn_inject_failed(&mut self.warn_inject, "touch", &e);
                }
            }
            return;
        }
        Self::warn_inject_failed(&mut self.warn_inject, "touch", &e);
    }
}

impl InputSink for WinInputSink {
    fn pen(&mut self, s: &PenSample) {
        let entering = s.flags & (PenSample::IN_RANGE | PenSample::CONTACT) != 0;
        match self.pen_mode {
            PenMode::Mouse => {
                self.refresh_screen(entering && !self.mouse_pen.in_range());
                let events = self.mouse_pen.sample(self.screen, s);
                self.send(&events);
            }
            PenMode::Tablet => {
                if self.pen_dev.is_none() {
                    return;
                }
                self.refresh_screen(entering && !self.pen.in_range());
                let frames = self.pen.sample(s);
                self.inject_pen_frames(&frames);
            }
        }
    }

    fn touch(&mut self, f: &TouchFrame) {
        if self.touch_dev.is_none() {
            return;
        }
        self.refresh_screen(false);
        let contacts = self.touch.frame(f, Instant::now());
        self.inject_touch_contacts(&contacts);
    }

    fn button(&mut self, b: &PointerButton) {
        self.refresh_screen(false);
        let events = input_map::button_inputs(self.screen, b);
        self.send(&events);
    }

    fn scroll(&mut self, s: &Scroll) {
        self.refresh_screen(false);
        let events = input_map::scroll_inputs(self.screen, s);
        self.send(&events);
    }

    fn key(&mut self, k: &Key) {
        let events = self.keys.key(k);
        self.send(&events);
    }

    fn tick(&mut self) {
        if let Some(contacts) = self.touch.refresh(Instant::now()) {
            self.inject_touch_contacts(&contacts);
        }
    }

    fn release_all(&mut self) {
        let frames = self.pen.release();
        self.inject_pen_frames(&frames);
        let events = self.mouse_pen.release();
        self.send(&events);
        let contacts = self.touch.release_all();
        self.inject_touch_contacts(&contacts);
        let events = self.keys.release_all();
        self.send(&events);
    }

    fn status(&self) -> InputStatus {
        let mut caps = InputStatus::ACTIONS | InputStatus::KEYS;
        if self.touch_dev.is_some() {
            caps |= InputStatus::TOUCH;
        }
        if self.pen_dev.is_some() || self.pen_mode == PenMode::Mouse {
            caps |= InputStatus::PEN;
        }
        InputStatus {
            caps,
            note: self.notes.join("; "),
        }
    }
}
