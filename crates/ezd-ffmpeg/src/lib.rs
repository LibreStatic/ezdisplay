//! Small safe wrapper around the parts of FFmpeg ezdisplay needs.
//!
//! * [`Encoder`]: low-latency encoder with per-encoder option presets and an
//!   optional [`HwDevice`] (software frames are uploaded to the device).
//! * [`Decoder`]: low-delay decoder for tests and the desktop fake client.
//! * [`SwVideoSource`]: [`ezd_host_core::VideoSource`] built from
//!   `SyntheticFrames` plus a software encoder.
//! * [`probe`]: "does this encoder actually work here" checks used by `diag`.
//!
//! All `unsafe` in the workspace lives in this crate. Extension point for
//! zero-copy backends (VAAPI/CUDA/D3D11 frames produced by the capture
//! pipeline): create the [`HwDevice`], pass it in [`EncoderConfig::hw`], and
//! submit frames you built yourself with [`Encoder::send_raw_frame`]; the raw
//! FFmpeg bindings are re-exported as [`sys`].
#![allow(unsafe_code)]
// Bindgen constant types (i32 vs u32) differ between platforms; the casts keep one code base portable.
#![allow(clippy::unnecessary_cast)]

mod color;
mod decoder;
mod encoder;
mod error;
mod hw;
mod options;
mod probe;
mod rate;
mod source;
#[cfg(target_os = "linux")]
mod vulkan;

pub use color::{ColorInfo, HdrMeta};
pub use decoder::{DecodedFrame, Decoder};
pub use encoder::{EncodedPacket, Encoder, EncoderConfig, RawPixelFormat};
pub use error::FfmpegError;
pub use ffmpeg_sys_next as sys;
pub use hw::{HwDevice, HwKind};
pub use options::{option_sets, option_sets_for};
pub use probe::{
    ProbeResult, encoder_available, probe_encoder, probe_encoder_with, software_encoders,
};
pub use rate::{
    HIGH_QP, NVENC_CQ, PEAK_FACTOR, RateSettings, VAAPI_QP, VBV_FRAMES, X264_CRF, describe_rate,
    options_label, rate_settings,
};
pub use source::SwVideoSource;
#[cfg(target_os = "linux")]
pub use vulkan::vulkan_queue_families;
