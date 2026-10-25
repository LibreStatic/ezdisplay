//! [`VideoSource`] for KDE Plasma: portal virtual output -> PipeWire -> hardware encode.

use crate::adapter::{self, RenderNode};
use crate::ahead::{ConvertAhead, Item, Start};
use crate::encode::{
    Converter, Encoder, Kind, Pipeline, Plan, StreamTags, plan_encoders, plan_hdr_encoders,
    probe_plan,
};
use crate::kscreen;
use crate::portal::{PortalCapture, SourceKind};
use crate::pw::{PwCapture, PwParams, Target, default_intel_modifiers, default_nvidia_modifiers};
use ezd_host_core::{
    CursorSink, DisplayMode, EncodedFrame, OutputInfo, PlatformOptions, SourceError, SourceHdr,
    VideoSource,
};
use ezd_proto::{Codec, DynamicRange, EncoderCaps, StreamColor, StreamConfig};
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

/// What the last capture showed about the HDR state of the compositor's output. The session asks
/// `source_hdr` before the capture exists, so the answer comes from the previous capture in this
/// process: when the output turns HDR10 (or stops being), `start` fails once and the tablet's
/// reconnect negotiates again with the new state. Compositors that send 8-bit sRGB (KWin today)
/// never set it.
static OBSERVED_HDR: Mutex<Option<SourceHdr>> = Mutex::new(None);

fn observed_hdr() -> Option<SourceHdr> {
    *OBSERVED_HDR.lock().unwrap_or_else(|e| e.into_inner())
}

fn observe_hdr(hdr: Option<SourceHdr>) {
    *OBSERVED_HDR.lock().unwrap_or_else(|e| e.into_inner()) = hdr;
}

/// Checks the stream the compositor delivers against what the session negotiated: `expected` is
/// the HDR state the negotiation assumed, `actual` the one the capture shows.
fn hdr_mismatch(expected: bool, actual: bool) -> Option<&'static str> {
    match (expected, actual) {
        (false, true) => Some("the display switched to HDR10"),
        (true, false) => Some("the display stopped sending HDR10"),
        _ => None,
    }
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
    /// HEVC Main10 plans; only probed once the compositor has shown HDR10.
    hdr_plans: Vec<Plan>,
    /// The HDR state the session was negotiated with (see [`OBSERVED_HDR`]).
    hdr_source: Option<SourceHdr>,
    /// Dynamic range negotiated for the session and the matching stream colour.
    range: DynamicRange,
    color: StreamColor,
    running: Option<Running>,
    force_key: bool,
    /// The output the stream shows (KDE virtual output or the captured monitor), after `start`.
    output: Option<kscreen::KOutput>,
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
        let hdr_source = observed_hdr();
        // Probing opens a Main10 encoder: only worth it when the compositor offers HDR10.
        let hdr_plans = if hdr_source.is_some() {
            plan_hdr_encoders(node.as_ref(), &|p| plan_works(p).is_ok())
        } else {
            Vec::new()
        };
        KdeVirtualSource {
            kind: if opts.output_name.is_some() {
                SourceKind::Monitor
            } else {
                SourceKind::Virtual
            },
            output_name: opts.output_name.clone(),
            node,
            plans,
            hdr_plans,
            hdr_source,
            range: DynamicRange::Sdr,
            color: StreamColor::sdr_bt709(),
            running: None,
            force_key: false,
            output: None,
        }
    }

    fn dmabuf_modifiers(&self, plan: &Plan) -> Option<Vec<u64>> {
        if std::env::var_os("EZD_NO_DMABUF").is_some() {
            return None;
        }
        let node = self.node.as_ref()?;
        // Zero-copy only works when the encoder GPU also renders the compositor output.
        match plan.kind {
            Kind::Vaapi => {
                (node.compositor || self.output_name.is_none()).then(default_intel_modifiers)
            }
            Kind::Nvenc => node.compositor.then(default_nvidia_modifiers),
            Kind::X264 => None,
        }
    }
}

impl VideoSource for KdeVirtualSource {
    fn encoders(&self) -> Vec<EncoderCaps> {
        let codec = |p: &Plan| {
            if p.hdr10() { Codec::Hevc } else { Codec::H264 }
        };
        self.plans
            .iter()
            .chain(&self.hdr_plans)
            .map(|p| EncoderCaps {
                codec: codec(p),
                name: p.name.to_string(),
                hardware: p.hardware(),
            })
            .collect()
    }

    fn source_hdr(&self) -> Option<SourceHdr> {
        self.hdr_source
    }

    fn main10_codecs(&self) -> Vec<Codec> {
        if self.hdr_plans.is_empty() {
            Vec::new()
        } else {
            vec![Codec::Hevc]
        }
    }

