//! [`VideoSource`] for KDE Plasma: portal virtual output -> PipeWire -> hardware encode.

use crate::adapter::{self, RenderNode};
use crate::ahead::{ConvertAhead, Item, Start};
use crate::encode::{Converter, Encoder, Kind, Pipeline, Plan, plan_encoders, probe_plan};
use crate::kscreen;
use crate::portal::{PortalCapture, SourceKind};
use crate::pw::{PwCapture, PwParams, Target, default_intel_modifiers};
use ezd_host_core::{CursorSink, EncodedFrame, PlatformOptions, SourceError, VideoSource};
use ezd_proto::{Codec, EncoderCaps, StreamConfig};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

type ProbeCache = Mutex<HashMap<(String, String), Result<(), String>>>;

fn cache() -> &'static ProbeCache {
    static C: OnceLock<ProbeCache> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// Probes `plan` once per process (the result is cached, including failures).
pub fn plan_works(plan: &Plan) -> Result<(), String> {
    let key = (plan.name.to_string(), plan.node.clone().unwrap_or_default());
    if let Some(r) = cache().lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return r.clone();
    }
    let r = std::panic::catch_unwind(|| probe_plan(plan))
        .unwrap_or_else(|_| Err("probe panicked".into()));
    match &r {
        Ok(()) => tracing::info!(encoder = plan.name, node = ?plan.node, "encoder probe ok"),
        Err(e) => tracing::info!(encoder = plan.name, node = ?plan.node, "encoder unusable: {e}"),
    }
    cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key, r.clone());
    r
}

