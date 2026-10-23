//! [`VideoSource`] for Windows: VDD output -> Desktop Duplication -> GPU encoder.

use super::dxgi::AdapterEntry;
use crate::policy::{self, AdapterInfo, Decision};
use ezd_host_core::{PlatformError, PlatformOptions};

#[cfg(feature = "ffmpeg")]
pub use imp::*;

/// Chooses the adapter for these options (shared by `create_virtual_source` and `diag`).
pub fn decide(
    adapters: &[AdapterEntry],
    opts: &PlatformOptions,
) -> Result<Decision, policy::PolicyError> {
    let infos: Vec<AdapterInfo> = adapters.iter().map(|a| a.info.clone()).collect();
    policy::decide(&infos, opts)
}

/// Entry point used by the crate root.
#[cfg(not(feature = "ffmpeg"))]
pub fn create_virtual_source(
    _opts: &PlatformOptions,
) -> Result<Box<dyn ezd_host_core::VideoSource>, PlatformError> {
    Err(PlatformError::Other("built without ffmpeg".into()))
}

/// Entry point used by the crate root.
#[cfg(feature = "ffmpeg")]
pub fn create_virtual_source(
    opts: &PlatformOptions,
) -> Result<Box<dyn ezd_host_core::VideoSource>, PlatformError> {
    let dpi = super::util::ensure_dpi_aware();
    tracing::debug!("DPI awareness: {dpi}");
    let adapters = super::dxgi::enumerate().map_err(PlatformError::Other)?;
    let decision = decide(&adapters, opts).map_err(|e| PlatformError::Other(e.to_string()))?;
    tracing::info!(
        "GPU policy: {} ({}) because: {}",
        decision.adapter.name,
        decision.adapter.luid_hex(),
        decision.reason.as_str()
    );
    imp::WindowsSource::new(opts.clone(), adapters, decision)
        .map(|s| Box::new(s) as Box<dyn ezd_host_core::VideoSource>)
        .map_err(PlatformError::Other)
}

#[cfg(feature = "ffmpeg")]
mod imp {
    use super::super::capture::{Capture, create_bgra_texture};
    use super::super::dxgi::{self, AdapterEntry};
    use super::super::encode::{self, GpuEncoder};
    use super::super::vdd;
    use crate::policy::{AdapterInfo, Decision};
    use crate::vdd_xml::Mode;
    use ezd_host_core::{
        CursorSink, EncodedFrame, PlatformOptions, SourceError, VideoSource, now_us,
    };
    use ezd_proto::{Codec, EncoderCaps, StreamConfig};
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    use std::time::Duration;

    /// Probe size used to decide which encoders work on an adapter.
    pub const PROBE_SIZE: (u32, u32) = (1920, 1080);

    type ProbeCache = Mutex<HashMap<(u64, String), Result<String, String>>>;

