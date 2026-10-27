//! JNI entry points of `com.librestatic.ezdisplay.NativeBridge`.
//!
//! Everything is polled from Kotlin (cursor, stats): native threads never call back into the JVM.

use crate::caps::parse_caps;
use crate::decoder::SendWindow;
use crate::input::{mono_to_now_us, unpack_motion};
use crate::keys::keycode_to_hid;
use crate::session::{AccessoryFd, Session, Target};
use crate::wifi;
use ezd_client_core::now_us;
use ezd_input::{InputConfig, InputEngine, ViewTransform};
use ezd_link::identity::parse_fingerprint;
use ezd_proto::{AudioVolume, Key, Message};
use jni::EnvUnowned;
use jni::errors::ThrowRuntimeExAndDefault;
use jni::objects::{JByteArray, JClass, JFloatArray, JIntArray, JLongArray, JObject, JString};
use jni::sys::{jboolean, jint};
use ndk::native_window::NativeWindow;
use std::cell::RefCell;
use std::os::fd::FromRawFd;
use std::sync::Mutex;
use std::sync::Once;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static SESSION: Mutex<Option<Session>> = Mutex::new(None);

/// Input engine plus what is needed to (re)build it. Lock order: never hold [`SESSION`]
/// while locking this; collect messages here, release, then lock the session to send.
struct InputState {
    engine: Option<InputEngine>,
    config: Option<InputConfig>,
    view: (f32, f32),
}

static INPUT: Mutex<InputState> = Mutex::new(InputState {
    engine: None,
    config: None,
    view: (1.0, 1.0),
});
/// Last hardware keyboard state from Kotlin; a new session starts with it.
static KEYBOARD_ATTACHED: AtomicBool = AtomicBool::new(false);
/// The tablet's media volume as last reported by Kotlin; sent to the host with each new session.
static LOCAL_VOLUME: Mutex<Option<AudioVolume>> = Mutex::new(None);
static LAST_INPUT_ERR_S: AtomicU64 = AtomicU64::new(0);

#[derive(Default)]
struct Scratch {
    ints: Vec<i32>,
    floats: Vec<f32>,
    times: Vec<i64>,
    out: Vec<Message>,
}

thread_local! {
    static SCRATCH: RefCell<Scratch> = RefCell::new(Scratch::default());
}
static LOG_INIT: Once = Once::new();

fn init_logging() {
    LOG_INIT.call_once(|| {
        android_logger::init_once(
            android_logger::Config::default()
                .with_tag("ezdisplay")
                .with_max_level(log::LevelFilter::Debug),
        );
        std::panic::set_hook(Box::new(|info| log::error!("panic: {info}")));
    });
}

fn input() -> std::sync::MutexGuard<'static, InputState> {
    INPUT.lock().unwrap_or_else(|e| e.into_inner())
}

impl InputState {
    fn engine(&mut self) -> &mut InputEngine {
        let (cfg, (w, h)) = (&self.config, self.view);
        self.engine
            .get_or_insert_with(|| InputEngine::new(cfg.clone().unwrap_or_default(), w, h))
    }
}

/// Sends collected messages through the session (after the `INPUT` lock was released).
fn send_all(out: &mut Vec<Message>) {
    if out.is_empty() {
        return;
    }
    if let Some(s) = session().as_ref() {
        for m in out.iter() {
            s.send_input(m);
        }
    }
    out.clear();
}

fn mono_ns() -> i64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is a valid, writable timespec for the duration of the call.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec * 1_000_000_000 + ts.tv_nsec
}

fn log_input_error(e: &str) {
    let sec = now_us() / 1_000_000;
    if LAST_INPUT_ERR_S.swap(sec, Ordering::Relaxed) != sec {
        log::warn!("bad input event: {e}");
    }
}

fn session() -> std::sync::MutexGuard<'static, Option<Session>> {
    SESSION.lock().unwrap_or_else(|e| e.into_inner())
}

