//! Session phase and the JSON returned by `pollStats()`.

use ezd_client_core::ClientSnapshot;
use ezd_proto::{Codec, StreamConfig};
use serde::Serialize;
use std::sync::Mutex;

/// Where the session is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    /// Not started.
    Idle,
    /// Opening the TCP connection.
    Connecting,
    /// Connected, exchanging Hello/caps.
    Handshaking,
    /// Stream negotiated and running.
    Streaming,
    /// Session ended normally (host bye/eof or stop).
    Ended,
    /// Session ended with an error.
    Failed,
}

#[derive(Debug, Clone)]
struct State {
    phase: Phase,
    detail: String,
    stream: Option<StreamConfig>,
    decoder: Option<String>,
}

/// State shared between the client task, the decoder thread and the JNI poll calls.
pub struct Shared(Mutex<State>);

impl Default for Shared {
    fn default() -> Self {
        Shared(Mutex::new(State {
            phase: Phase::Idle,
            detail: String::new(),
            stream: None,
            decoder: None,
        }))
    }
}

impl Shared {
    fn with<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        f(&mut self.0.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Sets phase and detail text.
    pub fn set_phase(&self, phase: Phase, detail: impl Into<String>) {
        let detail = detail.into();
        self.with(|s| {
            // A failure is sticky: later teardown must not overwrite its reason.
            if s.phase == Phase::Failed && phase == Phase::Ended {
                return;
            }
            s.phase = phase;
            s.detail = detail;
        });
    }

    /// Records the negotiated stream.
    pub fn set_stream(&self, stream: StreamConfig) {
        self.with(|s| s.stream = Some(stream));
    }

    /// Records the decoder in use.
    pub fn set_decoder(&self, name: Option<String>) {
        self.with(|s| s.decoder = name);
    }

    /// Current phase.
    pub fn phase(&self) -> Phase {
        self.with(|s| s.phase)
    }

    /// Builds the `pollStats()` JSON.
    pub fn to_json(&self, snap: &ClientSnapshot) -> String {
        let s = self.with(|s| s.clone());
        let (win_ms, fps, mbps) = match &snap.last_stats {
            Some(st) if st.window_ms > 0 => (
                st.window_ms,
                st.frames_received as f64 * 1000.0 / st.window_ms as f64,
                snap.window_video_bytes as f64 * 8.0 / (st.window_ms as f64 * 1000.0),
            ),
            _ => (0, 0.0, 0.0),
        };
        let st = snap.last_stats.unwrap_or(ezd_proto::ClientStats {
            window_ms: 0,
            frames_received: 0,
            frames_decoded: 0,
            frames_dropped: 0,
            decode_us_p50: 0,
            decode_us_p95: 0,
            e2e_us_p50: 0,
            e2e_us_p95: 0,
        });
        let codec = s.stream.as_ref().map(|c| match c.codec {
            Codec::H264 => "h264",
            Codec::Hevc => "hevc",
            Codec::Av1 => "av1",
        });
        let color = &snap.stream_color;
        let v = serde_json::json!({
            "phase": s.phase,
            "detail": s.detail,
            "width": s.stream.as_ref().map_or(0, |c| c.width),
            "height": s.stream.as_ref().map_or(0, |c| c.height),
            "refresh_mhz": s.stream.as_ref().map_or(0, |c| c.refresh_mhz),
            "bitrate_kbps": s.stream.as_ref().map_or(0, |c| c.bitrate_kbps),
            "codec": codec,
            "encoder": s.stream.as_ref().map(|c| c.encoder_name.clone()),
            "decoder": s.decoder,
            "dynamic_range": if color.is_hdr() { "hdr10" } else { "sdr" },
            "bit_depth": color.bit_depth,
            "color_primaries": color.primaries,
            "color_transfer": color.transfer,
            "window_ms": win_ms,
            "fps": fps,
            "mbps": mbps,
            "decode_us_p50": st.decode_us_p50,
            "decode_us_p95": st.decode_us_p95,
            "e2e_us_p50": st.e2e_us_p50,
            "e2e_us_p95": st.e2e_us_p95,
            "rtt_us": snap.rtt_us,
            "frames_received": snap.total_received,
            "frames_decoded": snap.total_decoded,
            "frames_dropped": snap.total_dropped,
            "input_caps": snap.input_status.as_ref().map(|i| i.caps),
            "input_note": snap.input_status.as_ref().map(|i| i.note.clone()),
            "client_flags": snap.client_options.map(|o| o.flags),
            "inputs_sent": snap.inputs_sent,
        });
        v.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_has_phase_and_stream() {
        let sh = Shared::default();
        sh.set_phase(Phase::Streaming, "ok");
        sh.set_stream(StreamConfig {
            width: 2560,
            height: 1600,
            refresh_mhz: 120_000,
            codec: Codec::H264,
            bitrate_kbps: 40_000,
            encoder_name: "x264".into(),
        });
        let j: serde_json::Value = serde_json::from_str(&sh.to_json(&Default::default())).unwrap();
        assert_eq!(j["phase"], "streaming");
        assert_eq!(j["width"], 2560);
        assert_eq!(j["codec"], "h264");
        assert_eq!(j["fps"], 0.0);
        assert!(j["input_caps"].is_null());
        assert!(j["input_note"].is_null());
        assert!(j["client_flags"].is_null());
        assert_eq!(j["inputs_sent"], 0);
        assert_eq!(j["dynamic_range"], "sdr");
        assert_eq!(j["bit_depth"], 8);
    }

    #[test]
    fn json_reports_hdr10() {
        let sh = Shared::default();
        let snap = ClientSnapshot {
            stream_color: ezd_proto::StreamColor::hdr10(None, 0, 0),
            ..Default::default()
        };
        let j: serde_json::Value = serde_json::from_str(&sh.to_json(&snap)).unwrap();
        assert_eq!(j["dynamic_range"], "hdr10");
        assert_eq!(j["bit_depth"], 10);
        assert_eq!(j["color_transfer"], 16);
    }

    #[test]
    fn json_has_input_status() {
        let sh = Shared::default();
        let snap = ClientSnapshot {
            input_status: Some(ezd_proto::InputStatus {
                caps: 3,
                note: "no uinput".into(),
            }),
            client_options: Some(ezd_proto::ClientOptions { flags: 1 }),
            inputs_sent: 9,
            ..Default::default()
        };
        let j: serde_json::Value = serde_json::from_str(&sh.to_json(&snap)).unwrap();
        assert_eq!(j["input_caps"], 3);
        assert_eq!(j["input_note"], "no uinput");
        assert_eq!(j["client_flags"], 1);
        assert_eq!(j["inputs_sent"], 9);
    }

    #[test]
    fn failure_is_sticky() {
        let sh = Shared::default();
        sh.set_phase(Phase::Failed, "boom");
        sh.set_phase(Phase::Ended, "stopped");
        assert_eq!(sh.phase(), Phase::Failed);
    }
}