    /// Result of opening `name` on the adapter `entry` and encoding one frame; cached per process.
    pub fn probe_encoder(entry: &AdapterEntry, name: &str) -> Result<String, String> {
        static CACHE: OnceLock<ProbeCache> = OnceLock::new();
        let cache = CACHE.get_or_init(Default::default);
        let key = (entry.info.luid, name.to_string());
        if let Some(r) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
            return r.clone();
        }
        let r = if !ezd_ffmpeg::encoder_available(name) {
            Err("encoder not compiled into this FFmpeg".to_string())
        } else {
            dxgi::create_device(&entry.adapter)
                .and_then(|dev| encode::probe(&dev, name, PROBE_SIZE.0, PROBE_SIZE.1))
        };
        cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, r.clone());
        r
    }

    struct Running {
        capture: Capture,
        encoder: GpuEncoder,
        // Keeps the device alive for as long as the capture and encoder use it.
        _device: windows::Win32::Graphics::Direct3D11::ID3D11Device,
        last_seq: u64,
    }

    /// The Windows capture + encode pipeline.
    pub struct WindowsSource {
        opts: PlatformOptions,
        adapters: Vec<AdapterEntry>,
        decision: Decision,
        encoders: Vec<EncoderCaps>,
        running: Option<Running>,
        want_key: bool,
    }

    impl WindowsSource {
        /// Probes the encoders of the chosen adapter and prepares the source.
        pub fn new(
            opts: PlatformOptions,
            adapters: Vec<AdapterEntry>,
            decision: Decision,
        ) -> Result<Self, String> {
            let entry = adapters
                .iter()
                .find(|a| a.info.luid == decision.adapter.luid)
                .ok_or("the chosen adapter disappeared")?;
            let mut encoders = Vec::new();
            let mut failures = Vec::new();
            for name in &decision.encoder_chain {
                match probe_encoder(entry, name) {
                    Ok(note) => {
                        tracing::info!("encoder {name} works on {}: {note}", entry.info.name);
                        encoders.push(EncoderCaps {
                            codec: Codec::H264,
                            name: (*name).to_string(),
                            hardware: *name != "libx264",
                        });
                    }
                    Err(e) => {
                        tracing::warn!("encoder {name} unusable on {}: {e}", entry.info.name);
                        failures.push(format!("{name}: {e}"));
                    }
                }
            }
            if encoders.is_empty() {
                return Err(format!(
                    "no working H.264 encoder on {} ({}); tried: {}",
                    entry.info.name,
                    entry.info.luid_hex(),
                    failures.join("; ")
                ));
            }
            Ok(WindowsSource {
                opts,
                adapters,
                decision,
                encoders,
                running: None,
                want_key: true,
            })
        }

        fn start_inner(
            &mut self,
            cfg: &StreamConfig,
            cursor: CursorSink,
        ) -> Result<Running, String> {
            if cfg.codec != Codec::H264 {
                return Err(format!(
                    "codec {:?} is not supported by this host",
                    cfg.codec
                ));
            }
            if !self.encoders.iter().any(|e| e.name == cfg.encoder_name) {
                return Err(format!(
                    "encoder {:?} is not available here (have: {})",
                    cfg.encoder_name,
                    self.encoders
                        .iter()
                        .map(|e| e.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            let (w, h) = (cfg.width, cfg.height);
            let chosen: AdapterInfo = self.decision.adapter.clone();
            let (adapter, output) = if let Some(name) = self.opts.output_name.as_deref() {
                // Capture an existing output; no virtual display is created or modified.
                let fresh = dxgi::enumerate()?;
                let (a, o) = dxgi::find_output(&fresh, name).ok_or_else(|| {
                    let all: Vec<_> = fresh
                        .iter()
                        .flat_map(|a| a.outputs.iter().map(|o| o.device_name.clone()))
                        .collect();
                    format!("no output named {name:?}; available: {}", all.join(", "))
                })?;
                if !o.attached {
                    return Err(format!("output {name} is not attached to the desktop"));
                }
                if a.info.luid != chosen.luid {
                    tracing::warn!(
                        "WARNING: output {name} lives on {:?} ({}), but the GPU policy chose {:?} ({}); \
                         capture and encode run on {:?}",
                        a.info.name,
                        a.info.luid_hex(),
                        chosen.name,
                        chosen.luid_hex(),
                        a.info.name
                    );
                }
                (a.clone(), o.clone())
            } else {
                let mode = Mode {
                    width: w,
                    height: h,
                    refresh_mhz: cfg.refresh_mhz,
                };
                let p = vdd::prepare(&chosen, mode)?;
                for n in &p.notes {
                    tracing::info!("vdd: {n}");
                }
                (p.adapter, p.output)
            };
            let device = dxgi::create_device(&adapter.adapter)?;
            let capture = Capture::start(&device, &output.output, w, h, cursor)?;
            let encoder = GpuEncoder::open(
                &cfg.encoder_name,
                &device,
                w,
                h,
                cfg.refresh_mhz,
                cfg.bitrate_kbps,
            )
            .map_err(|e| {
                format!(
                    "cannot open {} on {} ({}): {e}",
                    cfg.encoder_name,
                    adapter.info.name,
                    adapter.info.luid_hex()
                )
            })?;
            // Texture creation also validates the device before the first frame.
            drop(create_bgra_texture(&device, 2, 2)?);
            Ok(Running {
                capture,
                encoder,
                _device: device,
                last_seq: 0,
            })
        }
    }

    impl VideoSource for WindowsSource {
        fn encoders(&self) -> Vec<EncoderCaps> {
            self.encoders.clone()
        }

        fn gpu_description(&self) -> String {
            let a = &self.decision.adapter;
            format!(
                "{} ({}, {} MiB, LUID {})",
                a.name,
                a.vendor_name(),
                a.dedicated_vram >> 20,
                a.luid_hex()
            )
        }

        fn start(&mut self, cfg: &StreamConfig, cursor: CursorSink) -> Result<(), SourceError> {
            let _ = &self.adapters;
            let r = self.start_inner(cfg, cursor).map_err(SourceError::msg)?;
            self.running = Some(r);
            self.want_key = true;
            Ok(())
        }

        fn next_frame(&mut self, timeout: Duration) -> Result<Option<EncodedFrame>, SourceError> {
            let Some(run) = self.running.as_mut() else {
                return Err(SourceError::msg("next_frame before start"));
            };
            let ticket = run
                .capture
                .wait_frame(run.last_seq, self.want_key, timeout)
                .map_err(SourceError::msg)?;
            let Some(t) = ticket else { return Ok(None) };
            let force = self.want_key;
            let out = run
                .encoder
                .encode(&run.capture.latest, force)
                .map_err(SourceError::msg)?;
            run.last_seq = t.seq;
            let Some(o) = out else { return Ok(None) };
            if o.keyframe {
                self.want_key = false;
            }
            Ok(Some(EncodedFrame {
                data: o.data,
                keyframe: o.keyframe,
                codec_config: o.codec_config,
                capture_us: t.capture_us,
                encoded_us: now_us(),
                frame_id: Some(t.seq),
            }))
        }

        fn request_keyframe(&mut self) {
            self.want_key = true;
        }

        fn stop(&mut self) {
            if let Some(mut r) = self.running.take() {
                let (frames, lost) = r.capture.stats();
                tracing::info!("capture stopped: {frames} frames, {lost} access-lost events");
                r.capture.stop();
            }
        }
    }
}
