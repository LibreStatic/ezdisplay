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
    use super::super::capture::FrameTicket;
    use super::super::capture::{Capture, CaptureOutput, create_bgra_texture};
    use super::super::dxgi::{self, AdapterEntry};
    use super::super::encode::{self, GpuEncoder, Output};
    use super::super::ffenc::StreamFormat;
    use super::super::hdr::{ScrgbToSdr, create_sdr_target};
    use super::super::{display, vdd};
    use crate::policy::{AdapterInfo, Decision};
    use crate::vdd_xml::Mode;
    use ezd_ffmpeg::HdrMeta;
    use ezd_host_core::{
        CursorSink, DisplayMode, EncodedFrame, OutputInfo, PlatformOptions, SourceError, SourceHdr,
        VideoSource, now_us,
    };
    use ezd_proto::{Codec, DynamicRange, EncoderCaps, StreamColor, StreamConfig};
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};
    use windows::Win32::Graphics::Direct3D11::{ID3D11RenderTargetView, ID3D11Texture2D};
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R16G16B16A16_FLOAT;

    /// How long a look at the output's HDR state is reused (`encoders`, `source_hdr` and
    /// `main10_codecs` are all asked at the start of a session).
    const HDR_CACHE: Duration = Duration::from_secs(2);

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

    /// Scales an HDR (scRGB) capture to SDR for clients that get no HDR10.
    struct ToneMap {
        conv: ScrgbToSdr,
        out: ID3D11Texture2D,
        rtv: ID3D11RenderTargetView,
    }

    struct Running {
        capture: Capture,
        encoder: GpuEncoder,
        tonemap: Option<ToneMap>,
        // Keeps the device alive for as long as the capture and encoder use it.
        _device: windows::Win32::Graphics::Direct3D11::ID3D11Device,
        output: OutputInfo,
        last_seq: u64,
        /// The frame handed to the encoder whose packet has not been returned yet (pipelined
        /// encoders only).
        in_flight: Option<FrameMeta>,
        timing: PipelineTiming,
    }

    /// What a packet needs from the capture it came from.
    #[derive(Clone, Copy)]
    struct FrameMeta {
        capture_us: u64,
        seq: u64,
        /// The frame was encoded as a forced keyframe.
        forced: bool,
    }

    /// Per-stage times of the capture + encode loop, logged every few seconds so a slow stream
    /// can be traced to the stage that limits it.
    struct PipelineTiming {
        since: Instant,
        /// When the previous `next_frame` returned: the time until the next call is spent by the
        /// session (sending, waiting for credit).
        returned: Option<Instant>,
        /// Time between calls since the last frame was returned (`Ok(None)` calls included).
        outside: Duration,
        captured: u64,
        frames: u32,
        // Sum and maximum, in microseconds: outside, wait, copy, send, receive, frame age. With a
        // pipelined encoder, copy is the convert and read back of the newest capture, send the
        // worker's encode time of the returned packet and receive the time blocked waiting for it.
        sum: [u64; 6],
        max: [u64; 6],
    }

    impl PipelineTiming {
        const PERIOD: Duration = Duration::from_secs(5);
        const LABELS: [&'static str; 6] = ["outside", "wait", "copy", "send", "receive", "age"];

        fn new() -> PipelineTiming {
            PipelineTiming {
                since: Instant::now(),
                returned: None,
                outside: Duration::ZERO,
                captured: 0,
                frames: 0,
                sum: [0; 6],
                max: [0; 6],
            }
        }

        fn add(&mut self, i: usize, d: Duration) {
            let us = d.as_micros() as u64;
            self.sum[i] += us;
            self.max[i] = self.max[i].max(us);
        }

        /// Logs and resets once a period has passed; `captured` is the capture thread's total.
        fn maybe_log(&mut self, name: &str, captured: u64) {
            let elapsed = self.since.elapsed();
            if elapsed < Self::PERIOD {
                return;
            }
            if self.frames > 0 {
                let n = u64::from(self.frames);
                let secs = elapsed.as_secs_f64();
                let stages: Vec<String> = Self::LABELS
                    .iter()
                    .enumerate()
                    .map(|(i, l)| {
                        format!(
                            "{l} {:.1}/{:.1}",
                            self.sum[i] as f64 / n as f64 / 1000.0,
                            self.max[i] as f64 / 1000.0
                        )
                    })
                    .collect();
                tracing::info!(
                    "{name}: {:.1} fps encoded, {:.1} fps captured; ms avg/max: {}",
                    n as f64 / secs,
                    captured.saturating_sub(self.captured) as f64 / secs,
                    stages.join(", ")
                );
            }
            *self = PipelineTiming {
                captured,
                ..PipelineTiming::new()
            };
        }
    }

    /// The Windows capture + encode pipeline.
    pub struct WindowsSource {
        opts: PlatformOptions,
        adapters: Vec<AdapterEntry>,
        decision: Decision,
        encoders: Vec<EncoderCaps>,
        running: Option<Running>,
        want_key: bool,
        /// Negotiated for the next `start` (SDR until `set_dynamic_range`).
        range: DynamicRange,
        color: StreamColor,
        hdr_seen: Mutex<Option<(Instant, Option<SourceHdr>)>>,
    }

    /// The output the session will capture, as it is now: the named one, or the VDD output if it
    /// exists and is attached. `None` when there is none to look at (the VDD is created at start).
    fn current_output(opts: &PlatformOptions) -> Option<dxgi::OutputEntry> {
        let adapters = dxgi::enumerate().ok()?;
        let found = match opts.output_name.as_deref() {
            Some(name) => dxgi::find_output(&adapters, name).map(|(_, o)| o.clone()),
            None => {
                let monitors = display::find_vdd_monitors();
                vdd::find_vdd_output(&adapters, &monitors)
                    .map(|(ai, oi)| adapters[ai].outputs[oi].clone())
            }
        };
        found.filter(|o| o.attached)
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
                range: DynamicRange::Sdr,
                color: StreamColor::sdr_bt709(),
                hdr_seen: Mutex::new(None),
            })
        }

        /// HDR10 properties of the output as it is right now (`None`: SDR or no output yet).
        fn detect_hdr(&self) -> Option<SourceHdr> {
            let mut seen = self.hdr_seen.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((at, v)) = *seen {
                if at.elapsed() < HDR_CACHE {
                    return v;
                }
            }
            let v = current_output(&self.opts)
                .and_then(|o| o.color)
                .and_then(|c| c.source_hdr());
            *seen = Some((Instant::now(), v));
            v
        }

        /// The HEVC Main10 encoder that works on the chosen adapter, if any (probed once per
        /// process, see [`probe_encoder`]). There is no software HDR10 encoder.
        fn hdr_encoder(&self) -> Option<&'static str> {
            let entry = self
                .adapters
                .iter()
                .find(|a| a.info.luid == self.decision.adapter.luid)?;
            self.decision
                .hdr_chain
                .iter()
                .copied()
                .find(|name| match probe_encoder(entry, name) {
                    Ok(note) => {
                        tracing::info!("HDR10 encoder {name} works on {}: {note}", entry.info.name);
                        true
                    }
                    Err(e) => {
                        tracing::warn!("HDR10 encoder {name} unusable on {}: {e}", entry.info.name);
                        false
                    }
                })
        }

        fn start_inner(
            &mut self,
            cfg: &StreamConfig,
            cursor: CursorSink,
        ) -> Result<Running, String> {
            let hdr10 = matches!(self.range, DynamicRange::Hdr10 { .. });
            let want_codec = if hdr10 { Codec::Hevc } else { Codec::H264 };
            if cfg.codec != want_codec {
                return Err(format!(
                    "codec {:?} does not fit this stream (host supports {:?} for {})",
                    cfg.codec,
                    want_codec,
                    if hdr10 { "HDR10" } else { "SDR" }
                ));
            }
            let known = if hdr10 {
                self.hdr_encoder() == Some(cfg.encoder_name.as_str())
            } else {
                self.encoders.iter().any(|e| e.name == cfg.encoder_name)
            };
            if !known {
                return Err(format!(
                    "encoder {:?} is not available here (have: {})",
                    cfg.encoder_name,
                    self.encoders()
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
            // The output's HDR state decides the capture format and the conversion. `output` was
            // enumerated just now, so it is current.
            let out_hdr = output.color.is_some_and(|c| c.hdr);
            if hdr10 && !out_hdr {
                // Negotiated before the display went back to SDR: end the session, the client
                // reconnects and renegotiates.
                return Err("display HDR mode changed".into());
            }
            let device = dxgi::create_device(&adapter.adapter)?;
            let capture = Capture::start(
                &device,
                &CaptureOutput {
                    output: &output.output,
                    device_name: &output.device_name,
                    hdr: out_hdr,
                    sdr_white_nits: output.sdr_white_nits,
                },
                w,
                h,
                cursor,
            )?;
            let fp16 = capture.format == DXGI_FORMAT_R16G16B16A16_FLOAT;
            if hdr10 && !fp16 {
                return Err(format!(
                    "the HDR desktop is delivered as {:?}, not FP16 scRGB; HDR10 needs FP16",
                    capture.format
                ));
            }
            let format = if hdr10 {
                GpuEncoder::hdr10_format(HdrMeta::from_stream_color(&self.color))
            } else {
                StreamFormat::SDR
            };
            // HDR output, SDR stream: scale to the SDR white level into a BGRA texture and encode
            // that as usual.
            let tonemap = if out_hdr && !hdr10 && fp16 {
                let (out, rtv) = create_sdr_target(&device, w, h)?;
                let conv = ScrgbToSdr::new(&device, w, h, output.sdr_white_nits)?;
                tracing::info!(
                    "HDR display, SDR stream: scaling to the SDR white level ({} cd/m2)",
                    output.sdr_white_nits
                );
                Some(ToneMap { conv, out, rtv })
            } else {
                if out_hdr && !hdr10 {
                    tracing::warn!(
                        "HDR display delivered as {:?}: DXGI tone-maps it itself",
                        capture.format
                    );
                }
                None
            };
            let encoder = GpuEncoder::open(
                &cfg.encoder_name,
                &device,
                w,
                h,
                cfg.refresh_mhz,
                cfg.bitrate_kbps,
                format,
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
            let output_info = OutputInfo {
                name: output.device_name.clone(),
                mode: DisplayMode {
                    width: w,
                    height: h,
                    refresh_mhz: cfg.refresh_mhz,
                },
                origin: (output.rect.left, output.rect.top),
                adapter: Some(adapter.info.luid_hex()),
            };
            Ok(Running {
                capture,
                encoder,
                tonemap,
                _device: device,
                output: output_info,
                last_seq: 0,
                in_flight: None,
                timing: PipelineTiming::new(),
            })
        }
    }

    /// Converts the capture `t` and hands it to the encoder. Returns the packet that came out
    /// with the frame it belongs to: the previous frame's on a pipelined encoder, this one's
    /// otherwise.
    fn submit(
        run: &mut Running,
        force: bool,
        t: FrameTicket,
    ) -> Result<Option<(Output, FrameMeta)>, SourceError> {
        let src = match run.tonemap.as_mut() {
            Some(t) => {
                t.conv.set_white_nits(run.capture.sdr_white_nits());
                t.conv
                    .convert(&run.capture.latest, &t.rtv)
                    .map_err(SourceError::msg)?;
                &t.out
            }
            None => &run.capture.latest,
        };
        let out = run.encoder.push(src, force).map_err(SourceError::msg)?;
        run.last_seq = t.seq;
        run.timing.add(2, run.encoder.stages[0]);
        let meta = FrameMeta {
            capture_us: t.capture_us,
            seq: t.seq,
            forced: force,
        };
        let owner = if run.encoder.pipelined() {
            run.in_flight.replace(meta)
        } else {
            Some(meta)
        };
        Ok(deliver(run, out, owner))
    }

    /// Waits for the packet of the frame in flight, if any.
    fn finish(run: &mut Running) -> Result<Option<(Output, FrameMeta)>, SourceError> {
        let out = run.encoder.finish().map_err(SourceError::msg)?;
        let owner = run.in_flight.take();
        Ok(deliver(run, out, owner))
    }

    /// Pairs a packet with its frame and records the timing of a frame leaving the pipeline.
    fn deliver(
        run: &mut Running,
        out: Option<Output>,
        meta: Option<FrameMeta>,
    ) -> Option<(Output, FrameMeta)> {
        let (o, m) = out.zip(meta)?;
        let timing = &mut run.timing;
        let outside = std::mem::take(&mut timing.outside);
        timing.add(0, outside);
        timing.add(3, run.encoder.stages[1]);
        timing.add(4, run.encoder.stages[2]);
        timing.add(
            5,
            Duration::from_micros(now_us().saturating_sub(m.capture_us)),
        );
        timing.frames += 1;
        timing.maybe_log(run.encoder.name(), run.capture.stats().0);
        Some((o, m))
    }

    impl VideoSource for WindowsSource {
        fn encoders(&self) -> Vec<EncoderCaps> {
            let mut v = self.encoders.clone();
            // The HEVC Main10 encoder is only offered while the display is HDR, so a host without
            // HDR content advertises exactly what it always did.
            if self.detect_hdr().is_some() {
                if let Some(name) = self.hdr_encoder() {
                    v.push(EncoderCaps {
                        codec: Codec::Hevc,
                        name: name.to_string(),
                        hardware: true,
                    });
                }
            }
            v
        }

        fn source_hdr(&self) -> Option<SourceHdr> {
            self.detect_hdr()
        }

        fn main10_codecs(&self) -> Vec<Codec> {
            if self.detect_hdr().is_some() && self.hdr_encoder().is_some() {
                vec![Codec::Hevc]
            } else {
                Vec::new()
            }
        }

        fn set_dynamic_range(&mut self, range: DynamicRange, color: &StreamColor) {
            self.range = range;
            self.color = *color;
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
            let called = Instant::now();
            if let Some(r) = run.timing.returned {
                run.timing.outside += called - r;
            }
            // A forced keyframe already in flight covers the request.
            let force = self.want_key && !run.in_flight.is_some_and(|m| m.forced);
            let done = if run.encoder.in_flight() {
                // Encoding the previous frame. Take a newer capture if there is one, so the D3D11
                // work overlaps the encode; if not, return the packet now.
                let ticket = run
                    .capture
                    .wait_frame(run.last_seq, false, Duration::ZERO)
                    .map_err(SourceError::msg)?;
                match ticket {
                    Some(t) => submit(run, force, t)?,
                    None => finish(run)?,
                }
            } else {
                let ticket = run
                    .capture
                    .wait_frame(run.last_seq, self.want_key, timeout)
                    .map_err(SourceError::msg)?;
                let Some(t) = ticket else {
                    run.timing.returned = Some(Instant::now());
                    return Ok(None);
                };
                run.timing.add(1, called.elapsed());
                // A pipelined encoder returns nothing for this frame: the session calls again at
                // once, and the packet is collected then.
                submit(run, force, t)?
            };
            run.timing.returned = Some(Instant::now());
            let Some((o, m)) = done else { return Ok(None) };
            if o.keyframe {
                self.want_key = false;
            }
            Ok(Some(EncodedFrame {
                data: o.data,
                keyframe: o.keyframe,
                codec_config: o.codec_config,
                capture_us: m.capture_us,
                encoded_us: now_us(),
                frame_id: Some(m.seq),
            }))
        }

        fn output(&self) -> Option<OutputInfo> {
            self.running.as_ref().map(|r| r.output.clone())
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
