//! JNI entry points of `io.github.facum.ezdisplay.NativeBridge`.
//!
//! Everything is polled from Kotlin (cursor, stats): native threads never call back into the JVM.

use crate::caps::parse_caps;
use crate::decoder::SendWindow;
use crate::session::Session;
use jni::EnvUnowned;
use jni::errors::ThrowRuntimeExAndDefault;
use jni::objects::{JByteArray, JClass, JLongArray, JObject, JString};
use jni::sys::jint;
use ndk::native_window::NativeWindow;
use std::sync::Mutex;
use std::sync::Once;

static SESSION: Mutex<Option<Session>> = Mutex::new(None);
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

/// `external fun start(surface: Surface?, host: String, port: Int, capsJson: String): String?`
/// Returns null on success or an error message.
#[jni::jni_mangle("io.github.facum.ezdisplay.NativeBridge")]
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
        let mut guard = session();
        if let Some(old) = guard.take() {
            old.stop();
        }
        log::info!("start {host}:{port} surface={}", window.is_some());
        *guard = Some(Session::start(window, host, port, caps));
        Ok(JString::null())
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

/// `external fun stop()`
#[jni::jni_mangle("io.github.facum.ezdisplay.NativeBridge")]
pub extern "system" fn stop<'local>(mut env: EnvUnowned<'local>, _class: JClass<'local>) {
    env.with_env(|_env| -> jni::errors::Result<()> {
        if let Some(s) = session().take() {
            s.stop();
        }
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

/// `external fun surfaceChanged(surface: Surface)`: a (new) surface is available.
#[jni::jni_mangle("io.github.facum.ezdisplay.NativeBridge")]
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
#[jni::jni_mangle("io.github.facum.ezdisplay.NativeBridge")]
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
#[jni::jni_mangle("io.github.facum.ezdisplay.NativeBridge")]
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
#[jni::jni_mangle("io.github.facum.ezdisplay.NativeBridge")]
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
#[jni::jni_mangle("io.github.facum.ezdisplay.NativeBridge")]
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
#[jni::jni_mangle("io.github.facum.ezdisplay.NativeBridge")]
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