/// Acquires an `ANativeWindow` from a Java `Surface`.
fn window_from(env: &jni::Env<'_>, surface: &JObject<'_>) -> Option<SendWindow> {
    if surface.is_null() {
        return None;
    }
    // SAFETY: `env` and `surface` are valid for the duration of this native call.
    unsafe { NativeWindow::from_surface(env.get_raw().cast(), surface.as_raw().cast()) }
        .map(SendWindow)
}

/// Replaces the running session (if any) with one connecting to `target`.
fn start_session(window: Option<SendWindow>, target: Target, caps: crate::caps::ParsedCaps) {
    let mut guard = session();
    if let Some(old) = guard.take() {
        old.stop();
    }
    log::info!("start {target:?} surface={}", window.is_some());
    let s = Session::start(window, target, caps);
    s.set_keyboard_attached(KEYBOARD_ATTACHED.load(Ordering::Relaxed));
    let volume = *LOCAL_VOLUME.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(v) = volume {
        s.set_audio_volume(v);
    }
    *guard = Some(s);
}

/// `external fun start(surface: Surface?, host: String, port: Int, capsJson: String): String?`
/// Returns null on success or an error message.
#[jni::jni_mangle("com.librestatic.ezdisplay.NativeBridge")]
pub extern "system" fn start<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    surface: JObject<'local>,
    host: JString<'local>,
    port: jint,
    caps_json: JString<'local>,
) -> JString<'local> {
    init_logging();
    env.with_env(|env| -> jni::errors::Result<JString<'local>> {
        let host = host.try_to_string(env)?;
        let caps_json = caps_json.try_to_string(env)?;
        let mut err = |m: String| env.new_string(m);
        let Ok(port) = u16::try_from(port) else {
            return err(format!("bad port {port}"));
        };
        let caps = match parse_caps(&caps_json) {
            Ok(c) => c,
            Err(e) => return err(e),
        };
        let window = window_from(env, &surface);
        start_session(window, Target::Tcp { host, port }, caps);
        Ok(JString::null())
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

/// `external fun startWifi(surface: Surface?, addr: String, fingerprintHex: String,
/// stateDir: String, capsJson: String): String?`
/// Connects over Wi-Fi to the paired host with this certificate fingerprint. `addr` is
/// `ip:port` (a bare IP uses the default port). Returns null on success or an error message.
#[jni::jni_mangle("com.librestatic.ezdisplay.NativeBridge")]
pub extern "system" fn startWifi<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    surface: JObject<'local>,
    addr: JString<'local>,
    fingerprint_hex: JString<'local>,
    state_dir: JString<'local>,
    caps_json: JString<'local>,
) -> JString<'local> {
    init_logging();
    env.with_env(|env| -> jni::errors::Result<JString<'local>> {
        let addr = addr.try_to_string(env)?;
        let fingerprint_hex = fingerprint_hex.try_to_string(env)?;
        let state_dir = state_dir.try_to_string(env)?;
        let caps_json = caps_json.try_to_string(env)?;
        let mut err = |m: String| env.new_string(m);
        let Some(addr) = wifi::parse_addr(&addr) else {
            return err(format!("bad address {addr}"));
        };
        let Some(fingerprint) = parse_fingerprint(&fingerprint_hex) else {
            return err("bad fingerprint".into());
        };
        let caps = match parse_caps(&caps_json) {
            Ok(c) => c,
            Err(e) => return err(e),
        };
        let window = window_from(env, &surface);
        let target = Target::Wifi {
            addr,
            fingerprint,
            state_dir: state_dir.into(),
        };
        start_session(window, target, caps);
        Ok(JString::null())
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

/// `external fun startFd(surface: Surface?, fd: Int, capsJson: String): String?`
/// Streams over the USB accessory file descriptor. Takes ownership of `fd` (Kotlin detached it
/// from its `ParcelFileDescriptor`): it is closed exactly once, when the session ends. Returns
/// null on success or an error message.
#[jni::jni_mangle("com.librestatic.ezdisplay.NativeBridge")]
pub extern "system" fn startFd<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    surface: JObject<'local>,
    fd: jint,
    caps_json: JString<'local>,
) -> JString<'local> {
    init_logging();
    env.with_env(|env| -> jni::errors::Result<JString<'local>> {
        let caps_json = caps_json.try_to_string(env)?;
        let mut err = |m: String| env.new_string(m);
        if fd < 0 {
            return err(format!("bad accessory fd {fd}"));
        }
        // SAFETY: Kotlin detached `fd` and hands over its only owner; nothing else closes it.
        let fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) };
        let caps = match parse_caps(&caps_json) {
            Ok(c) => c,
            Err(e) => return err(e),
        };
        let window = window_from(env, &surface);
        start_session(
            window,
            Target::Accessory {
                fd: AccessoryFd::new(fd),
            },
            caps,
        );
        Ok(JString::null())
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