fn portal_timeout() -> Duration {
    let secs = std::env::var("EZD_PORTAL_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(120);
    Duration::from_secs(secs)
}

/// Linux/KDE capture + encode source.
pub struct KdeVirtualSource {
    kind: SourceKind,
    output_name: Option<String>,
    node: Option<RenderNode>,
    plans: Vec<Plan>,
    running: Option<Running>,
    force_key: bool,
}

/// Converts and encodes capture frames.
enum Input {
    /// Both on the core thread (the default).
    Serial(Pipeline),
    /// Conversion on its own thread, one frame ahead of the encoder on the core thread.
    Ahead {
        // Field order matters for drop: join the converter thread first.
        conv: ConvertAhead,
        enc: Encoder,
    },
}

struct Running {
    // Field order matters for drop: stop converting, then capture, then close the portal session.
    input: Input,
    pw: PwCapture,
    _portal: PortalCapture,
    dmabuf_ok: bool,
    /// When the shared-memory fallback started.
    dmabuf_off_at: Option<Instant>,
}

/// After the shared-memory fallback starts, DMA-BUF frames that were already captured or
/// converted can still fail for this long; they are dropped instead of ending the stream.
const DMABUF_GRACE: Duration = Duration::from_secs(2);

/// Converting ahead is opt-in: `EZD_CONVERT_AHEAD=1` (late start) or `early`. On the Intel iGPU
/// it gives more frames during window animations but adds latency, because KWin already keeps
/// the GPU busy and the conversion slows the encoder down (`docs/phase0-findings.md`). The
/// benchmark's skip-encode mode needs the serial path, which drops frames before converting them.
fn convert_ahead() -> Option<Start> {
    if std::env::var_os("EZD_DIAG_SKIP_ENCODE").is_some() {
        return None;
    }
    match std::env::var("EZD_CONVERT_AHEAD").as_deref() {
        Ok("1") => Some(Start::Late),
        Ok("early") => Some(Start::Early),
        _ => None,
    }
}

impl KdeVirtualSource {
    /// Selects the adapter and probes the encoders (cached per process).
    pub fn new(opts: &PlatformOptions) -> KdeVirtualSource {
        let nodes = adapter::list_render_nodes();
        let node = adapter::select(&nodes, opts.adapter.as_deref(), opts.prefer_dgpu);
        if let Some(s) = &node {
            tracing::info!(node = %s.node.description(), reason = ?s.reason, "adapter selected");
        } else {
            tracing::warn!("no DRM render node found; software encoding only");
        }
        let node = node.map(|s| s.node);
        let plans = plan_encoders(node.as_ref(), &|p| plan_works(p).is_ok());
        KdeVirtualSource {
            kind: if opts.output_name.is_some() {
                SourceKind::Monitor
            } else {
                SourceKind::Virtual
            },
            output_name: opts.output_name.clone(),
            node,
            plans,
            running: None,
            force_key: false,
        }
    }

    fn dmabuf_modifiers(&self, plan: &Plan) -> Option<Vec<u64>> {
        if std::env::var_os("EZD_NO_DMABUF").is_some() {
            return None;
        }
        let node = self.node.as_ref()?;
        // Zero-copy only works when the encoder GPU also renders the compositor output.
        (plan.kind == Kind::Vaapi && (node.compositor || self.output_name.is_none()))
            .then(default_intel_modifiers)
    }
}

impl VideoSource for KdeVirtualSource {
    fn encoders(&self) -> Vec<EncoderCaps> {
        self.plans
            .iter()
            .map(|p| EncoderCaps {
                codec: Codec::H264,
                name: p.name.to_string(),
                hardware: p.hardware(),
            })
            .collect()
    }

    fn gpu_description(&self) -> String {
        self.node
            .as_ref()
            .map(RenderNode::description)
            .unwrap_or_else(|| "none (software)".into())
    }

    fn start(&mut self, cfg: &StreamConfig, cursor: CursorSink) -> Result<(), SourceError> {
        if cfg.codec != Codec::H264 {
            return Err(SourceError::msg("the Linux host only produces H.264"));
        }
        let plan = self
            .plans
            .iter()
            .find(|p| p.name == cfg.encoder_name)
            .cloned()
            .ok_or_else(|| SourceError::msg(format!("encoder {} not offered", cfg.encoder_name)))?;
        let virtual_kind = self.kind == SourceKind::Virtual;
        let before = if virtual_kind && kscreen::available() {
            kscreen::list_outputs().ok()
        } else {
            None
        };
        let mut portal = PortalCapture::open(self.kind, portal_timeout()).map_err(|e| {
            let hint = virtual_kind
                .then(|| kscreen::list_outputs().ok())
                .flatten()
                .and_then(|outs| kscreen::disabled_virtual_hint(&outs));
            SourceError::msg(match hint {
                Some(h) => format!("{e}. {h}"),
                None => e,
            })
        })?;
        tracing::info!(
            node = portal.node_id,
            restored = portal.restored,
            size = ?portal.size,
            position = ?portal.position,
            stream = ?portal.stream_id,
            "screencast session open"
        );

        if virtual_kind {
            match &before {
                Some(before) => switch_mode(before, cfg),
                None => tracing::warn!(
                    "kscreen-doctor unavailable: the virtual display keeps its default size"
                ),
            }
        }

        let params = PwParams {
            width: cfg.width,
            height: cfg.height,
            fps: cfg.refresh_mhz.div_ceil(1000),
            dmabuf_modifiers: self.dmabuf_modifiers(&plan),
        };
        let fd = portal.fd.take();
        let pw = PwCapture::start(fd, Target::Node(portal.node_id), params, cursor)
            .map_err(SourceError::msg)?;
        let fmt = pw
            .wait_format((cfg.width, cfg.height), Duration::from_secs(5))
            .or_else(|| pw.wait_any_format(Duration::from_secs(5)))
            .ok_or_else(|| SourceError::msg("PipeWire stream did not negotiate a format"))?;
        if (fmt.width, fmt.height) != (cfg.width, cfg.height) {
            tracing::warn!(
                "stream is {}x{} but {}x{} was negotiated with the client; encoding at the stream size",
                fmt.width,
                fmt.height,
                cfg.width,
                cfg.height
            );
        }
        tracing::info!(?fmt, plan = ?plan, "capture format negotiated");
        let ahead = convert_ahead();
        tracing::info!(convert_ahead = ?ahead, "encode pipeline ready");
        let input = match ahead {
            Some(start) => Input::Ahead {
                enc: Encoder::new(plan.name, cfg.refresh_mhz, cfg.bitrate_kbps),
                conv: ConvertAhead::start(
                    Converter::new(plan, cfg.refresh_mhz),
                    pw.frames(),
                    start,
                )
                .map_err(SourceError::msg)?,
            },
            None => Input::Serial(Pipeline::new(plan, cfg.refresh_mhz, cfg.bitrate_kbps)),
        };
        self.running = Some(Running {
            input,
            pw,
            _portal: portal,
            dmabuf_ok: fmt.modifier.is_some(),
            dmabuf_off_at: None,
        });
        self.force_key = true;
        Ok(())
    }

    fn next_frame(&mut self, timeout: Duration) -> Result<Option<EncodedFrame>, SourceError> {
        let force = std::mem::take(&mut self.force_key);
        let Some(r) = self.running.as_mut() else {
            return Err(SourceError::msg("source not started"));
        };
        // `start_us`: when this frame's conversion (serial) or encoding (ahead) began.
        let (result, start_us, capture_us, seq, was_dma) = match &mut r.input {
            Input::Serial(pipeline) => {
                let frame = match r.pw.next(timeout) {
                    Ok(Some(f)) => f,
                    Ok(None) => {
                        self.force_key |= force;
                        return Ok(None);
                    }
                    Err(e) => return Err(SourceError::msg(e)),
                };
                // Benchmark aid (tools/kwin-anim-bench): drop every frame to measure KWin alone.
                if std::env::var_os("EZD_DIAG_SKIP_ENCODE").is_some() {
                    drop(frame);
                    return Ok(None);
                }
                let (capture_us, seq, dma) = (frame.capture_us, frame.seq, frame.is_dma());
                let start_us = ezd_host_core::now_us();
                (
                    pipeline.encode(frame, force),
                    start_us,
                    capture_us,
                    seq,
                    dma,
                )
            }
            Input::Ahead { conv, enc } => match conv.take(timeout) {
                None => {
                    self.force_key |= force;
                    return Ok(None);
                }
                Some(Item::Frame(p)) => {
                    let (capture_us, seq, dma) = (p.capture_us, p.seq, p.dma);
                    let start_us = ezd_host_core::now_us();
                    (enc.encode(p, force), start_us, capture_us, seq, dma)
                }
                Some(Item::Failed { dma, error }) => (Err(error), 0, 0, 0, dma),
                Some(Item::Closed(e)) => return Err(SourceError::msg(e)),
            },
        };
        match result {
            Ok(Some(o)) => {
                tracing::trace!(
                    path = o.path,
                    bytes = o.data.len(),
                    key = o.keyframe,
                    age_us = start_us.saturating_sub(capture_us),
                    encode_us = o.encoded_us.saturating_sub(start_us),
                    "encoded"
                );
                Ok(Some(EncodedFrame {
                    data: o.data,
                    keyframe: o.keyframe,
                    codec_config: o.codec_config,
                    capture_us,
                    encoded_us: o.encoded_us,
                    frame_id: Some(seq),
                }))
            }
            Ok(None) => {
                self.force_key |= force;
                Ok(None)
            }
            Err(e) if was_dma && r.dmabuf_ok => {
                tracing::warn!("DMA-BUF encode failed ({e}); falling back to shared memory");
                r.dmabuf_ok = false;
                r.dmabuf_off_at = Some(Instant::now());
                r.pw.disable_dmabuf();
                self.force_key = true;
                Ok(None)
            }
            Err(e) if was_dma && r.dmabuf_off_at.is_some_and(|t| t.elapsed() < DMABUF_GRACE) => {
                tracing::debug!("dropping a DMA-BUF frame during the fallback: {e}");
                self.force_key = true;
                Ok(None)
            }
            Err(e) => Err(SourceError::msg(e)),
        }
    }

    fn request_keyframe(&mut self) {
        self.force_key = true;
    }

    fn stop(&mut self) {
        // Drop order: converter and encoder, PipeWire, then the portal session (removes the
        // virtual output).
        if let Some(r) = &self.running {
            let (produced, coalesced) = r.pw.counters();
            tracing::info!(produced, coalesced, "capture stopped");
        }
        self.running = None;
    }
}

/// Best-effort switch of the new virtual output to the negotiated mode.
fn switch_mode(before: &[kscreen::KOutput], cfg: &StreamConfig) {
    let mut found = None;
    for _ in 0..20 {
        if let Ok(after) = kscreen::list_outputs() {
            found = kscreen::find_new_output(before, &after)
                .or_else(|| kscreen::find_virtual_output(&after))
                .map(|o| o.name.clone());
            if found.is_some() {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let Some(name) = found else {
        tracing::warn!("could not find the new virtual output; keeping its default mode");
        return;
    };
    match kscreen::apply_mode(&name, cfg.width, cfg.height, cfg.refresh_mhz) {
        Ok(_) => tracing::info!("{name} set to {}x{}", cfg.width, cfg.height),
        Err(e) => tracing::warn!("could not set {name} to {}x{}: {e}", cfg.width, cfg.height),
    }
}
