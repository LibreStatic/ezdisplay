//! uinput devices and the [`InputSink`] / [`InputBackend`] built on the encoders in `input.rs`.

use crate::input::{
    Frame, MAX_SLOTS, Mapper, PRESSURE_MAX, PenButton, PenEncoder, PointerEncoder, TouchEncoder,
    physical_mm, resolution,
};
use crate::{kscreen, kwin_input};
use evdev::uinput::VirtualDevice;
use evdev::{
    AbsInfo, AbsoluteAxisCode as Abs, AttributeSet, BusType, InputEvent, InputId, KeyCode as Key,
    PropType, RelativeAxisCode as Rel, UinputAbsSetup,
};
use ezd_host_core::{InputBackend, InputSink, InputTarget, PenMode};
use ezd_proto::{InputStatus, PenSample, PointerButton, Scroll, TouchFrame};
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;
use tokio::runtime::Handle;

/// pid.codes test vendor id.
pub const VENDOR: u16 = 0x1209;
/// Product ids and names of the three devices.
pub const TOUCH_PRODUCT: u16 = 0xEDD1;
/// See [`TOUCH_PRODUCT`].
pub const PEN_PRODUCT: u16 = 0xEDD2;
/// See [`TOUCH_PRODUCT`].
pub const POINTER_PRODUCT: u16 = 0xEDD3;
/// Device names (KWin persists the output mapping by vendor / product / name).
pub const TOUCH_NAME: &str = "ezdisplay touch";
/// See [`TOUCH_NAME`].
pub const PEN_NAME: &str = "ezdisplay pen";
/// See [`TOUCH_NAME`].
pub const POINTER_NAME: &str = "ezdisplay pointer";

const UDEV_HINT: &str = "Install packaging/linux/70-ezdisplay-uinput.rules, see docs/input.md";

fn id(product: u16) -> InputId {
    InputId::new(BusType::BUS_USB, VENDOR, product, 1)
}

fn setup(code: Abs, min: i32, max: i32, res: i32) -> UinputAbsSetup {
    UinputAbsSetup::new(code, AbsInfo::new(0, min, max, 0, 0, res))
}

/// Maps an error from opening `/dev/uinput` to the note the client sees.
fn open_error(e: &io::Error) -> String {
    match e.kind() {
        io::ErrorKind::PermissionDenied => {
            format!("cannot open /dev/uinput (permission denied). {UDEV_HINT}")
        }
        io::ErrorKind::NotFound => {
            "/dev/uinput is missing (load the module: sudo modprobe uinput)".to_string()
        }
        _ => format!("cannot create the uinput device: {e}"),
    }
}

/// The touchscreen: multi-touch type B, direct.
pub fn create_touch(mm: (f64, f64)) -> io::Result<VirtualDevice> {
    let (rx, ry) = (resolution(mm.0), resolution(mm.1));
    VirtualDevice::builder()?
        .name(TOUCH_NAME)
        .input_id(id(TOUCH_PRODUCT))
        .with_properties(&AttributeSet::from_iter([PropType::DIRECT]))?
        .with_keys(&AttributeSet::from_iter([Key::BTN_TOUCH]))?
        .with_absolute_axis(&setup(Abs::ABS_MT_SLOT, 0, MAX_SLOTS as i32 - 1, 0))?
        .with_absolute_axis(&setup(Abs::ABS_MT_TRACKING_ID, 0, 65535, 0))?
        .with_absolute_axis(&setup(Abs::ABS_MT_TOOL_TYPE, 0, 2, 0))?
        .with_absolute_axis(&setup(Abs::ABS_MT_POSITION_X, 0, 65535, rx))?
        .with_absolute_axis(&setup(Abs::ABS_MT_POSITION_Y, 0, 65535, ry))?
        .with_absolute_axis(&setup(Abs::ABS_X, 0, 65535, rx))?
        .with_absolute_axis(&setup(Abs::ABS_Y, 0, 65535, ry))?
        .build()
}

/// The graphics tablet.
pub fn create_pen(mm: (f64, f64)) -> io::Result<VirtualDevice> {
    let (rx, ry) = (resolution(mm.0), resolution(mm.1));
    VirtualDevice::builder()?
        .name(PEN_NAME)
        .input_id(id(PEN_PRODUCT))
        .with_properties(&AttributeSet::from_iter([PropType::DIRECT]))?
        .with_keys(&AttributeSet::from_iter([
            Key::BTN_TOOL_PEN,
            Key::BTN_TOOL_RUBBER,
            Key::BTN_TOUCH,
            Key::BTN_STYLUS,
            Key::BTN_STYLUS2,
        ]))?
        .with_absolute_axis(&setup(Abs::ABS_X, 0, 65535, rx))?
        .with_absolute_axis(&setup(Abs::ABS_Y, 0, 65535, ry))?
        .with_absolute_axis(&setup(Abs::ABS_PRESSURE, 0, PRESSURE_MAX, 0))?
        .with_absolute_axis(&setup(Abs::ABS_TILT_X, -9000, 9000, 5730))?
        .with_absolute_axis(&setup(Abs::ABS_TILT_Y, -9000, 9000, 5730))?
        .with_absolute_axis(&setup(Abs::ABS_DISTANCE, 0, 1, 0))?
        .build()
}