    fn set_dynamic_range(&mut self, range: DynamicRange, color: &StreamColor) {
        self.range = range;
        self.color = *color;
    }

    fn gpu_description(&self) -> String {
        self.node
            .as_ref()
            .map(RenderNode::description)
            .unwrap_or_else(|| "none (software)".into())
    }

    fn start(&mut self, cfg: &StreamConfig, cursor: CursorSink) -> Result<(), SourceError> {
        // H.264 is the SDR codec; HEVC only exists here as HDR10 (Main10).
        let hdr10 = matches!(self.range, DynamicRange::Hdr10 { codec } if codec == cfg.codec);
        match (cfg.codec, hdr10) {
            (Codec::H264, false) | (Codec::Hevc, true) => {}
            (Codec::Hevc, false) => {
                return Err(SourceError::msg(
                    "the Linux host only produces HEVC for HDR10 streams",
                ));
            }
            (codec, _) => {
                return Err(SourceError::msg(format!(
                    "the Linux host cannot encode {codec:?}"
                )));
            }
        }
        let plan = self
            .plans
            .iter()
            .chain(&self.hdr_plans)
            .find(|p| p.name == cfg.encoder_name && p.hdr10() == hdr10)
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

        self.output = None;
        if virtual_kind {
            match &before {
                Some(before) => self.output = switch_mode(before, cfg),
                None => tracing::warn!(
                    "kscreen-doctor unavailable: the virtual display keeps its default size"
                ),
            }
        } else if let Some(name) = &self.output_name
            && kscreen::available()
        {
            self.output = kscreen::list_outputs()
                .ok()
                .and_then(|outs| outs.into_iter().find(|o| &o.name == name));
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
        let hdr_now = fmt.is_hdr10();
        if let Some(why) = hdr_mismatch(self.hdr_source.is_some(), hdr_now) {
            // The negotiation (codec, dynamic range) was made for the other state. Remember the
            // new one and end the session; the tablet reconnects and negotiates again.
            observe_hdr(hdr_now.then(SourceHdr::default));
            return Err(SourceError::msg(format!("{why}; reconnecting")));
        }
        let tags = StreamTags::from(&self.color);
        let ahead = convert_ahead();
        tracing::info!(convert_ahead = ?ahead, "encode pipeline ready");
        let input = match ahead {
            Some(start) => Input::Ahead {
                enc: Encoder::new(plan.name, cfg.refresh_mhz, cfg.bitrate_kbps).with_tags(tags),
                conv: ConvertAhead::start(
                    Converter::new(plan, cfg.refresh_mhz).with_tags(tags),
                    pw.frames(),
                    start,
                )
                .map_err(SourceError::msg)?,
            },
            None => Input::Serial(
                Pipeline::new(plan, cfg.refresh_mhz, cfg.bitrate_kbps).with_tags(tags),
            ),
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

    fn output(&self) -> Option<OutputInfo> {
        let o = self.output.as_ref()?;
        Some(OutputInfo {
            name: o.name.clone(),
            mode: DisplayMode {
                width: o.size.0,
                height: o.size.1,
                refresh_mhz: o
                    .modes
                    .iter()
                    .find(|m| m.id == o.current_mode_id)
                    .map_or(0, |m| (m.refresh_hz * 1000.0).round() as u32),
            },
            origin: o.pos,
            adapter: None,
        })
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
/// Returns the output as KWin reports it after the switch.
fn switch_mode(before: &[kscreen::KOutput], cfg: &StreamConfig) -> Option<kscreen::KOutput> {
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
        return None;
    };
    match kscreen::apply_mode(&name, cfg.width, cfg.height, cfg.refresh_mhz) {
        Ok(o) => {
            tracing::info!("{name} set to {}x{}", cfg.width, cfg.height);
            // Re-list: the position and scale can change with the mode.
            kscreen::list_outputs()
                .ok()
                .and_then(|outs| outs.into_iter().find(|x| x.name == name))
                .or(Some(o))
        }
        Err(e) => {
            tracing::warn!("could not set {name} to {}x{}: {e}", cfg.width, cfg.height);
            kscreen::list_outputs()
                .ok()
                .and_then(|outs| outs.into_iter().find(|x| x.name == name))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hdr_state_changes_end_the_session_for_a_new_negotiation() {
        // Today's compositors: 8-bit sRGB all the time, nothing to renegotiate.
        assert_eq!(hdr_mismatch(false, false), None);
        // HDR10 negotiated and still HDR10.
        assert_eq!(hdr_mismatch(true, true), None);
        // The output turned HDR10 (or stopped being) after the negotiation.
        assert!(hdr_mismatch(false, true).is_some());
        assert!(hdr_mismatch(true, false).is_some());
    }
}