/// `external fun wifiPair(addr: String, pinOrUri: String, deviceName: String, stateDir: String):
/// String` Blocks (call off the main thread). JSON, see [`wifi::pair_json`].
#[jni::jni_mangle("com.librestatic.ezdisplay.NativeBridge")]
pub extern "system" fn wifiPair<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    addr: JString<'local>,
    pin_or_uri: JString<'local>,
    device_name: JString<'local>,
    state_dir: JString<'local>,
) -> JString<'local> {
    init_logging();
    env.with_env(|env| -> jni::errors::Result<JString<'local>> {
        let addr = addr.try_to_string(env)?;
        let pin_or_uri = pin_or_uri.try_to_string(env)?;
        let device_name = device_name.try_to_string(env)?;
        let state_dir = state_dir.try_to_string(env)?;
        let json = wifi::pair_json(&addr, &pin_or_uri, &device_name, state_dir.as_ref());
        env.new_string(json)
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

/// `external fun wifiHosts(stateDir: String): String` JSON array of paired hosts.
#[jni::jni_mangle("com.librestatic.ezdisplay.NativeBridge")]
pub extern "system" fn wifiHosts<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    state_dir: JString<'local>,
) -> JString<'local> {
    init_logging();
    env.with_env(|env| -> jni::errors::Result<JString<'local>> {
        let state_dir = state_dir.try_to_string(env)?;
        env.new_string(wifi::hosts_json(state_dir.as_ref()))
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

/// `external fun wifiForget(stateDir: String, fingerprint: String): Boolean`
#[jni::jni_mangle("com.librestatic.ezdisplay.NativeBridge")]
pub extern "system" fn wifiForget<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    state_dir: JString<'local>,
    fingerprint: JString<'local>,
) -> jboolean {
    init_logging();
    env.with_env(|env| -> jni::errors::Result<jboolean> {
        let state_dir = state_dir.try_to_string(env)?;
        let fingerprint = fingerprint.try_to_string(env)?;
        Ok(wifi::forget(state_dir.as_ref(), &fingerprint))
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

/// `external fun wifiIdentity(stateDir: String): String` Short fingerprint of this device.
#[jni::jni_mangle("com.librestatic.ezdisplay.NativeBridge")]
pub extern "system" fn wifiIdentity<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    state_dir: JString<'local>,
) -> JString<'local> {
    init_logging();
    env.with_env(|env| -> jni::errors::Result<JString<'local>> {
        let state_dir = state_dir.try_to_string(env)?;
        env.new_string(wifi::identity_short(state_dir.as_ref()))
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

/// `external fun stop()`
#[jni::jni_mangle("com.librestatic.ezdisplay.NativeBridge")]
pub extern "system" fn stop<'local>(mut env: EnvUnowned<'local>, _class: JClass<'local>) {
    env.with_env(|_env| -> jni::errors::Result<()> {
        // Lift everything on the host before the session goes away; the next session starts
        // with a fresh engine (no local zoom, no pending gesture).
        let mut out = Vec::new();
        if let Some(mut engine) = input().engine.take() {
            engine.release_all(now_us(), &mut out);
        }
        send_all(&mut out);
        let old = session().take();
        if let Some(s) = old {
            s.stop();
        }
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

/// `external fun surfaceChanged(surface: Surface)`: a (new) surface is available.
#[jni::jni_mangle("com.librestatic.ezdisplay.NativeBridge")]
pub extern "system" fn surface_changed<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    surface: JObject<'local>,
) {
    env.with_env(|env| -> jni::errors::Result<()> {
        if let (Some(s), Some(w)) = (session().as_ref(), window_from(env, &surface)) {
            s.surface_changed(w);
        }
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

/// `external fun surfaceDestroyed()`: blocks until the decoder released the surface.
#[jni::jni_mangle("com.librestatic.ezdisplay.NativeBridge")]
pub extern "system" fn surface_destroyed<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
) {
    env.with_env(|_env| -> jni::errors::Result<()> {
        if let Some(s) = session().as_ref() {
            s.surface_destroyed();
        }
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

/// `external fun requestKeyframe()`
#[jni::jni_mangle("com.librestatic.ezdisplay.NativeBridge")]
pub extern "system" fn request_keyframe<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
) {
    env.with_env(|_env| -> jni::errors::Result<()> {
        if let Some(s) = session().as_ref() {
            s.request_keyframe();
        }
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

/// `external fun pollCursor(): LongArray` = `[seq, x, y, visible(0/1), shapeId]`.
/// `seq == 0` means no cursor state has arrived yet.
#[jni::jni_mangle("com.librestatic.ezdisplay.NativeBridge")]
pub extern "system" fn poll_cursor<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
) -> JLongArray<'local> {
    env.with_env(|env| -> jni::errors::Result<JLongArray<'local>> {
        let v: [i64; 5] = match session().as_ref().and_then(|s| s.cursor()) {
            Some(c) => [
                c.seq as i64,
                c.state.x as i64,
                c.state.y as i64,
                c.state.visible as i64,
                c.state.shape_id as i64,
            ],
            None => [0; 5],
        };
        let arr = env.new_long_array(v.len())?;
        arr.set_region(env, 0, &v)?;
        Ok(arr)
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

/// `external fun takeCursorShape(shapeId: Int): ByteArray?`
/// Layout: width, height, hotX, hotY as u16 little endian, then `width*height*4` bytes of
/// straight RGBA. Null if the shape is unknown.
#[jni::jni_mangle("com.librestatic.ezdisplay.NativeBridge")]
pub extern "system" fn take_cursor_shape<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    shape_id: jint,
) -> JByteArray<'local> {
    env.with_env(|env| -> jni::errors::Result<JByteArray<'local>> {
        let shape = session()
            .as_ref()
            .and_then(|s| s.cursor_shape(shape_id as u32));
        let Some(shape) = shape else {
            return Ok(JByteArray::null());
        };
        let mut buf = Vec::with_capacity(8 + shape.rgba.len());
        for v in [shape.width, shape.height, shape.hot_x, shape.hot_y] {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        buf.extend_from_slice(&shape.rgba);
        env.byte_array_from_slice(&buf)
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

/// `external fun pollStats(): String` JSON, see `Shared::to_json`. `{"phase":"idle"}` without a session.
#[jni::jni_mangle("com.librestatic.ezdisplay.NativeBridge")]
pub extern "system" fn poll_stats<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
) -> JString<'local> {
    env.with_env(|env| -> jni::errors::Result<JString<'local>> {
        let json = match session().as_ref() {
            Some(s) => s.stats_json(),
            None => r#"{"phase":"idle"}"#.to_string(),
        };
        env.new_string(json)
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

/// `external fun inputEvent(ints: IntArray, floats: FloatArray, times: LongArray)`
///
/// One `MotionEvent`, packed (see `input::unpack_motion`): `ints` = `[actionMasked, actionIndex,
/// buttonState, flags, pointerCount, historySize]` + per pointer `[id, toolType]`; `floats` =
/// `[vscroll, hscroll]` + per step (history oldest first, then current) per pointer
/// `[x, y, pressure, tilt, orientation]`; `times` = one uptime-based nanosecond time per step.
#[jni::jni_mangle("com.librestatic.ezdisplay.NativeBridge")]
pub extern "system" fn input_event<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    ints: JIntArray<'local>,
    floats: JFloatArray<'local>,
    times: JLongArray<'local>,
) {
    env.with_env(|env| -> jni::errors::Result<()> {
        let mut scratch = SCRATCH.with(|s| std::mem::take(&mut *s.borrow_mut()));
        let sc = &mut scratch;
        sc.ints.resize(ints.len(env)?, 0);
        ints.get_region(env, 0, &mut sc.ints)?;
        sc.floats.resize(floats.len(env)?, 0.0);
        floats.get_region(env, 0, &mut sc.floats)?;
        sc.times.resize(times.len(env)?, 0);
        times.get_region(env, 0, &mut sc.times)?;
        let (mono_now, now) = (mono_ns(), now_us());
        match unpack_motion(&sc.ints, &sc.floats, &sc.times, |t| {
            mono_to_now_us(t, mono_now, now)
        }) {
            Ok(m) => {
                sc.out.clear();
                input().engine().motion(&m, &mut sc.out);
                send_all(&mut sc.out);
            }
            Err(e) => log_input_error(&e),
        }
        SCRATCH.with(|s| *s.borrow_mut() = scratch);
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

/// `external fun keyEvent(keyCode: Int, pressed: Boolean, timeNs: Long): Boolean`: one hardware
/// key transition (`timeNs` is uptime-based). Returns whether the key code maps to a HID usage
/// and was sent (a press of a key already down is an autorepeat).
#[jni::jni_mangle("com.librestatic.ezdisplay.NativeBridge")]
pub extern "system" fn key_event<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    key_code: jint,
    pressed: jni::sys::jboolean,
    time_ns: jni::sys::jlong,
) -> jni::sys::jboolean {
    env.with_env(|_env| -> jni::errors::Result<jni::sys::jboolean> {
        let Some(usage) = keycode_to_hid(key_code) else {
            return Ok(false);
        };
        let mut out = vec![Message::Key(Key {
            t_us: mono_to_now_us(time_ns, mono_ns(), now_us()),
            usage,
            pressed,
        })];
        send_all(&mut out);
        Ok(true)
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

/// `external fun setKeyboardAttached(attached: Boolean)`: whether a hardware keyboard is attached.
/// Stored also while there is no session, so the next `start` reports it to the host.
#[jni::jni_mangle("com.librestatic.ezdisplay.NativeBridge")]
pub extern "system" fn set_keyboard_attached<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    attached: jni::sys::jboolean,
) {
    env.with_env(|_env| -> jni::errors::Result<()> {
        KEYBOARD_ATTACHED.store(attached, Ordering::Relaxed);
        if let Some(s) = session().as_ref() {
            s.set_keyboard_attached(attached);
        }
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

/// `external fun setLocalVolume(volume: Float, muted: Boolean)`: the tablet's media volume,
/// perceptual 0..1. Stored also while there is no session; the host gets it once it announces
/// audio, and every later change right away. A no-op for the host without an audio session.
/// Kotlin must not report a volume it just applied from the host (it does the echo suppression).
#[jni::jni_mangle("com.librestatic.ezdisplay.NativeBridge")]
pub extern "system" fn set_local_volume<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    volume: jni::sys::jfloat,
    muted: jni::sys::jboolean,
) {
    env.with_env(|_env| -> jni::errors::Result<()> {
        let v = AudioVolume {
            volume: if volume.is_nan() {
                0.0
            } else {
                volume.clamp(0.0, 1.0)
            },
            muted,
        };
        *LOCAL_VOLUME.lock().unwrap_or_else(|e| e.into_inner()) = Some(v);
        if let Some(s) = session().as_ref() {
            s.set_audio_volume(v);
        }
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

/// `external fun pollHostVolume(): FloatArray` = `[seq, volume, muted(0/1)]`: the latest volume
/// the host sent in this session. `seq` grows with every message and is 0 while there is none.
/// It is exact in a float up to 2^24 messages, far more than a session sees.
#[jni::jni_mangle("com.librestatic.ezdisplay.NativeBridge")]
pub extern "system" fn poll_host_volume<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
) -> JFloatArray<'local> {
    env.with_env(|env| -> jni::errors::Result<JFloatArray<'local>> {
        let v: [f32; 3] = match session().as_ref().and_then(|s| s.host_audio_volume()) {
            Some((seq, a)) => [seq as f32, a.volume, f32::from(u8::from(a.muted))],
            None => [0.0; 3],
        };
        let arr = env.new_float_array(v.len())?;
        arr.set_region(env, 0, &v)?;
        Ok(arr)
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

/// `external fun inputTick()`: fires due gesture/pen timers.
#[jni::jni_mangle("com.librestatic.ezdisplay.NativeBridge")]
pub extern "system" fn input_tick<'local>(mut env: EnvUnowned<'local>, _class: JClass<'local>) {
    env.with_env(|_env| -> jni::errors::Result<()> {
        let now = now_us();
        let mut out = Vec::new();
        {
            let mut st = input();
            if let Some(engine) = st.engine.as_mut()
                && engine.next_deadline().is_some_and(|d| d <= now)
            {
                engine.tick(now, &mut out);
            }
        }
        send_all(&mut out);
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

/// `external fun pollView(): FloatArray` = `[scale, tx, ty, penX, penY]`; pen position is
/// normalized 0..=1 over the stream, or -1 when the pen is out of range.
#[jni::jni_mangle("com.librestatic.ezdisplay.NativeBridge")]
pub extern "system" fn poll_view<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
) -> JFloatArray<'local> {
    env.with_env(|env| -> jni::errors::Result<JFloatArray<'local>> {
        let v: [f32; 5] = match input().engine.as_ref() {
            Some(e) => {
                let ViewTransform { scale, tx, ty } = e.view();
                let (px, py) = e.pen_position().map_or((-1.0, -1.0), |(x, y)| {
                    (f32::from(x) / 65535.0, f32::from(y) / 65535.0)
                });
                [scale, tx, ty, px, py]
            }
            None => [1.0, 0.0, 0.0, -1.0, -1.0],
        };
        let arr = env.new_float_array(v.len())?;
        arr.set_region(env, 0, &v)?;
        Ok(arr)
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

/// `external fun setInputConfig(json: String): String?`: null on success, else the error
/// (the previous config stays).
#[jni::jni_mangle("com.librestatic.ezdisplay.NativeBridge")]
pub extern "system" fn set_input_config<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    json: JString<'local>,
) -> JString<'local> {
    env.with_env(|env| -> jni::errors::Result<JString<'local>> {
        let json = json.try_to_string(env)?;
        match InputConfig::from_json(&json) {
            Ok(cfg) => {
                let mut st = input();
                st.config = Some(cfg.clone());
                if let Some(e) = st.engine.as_mut() {
                    e.set_config(cfg);
                }
                Ok(JString::null())
            }
            Err(e) => env.new_string(e),
        }
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

/// `external fun setViewSize(width: Int, height: Int)`: size of the zoomed view in pixels.
#[jni::jni_mangle("com.librestatic.ezdisplay.NativeBridge")]
pub extern "system" fn set_view_size<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    width: jint,
    height: jint,
) {
    env.with_env(|_env| -> jni::errors::Result<()> {
        let (w, h) = (width.max(1) as f32, height.max(1) as f32);
        let mut st = input();
        st.view = (w, h);
        if let Some(e) = st.engine.as_mut() {
            e.set_view_size(w, h);
        } else {
            st.engine();
        }
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}
