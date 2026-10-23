use crate::encoder::{Encoder, EncoderConfig, RawPixelFormat};
use crate::error::init_logging;
use crate::hw::{HwDevice, HwKind};
use ffmpeg_sys_next as ffi;
use std::ffi::CString;

/// Outcome of probing an encoder: failures are data, not errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeResult {
    /// Whether an encoder produced a packet for one test frame.
    pub ok: bool,
    /// Failure reason, or a short success note.
    pub detail: String,
}

/// True if this FFmpeg build contains an encoder with this name.
pub fn encoder_available(name: &str) -> bool {
    init_logging();
    let Ok(c) = CString::new(name) else {
        return false;
    };
    // SAFETY: c is a valid NUL-terminated string.
    unsafe { !ffi::avcodec_find_encoder_by_name(c.as_ptr()).is_null() }
}

/// Names from `candidates` that exist in this FFmpeg build.
pub fn software_encoders() -> Vec<&'static str> {
    ["libx264", "libopenh264"]
        .into_iter()
        .filter(|n| encoder_available(n))
        .collect()
}

/// Opens `name` and encodes one frame (software frames, no device).
pub fn probe_encoder(name: &str, width: u32, height: u32) -> bool {
    probe_encoder_with(name, width, height, None).ok
}

/// Like [`probe_encoder`], optionally through a hardware device
/// (`Some((HwKind::Vaapi, Some("/dev/dri/renderD129")))`).
pub fn probe_encoder_with(
    name: &str,
    width: u32,
    height: u32,
    hw: Option<(HwKind, Option<&str>)>,
) -> ProbeResult {
    let fail = |e: String| ProbeResult {
        ok: false,
        detail: e,
    };
    if !encoder_available(name) {
        return fail("encoder not compiled into this FFmpeg".into());
    }
    let dev = match hw {
        Some((kind, path)) => match HwDevice::open(kind, path) {
            Ok(d) => Some(d),
            Err(e) => return fail(e.to_string()),
        },
        None => None,
    };
    let mut cfg = EncoderConfig::new(name, width, height, 60_000, 8_000);
    cfg.pix_fmt = RawPixelFormat::I420;
    cfg.hw = dev;
    let mut enc = match Encoder::open(cfg) {
        Ok(e) => e,
        Err(e) => return fail(e.to_string()),
    };
    let frame = vec![128u8; (width * height * 3 / 2) as usize];
    let res = (|| {
        enc.send_frame(&frame, true)?;
        let mut got = enc.receive_packet()?;
        if got.is_none() {
            got = enc.flush()?.into_iter().next();
        }
        Ok::<_, crate::FfmpegError>(got)
    })();
    match res {
        Ok(Some(p)) => ProbeResult {
            ok: true,
            detail: format!("encoded {} bytes", p.data.len()),
        },
        Ok(None) => fail("no packet produced".into()),
        Err(e) => fail(e.to_string()),
    }
}