/// The absolute pointer (QEMU-tablet style: no INPUT_PROP_DIRECT, no REL_X/Y).
pub fn create_pointer() -> io::Result<VirtualDevice> {
    VirtualDevice::builder()?
        .name(POINTER_NAME)
        .input_id(id(POINTER_PRODUCT))
        .with_keys(&AttributeSet::from_iter([
            Key::BTN_LEFT,
            Key::BTN_RIGHT,
            Key::BTN_MIDDLE,
            Key::KEY_LEFTCTRL,
            Key::KEY_LEFTSHIFT,
        ]))?
        .with_relative_axes(&AttributeSet::from_iter([
            Rel::REL_WHEEL,
            Rel::REL_HWHEEL,
            Rel::REL_WHEEL_HI_RES,
            Rel::REL_HWHEEL_HI_RES,
        ]))?
        .with_absolute_axis(&setup(Abs::ABS_X, 0, 65535, 0))?
        .with_absolute_axis(&setup(Abs::ABS_Y, 0, 65535, 0))?
        .build()
}

/// `eventN` names of a freshly created device (the nodes can show up a moment later).
fn event_name(dev: &mut VirtualDevice) -> Option<String> {
    for _ in 0..40 {
        if let Ok(nodes) = dev.enumerate_dev_nodes_blocking() {
            for p in nodes.flatten() {
                if let Some(n) = p.file_name().and_then(|n| n.to_str()) {
                    return Some(n.to_string());
                }
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    None
}

fn write(dev: &mut VirtualDevice, frames: Vec<Frame>, failed: &mut bool) {
    for f in frames {
        let events: Vec<InputEvent> = f
            .into_iter()
            .map(|(t, c, v)| InputEvent::new(t, c, v))
            .collect();
        if let Err(e) = dev.emit(&events)
            && !std::mem::replace(failed, true)
        {
            tracing::warn!("uinput write failed: {e}");
        }
    }
}

/// Refreshes the output geometry every 3 s on its own thread.
struct GeometryRefresh {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl GeometryRefresh {
    fn start(output: String, mapper: Arc<Mutex<Mapper>>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::Builder::new()
            .name("ezd-geometry".into())
            .spawn(move || {
                'outer: loop {
                    for _ in 0..30 {
                        if flag.load(Ordering::Relaxed) {
                            break 'outer;
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    if let Ok(outs) = kscreen::list_outputs() {
                        let m = Mapper::from_outputs(&outs, &output);
                        if m.is_known() {
                            *mapper.lock().unwrap_or_else(|e| e.into_inner()) = m;
                        }
                    }
                }
            })
            .ok();
        GeometryRefresh { stop, thread }
    }
}

impl Drop for GeometryRefresh {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct Tablet {
    enc: PenEncoder,
    dev: VirtualDevice,
}

struct Touchscreen {
    enc: TouchEncoder,
    dev: VirtualDevice,
}

/// The sink of one session: three uinput devices and their encoders.
pub struct UinputSink {
    // Drop order: the refresh thread first, then the devices (uinput removes them).
    _refresh: Option<GeometryRefresh>,
    mapper: Arc<Mutex<Mapper>>,
    warned_offset: bool,
    pointer_enc: PointerEncoder,
    pointer: VirtualDevice,
    /// `None` = not created (mouse mode), or not mapped to the output.
    touch: Option<Touchscreen>,
    pen: Option<Tablet>,
    pen_mode: PenMode,
    caps: u32,
    note: String,
    write_failed: bool,
}

impl UinputSink {
    fn mapper(&mut self) -> Mapper {
        let m = *self.mapper.lock().unwrap_or_else(|e| e.into_inner());
        if m.origin_offset && !self.warned_offset {
            self.warned_offset = true;
            tracing::warn!(
                "the desktop layout does not start at (0, 0): pointer coordinates will be offset \
                 (KWin scales absolute pointers over the workspace without its origin)"
            );
        }
        m
    }

    fn pointer_write(&mut self, frames: Vec<Frame>) {
        write(&mut self.pointer, frames, &mut self.write_failed);
    }
}

impl InputSink for UinputSink {
    fn pen(&mut self, s: &PenSample) {
        if self.pen_mode == PenMode::Mouse {
            let m = self.mapper();
            let frames = self.pointer_enc.pen_as_mouse(s, &m);
            self.pointer_write(frames);
        } else if let Some(p) = &mut self.pen {
            let frames = p.enc.pen(s);
            write(&mut p.dev, frames, &mut self.write_failed);
        }
    }

    fn touch(&mut self, f: &TouchFrame) {
        if let Some(t) = &mut self.touch {
            let frames = t.enc.touch(f);
            write(&mut t.dev, frames, &mut self.write_failed);
        }
    }

    fn button(&mut self, b: &PointerButton) {
        let m = self.mapper();
        let frames = self.pointer_enc.button(b, &m);
        self.pointer_write(frames);
    }

    fn scroll(&mut self, s: &Scroll) {
        let m = self.mapper();
        let frames = self.pointer_enc.scroll(s, &m);
        self.pointer_write(frames);
    }

    fn release_all(&mut self) {
        let frames = self.pointer_enc.release_all();
        self.pointer_write(frames);
        if let Some(t) = &mut self.touch {
            let frames = t.enc.release_all();
            write(&mut t.dev, frames, &mut self.write_failed);
        }
        if let Some(p) = &mut self.pen {
            let frames = p.enc.release_all();
            write(&mut p.dev, frames, &mut self.write_failed);
        }
    }

    fn status(&self) -> InputStatus {
        InputStatus {
            caps: self.caps,
            note: self.note.clone(),
        }
    }
}

impl Drop for UinputSink {
    fn drop(&mut self) {
        InputSink::release_all(self);
    }
}

/// Input injection for KDE Plasma 6: uinput devices mapped to the virtual output through KWin.
pub struct KdeInputBackend {
    handle: Handle,
    pen_button: PenButton,
}

impl KdeInputBackend {
    /// `handle` runs the D-Bus calls (the input thread has no runtime of its own). The barrel
    /// button comes from `EZD_PEN_BUTTON` (`stylus` / `stylus2`).
    pub fn new(handle: Handle) -> Self {
        let env = std::env::var("EZD_PEN_BUTTON").ok();
        KdeInputBackend {
            handle,
            pen_button: PenButton::from_env_value(env.as_deref()),
        }
    }
}

impl InputBackend for KdeInputBackend {
    fn create(&self, target: &InputTarget) -> Result<Box<dyn InputSink>, String> {
        let d = &target.client.display;
        let mm = physical_mm(d.width_px, d.height_px, d.density_dpi);
        let mouse = target.pen_mode == PenMode::Mouse;
        let mut notes: Vec<String> = Vec::new();

        let pointer = create_pointer().map_err(|e| open_error(&e))?;
        let mut touch_dev = create_touch(mm).map_err(|e| open_error(&e))?;
        let mut pen_dev = if mouse {
            None
        } else {
            Some(create_pen(mm).map_err(|e| open_error(&e))?)
        };

        // KWin drops touch and tablet events of devices without an output.
        let (mut touch_ok, mut pen_ok) = (false, false);
        match target.output.as_ref().map(|o| o.name.as_str()) {
            None | Some("") => notes.push(
                "touch and pen need the virtual output name, which the video source did not report"
                    .into(),
            ),
            Some(output) => {
                let mut devices: Vec<(&str, String)> = Vec::new();
                match event_name(&mut touch_dev) {
                    Some(n) => devices.push(("touch", n)),
                    None => notes.push("the touch device has no /dev/input node".into()),
                }
                if let Some(p) = pen_dev.as_mut() {
                    match event_name(p) {
                        Some(n) => devices.push(("pen", n)),
                        None => notes.push("the pen device has no /dev/input node".into()),
                    }
                }
                let refs: Vec<(&str, &str)> =
                    devices.iter().map(|(l, n)| (*l, n.as_str())).collect();
                let results = kwin_input::map_devices(&self.handle, &refs, output);
                for ((label, _), r) in devices.iter().zip(results) {
                    match r {
                        Ok(()) => match *label {
                            "touch" => touch_ok = true,
                            _ => pen_ok = true,
                        },
                        Err(e) => notes.push(format!("{label}: {e}")),
                    }
                }
            }
        }

        // Output geometry for the pointer (one blocking call here, then a helper thread).
        let mut mapper = Mapper::identity();
        let mut refresh = None;
        if let Some(o) = target.output.as_ref().filter(|o| !o.name.is_empty()) {
            if let Ok(outs) = kscreen::list_outputs() {
                mapper = Mapper::from_outputs(&outs, &o.name);
            }
            if mapper.is_known() {
                let shared = Arc::new(Mutex::new(mapper));
                refresh = Some((o.name.clone(), shared));
            } else {
                notes.push(
                    "output geometry unknown: pointer coordinates are mapped over the whole desktop"
                        .into(),
                );
            }
        }
        let shared = match &refresh {
            Some((_, s)) => s.clone(),
            None => Arc::new(Mutex::new(mapper)),
        };
        let refresh = refresh.map(|(name, s)| GeometryRefresh::start(name, s));

        let mut caps = InputStatus::ACTIONS;
        if touch_ok {
            caps |= InputStatus::TOUCH;
        }
        if mouse || pen_ok {
            caps |= InputStatus::PEN;
        }
        Ok(Box::new(UinputSink {
            _refresh: refresh,
            mapper: shared,
            warned_offset: false,
            pointer_enc: PointerEncoder::new(),
            pointer,
            touch: touch_ok.then(|| Touchscreen {
                enc: TouchEncoder::new(),
                dev: touch_dev,
            }),
            pen: if pen_ok {
                pen_dev.map(|dev| Tablet {
                    enc: PenEncoder::new(self.pen_button),
                    dev,
                })
            } else {
                None
            },
            pen_mode: target.pen_mode,
            caps,
            note: notes.join("; "),
            write_failed: false,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ezd_proto::TouchContact;

    #[test]
    fn open_errors_have_actionable_notes() {
        let denied = open_error(&io::Error::from(io::ErrorKind::PermissionDenied));
        assert!(denied.starts_with("cannot open /dev/uinput (permission denied)"));
        assert!(denied.contains("70-ezdisplay-uinput.rules"));
        assert!(open_error(&io::Error::from(io::ErrorKind::NotFound)).contains("missing"));
        assert!(open_error(&io::Error::other("boom")).contains("boom"));
    }

    /// Needs `/dev/uinput` access (see docs/input.md): `EZD_UINPUT_TEST=1 cargo test`.
    #[test]
    fn real_uinput_devices() {
        if std::env::var_os("EZD_UINPUT_TEST").is_none_or(|v| v != "1") {
            eprintln!("skipped: set EZD_UINPUT_TEST=1 (needs /dev/uinput access)");
            return;
        }
        let mm = (268.5, 167.6);
        let mut touch = create_touch(mm).unwrap();
        let mut pen = create_pen(mm).unwrap();
        let mut pointer = create_pointer().unwrap();

        let open = |dev: &mut VirtualDevice| -> evdev::Device {
            let name = event_name(dev).expect("event node");
            for _ in 0..40 {
                if let Ok(d) = evdev::Device::open(format!("/dev/input/{name}")) {
                    return d;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            panic!("cannot open /dev/input/{name}");
        };
        let mut t = open(&mut touch);
        let p = open(&mut pen);
        let q = open(&mut pointer);

        assert_eq!(t.name(), Some(TOUCH_NAME));
        assert_eq!(p.name(), Some(PEN_NAME));
        assert_eq!(q.name(), Some(POINTER_NAME));
        for (d, product) in [
            (&t, TOUCH_PRODUCT),
            (&p, PEN_PRODUCT),
            (&q, POINTER_PRODUCT),
        ] {
            let i = d.input_id();
            assert_eq!((i.vendor(), i.product()), (VENDOR, product));
        }
        assert!(t.properties().contains(PropType::DIRECT));
        assert!(p.properties().contains(PropType::DIRECT));
        assert!(!q.properties().contains(PropType::DIRECT));
        let ta = t.supported_absolute_axes().unwrap();
        assert!(ta.contains(Abs::ABS_MT_SLOT) && ta.contains(Abs::ABS_MT_POSITION_X));
        assert!(!t.supported_keys().unwrap().contains(Key::BTN_LEFT));
        let pa = p.supported_absolute_axes().unwrap();
        assert!(pa.contains(Abs::ABS_PRESSURE) && pa.contains(Abs::ABS_TILT_X));
        assert!(p.supported_keys().unwrap().contains(Key::BTN_STYLUS2));
        assert!(q.supported_keys().unwrap().contains(Key::BTN_LEFT));
        assert!(
            q.supported_relative_axes()
                .unwrap()
                .contains(Rel::REL_WHEEL_HI_RES)
        );
        assert!(
            q.supported_relative_axes()
                .unwrap()
                .contains(Rel::REL_WHEEL)
        );

        // An emitted touch frame is read back.
        let mut enc = TouchEncoder::new();
        let frames = enc.touch(&TouchFrame {
            t_us: 0,
            contacts: vec![TouchContact {
                id: 0,
                x: 1000,
                y: 2000,
                phase: TouchContact::DOWN,
            }],
        });
        let mut failed = false;
        write(&mut touch, frames, &mut failed);
        assert!(!failed);
        t.set_nonblocking(true).unwrap();
        let mut seen = false;
        for _ in 0..20 {
            if let Ok(events) = t.fetch_events() {
                for ev in events {
                    if ev.code() == Abs::ABS_MT_POSITION_X.0 && ev.value() == 1000 {
                        seen = true;
                    }
                }
            }
            if seen {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(seen, "touch frame was not read back");
    }
}
