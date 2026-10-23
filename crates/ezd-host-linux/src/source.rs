//! [`VideoSource`] for KDE Plasma: portal virtual output -> PipeWire -> hardware encode.

use crate::adapter::{self, RenderNode};
use crate::encode::{Kind, Pipeline, Plan, plan_encoders, probe_plan};
use crate::kscreen;
use crate::portal::{PortalCapture, SourceKind};
use crate::pw::{PwCapture, PwParams, Target, default_intel_modifiers};
use ezd_host_core::{CursorSink, EncodedFrame, PlatformOptions, SourceError, VideoSource};
use ezd_proto::{Codec, EncoderCaps, StreamConfig};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

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

struct Running {
    // Field order matters for drop: stop capture before closing the portal session.
    pipeline: Pipeline,
    pw: PwCapture,
    _portal: PortalCapture,
    dmabuf_ok: bool,
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
        let pipeline = Pipeline::new(plan, cfg.refresh_mhz, cfg.bitrate_kbps);
        self.running = Some(Running {
            pipeline,
            pw,
            _portal: portal,
            dmabuf_ok: fmt.modifier.is_some(),
        });
        self.force_key = true;
        Ok(())
    }

    fn next_frame(&mut self, timeout: Duration) -> Result<Option<EncodedFrame>, SourceError> {
        let force = std::mem::take(&mut self.force_key);
        let Some(r) = self.running.as_mut() else {
            return Err(SourceError::msg("source not started"));
        };
        let frame = match r.pw.next(timeout) {
            Ok(Some(f)) => f,
            Ok(None) => {
                self.force_key |= force;
                return Ok(None);
            }
            Err(e) => return Err(SourceError::msg(e)),
        };
        let capture_us = frame.capture_us;
        let seq = frame.seq;
        let was_dma = frame.is_dma();
        let dequeued_us = ezd_host_core::now_us();
        match r.pipeline.encode(frame, force) {
            Ok(Some(o)) => {
                tracing::trace!(
                    path = o.path,
                    bytes = o.data.len(),
                    key = o.keyframe,
                    age_us = dequeued_us.saturating_sub(capture_us),
                    encode_us = o.encoded_us.saturating_sub(dequeued_us),
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
                r.pw.disable_dmabuf();
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
        // Drop order: encoder, PipeWire, then the portal session (removes the virtual output).
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
