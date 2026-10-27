//! Configuration of the service: what the CLI flags and the tray settings both describe.

use ezd_host_core::{
    AudioBackend, InputBackend, PenMode, PlatformOptions, SessionEvent, SessionOptions,
    UnavailableBackend,
};
use ezd_link::VideoCredit;
use ezd_proto::{AudioMode, HdrMode, LinkKind, Prefs};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc::UnboundedSender, watch};

/// How long a sent video frame may wait for the client's ack before its credit is released.
const ACK_TIMEOUT: Duration = Duration::from_millis(500);

/// Where the video comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    /// A real virtual display captured and encoded by the platform backend.
    #[default]
    Virtual,
    /// A software-generated test pattern.
    Synthetic,
}

/// What the service does with adb.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AdbMode {
    /// Watch the USB devices and set up `adb reverse` for each ready one. Devices on a network
    /// transport (wireless debugging, `host:port` serials) are ignored on purpose: they are
    /// usually somebody's phone. Wireless users pass an explicit serial.
    #[default]
    Auto,
    /// Handle only this serial, whatever its transport.
    Serial(String),
    /// Leave adb alone (the tablet reaches the host over the network).
    Off,
}

/// What the service does about USB accessory mode (USB without developer mode).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AoaMode {
    /// Switch a connected tablet that shows up as a file-transfer or debugging device to
    /// accessory mode and stream over USB. adb keeps working as the fallback.
    #[default]
    Auto,
    /// Leave the USB devices alone.
    Off,
}

/// Transport kind carrying the stream; selects the default bitrate ceiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkChoice {
    /// USB cable through adb reverse.
    #[default]
    Usb,
    /// Wi-Fi / LAN.
    Network,
}

/// How the pen is injected on the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PenChoice {
    /// A graphics tablet with pressure and tilt.
    #[default]
    Tablet,
    /// An absolute mouse pointer.
    Mouse,
}

/// HDR10 streaming policy (serializable mirror of [`HdrMode`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HdrChoice {
    /// Stream HDR10 whenever the source, the tablet and the encoder all support it.
    #[default]
    Auto,
    /// Never stream HDR10; an HDR source is converted to SDR.
    Off,
}

impl From<HdrChoice> for HdrMode {
    fn from(c: HdrChoice) -> Self {
        match c {
            HdrChoice::Auto => HdrMode::Auto,
            HdrChoice::Off => HdrMode::Off,
        }
    }
}

/// PC audio streaming policy (serializable mirror of [`AudioMode`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioChoice {
    /// Uncompressed PCM when the link and the tablet allow it, else Opus.
    #[default]
    Auto,
    /// Prefer PCM even on a slow link.
    Pcm,
    /// Prefer Opus.
    Opus,
    /// No audio.
    Off,
}

impl From<AudioChoice> for AudioMode {
    fn from(c: AudioChoice) -> Self {
        match c {
            AudioChoice::Auto => AudioMode::Auto,
            AudioChoice::Pcm => AudioMode::Pcm,
            AudioChoice::Opus => AudioMode::Opus,
            AudioChoice::Off => AudioMode::Off,
        }
    }
}

/// Congestion controller of the Wi-Fi link.
#[cfg(feature = "wifi")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CongestionChoice {
    /// CUBIC.
    #[default]
    Cubic,
    /// BBR (experimental).
    Bbr,
}

#[cfg(feature = "wifi")]
impl From<CongestionChoice> for ezd_link::quic::Congestion {
    fn from(c: CongestionChoice) -> Self {
        match c {
            CongestionChoice::Cubic => Self::Cubic,
            CongestionChoice::Bbr => Self::Bbr,
        }
    }
}

/// The Wi-Fi (QUIC) listener. It is read when the service starts; changing it later needs a
/// restart of the service.
#[cfg(feature = "wifi")]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WifiConfig {
    /// UDP address the tablets connect to.
    pub listen: SocketAddr,
    /// Open the pairing window at start even if a device is already paired.
    pub pair_on_start: bool,
    /// Congestion controller.
    pub congestion: CongestionChoice,
    /// Where the identity and the paired devices live; `None` is the `ezdisplay` folder in the
    /// user's config directory.
    pub config_dir: Option<std::path::PathBuf>,
}

#[cfg(feature = "wifi")]
impl Default for WifiConfig {
    fn default() -> Self {
        WifiConfig {
            listen: SocketAddr::from(([0, 0, 0, 0], ezd_proto::DEFAULT_PORT)),
            pair_on_start: false,
            congestion: CongestionChoice::default(),
            config_dir: None,
        }
    }
}

/// Everything the service needs to run; loads from a file with missing or unknown keys.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServiceConfig {
    /// Address the tablet connects to (through `adb reverse` or the network).
    pub listen: SocketAddr,
    /// Video source.
    pub source: SourceKind,
    /// adb handling.
    pub adb: AdbMode,
    /// USB accessory mode (USB without developer mode). `Auto` tries the accessory first, and
    /// adb keeps working as the fallback. The field exists without the `aoa` feature too, so
    /// config files stay portable between builds.
    pub aoa: AoaMode,
    /// Transport kind.
    pub link: LinkChoice,
    /// Highest refresh rate to use, in millihertz.
    pub max_refresh_mhz: u32,
    /// Fixed bitrate in kbit/s (clamped to the link ceiling); `None` derives it.
    pub bitrate_kbps: Option<u32>,
    /// Force an encoder by name.
    pub encoder: Option<String>,
    /// Stream size as a fraction of the tablet's native size, in thousandths (250-1000).
    pub scale_permille: u16,
    /// GPU/adapter to use (virtual source).
    pub adapter: Option<String>,
    /// Prefer the discrete GPU (virtual source).
    pub prefer_dgpu: bool,
    /// Capture this existing output instead of creating one (virtual source).
    pub output_name: Option<String>,
    /// Video frames that may be sent but not yet acknowledged (1-8).
    pub video_window: u32,
    /// Ask the tablet to show its telemetry overlay.
    pub telemetry: bool,
    /// Inject input from the tablet.
    pub input: bool,
    /// How the pen is injected.
    pub pen_mode: PenChoice,
    /// Start the Android app when a device becomes ready.
    pub open_app_on_connect: bool,
    /// HDR10 streaming policy (`hdr = "auto" | "off"`).
    pub hdr: HdrChoice,
    /// PC audio policy (`audio = "auto" | "pcm" | "opus" | "off"`).
    pub audio: AudioChoice,
    /// Wi-Fi listener; `None` leaves Wi-Fi off. Sessions on it always use the network link kind,
    /// whatever `link` says.
    #[cfg(feature = "wifi")]
    pub wifi: Option<WifiConfig>,
    /// Milliseconds the virtual monitor stays after the tablet sleeps, turns off or disconnects
    /// (Windows; on Linux it goes with the session).
    pub detach_grace_ms: u32,
}

impl Default for ServiceConfig {
    fn default() -> Self {
        ServiceConfig {
            listen: SocketAddr::from(([127, 0, 0, 1], ezd_proto::DEFAULT_PORT)),
            source: SourceKind::default(),
            adb: AdbMode::default(),
            aoa: AoaMode::default(),
            link: LinkChoice::default(),
            max_refresh_mhz: 120_000,
            bitrate_kbps: None,
            encoder: None,
            scale_permille: 1000,
            adapter: None,
            prefer_dgpu: false,
            output_name: None,
            video_window: 1,
            telemetry: false,
            input: true,
            pen_mode: PenChoice::default(),
            open_app_on_connect: true,
            hdr: HdrChoice::default(),
            audio: AudioChoice::default(),
            #[cfg(feature = "wifi")]
            wifi: None,
            detach_grace_ms: ezd_host_core::DEFAULT_DETACH_GRACE.as_millis() as u32,
        }
    }
}

impl ServiceConfig {
    /// Stream preferences negotiated with each client.
    pub fn prefs(&self) -> Prefs {
        Prefs {
            max_refresh_mhz: self.max_refresh_mhz,
            link: match self.link {
                LinkChoice::Usb => LinkKind::Usb,
                LinkChoice::Network => LinkKind::Network,
            },
            bitrate_kbps: self.bitrate_kbps,
            encoder: self.encoder.clone(),
            scale_permille: self.scale_permille,
            hdr: self.hdr.into(),
            audio: self.audio.into(),
            ..Prefs::default()
        }
    }

    /// Preferences of a Wi-Fi session: the network link kind, the rest as [`prefs`](Self::prefs).
    pub fn wifi_prefs(&self) -> Prefs {
        Prefs {
            link: LinkKind::Network,
            ..self.prefs()
        }
    }

    /// Options of the virtual-display backend.
    pub fn platform_options(&self) -> PlatformOptions {
        PlatformOptions {
            adapter: self.adapter.clone(),
            prefer_dgpu: self.prefer_dgpu,
            output_name: self.output_name.clone(),
            detach_grace: Duration::from_millis(self.detach_grace_ms.into()),
        }
    }

    /// Options of one session: `shutdown` ends it, `events` receives its live progress.
    pub fn session_options(
        &self,
        shutdown: watch::Receiver<bool>,
        events: Option<UnboundedSender<SessionEvent>>,
    ) -> SessionOptions {
        let input: Option<Arc<dyn InputBackend>> = self.input.then(|| {
            crate::platform::input_backend().unwrap_or_else(|| {
                Arc::new(UnavailableBackend(
                    "input injection is not implemented on this platform yet".into(),
                ))
            })
        });
        // With audio off no source is ever created, whatever the client asks for.
        let audio: Option<Arc<dyn AudioBackend>> = (self.audio != AudioChoice::Off)
            .then(|| Arc::new(crate::audio::HostAudioBackend) as Arc<dyn AudioBackend>);
        SessionOptions {
            video_credit: VideoCredit::OnAck {
                window: self.video_window,
                timeout: ACK_TIMEOUT,
            },
            shutdown: Some(shutdown),
            input,
            pen_mode: match self.pen_mode {
                PenChoice::Tablet => PenMode::Tablet,
                PenChoice::Mouse => PenMode::Mouse,
            },
            telemetry: self.telemetry,
            audio,
            events,
            ..SessionOptions::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_cli() {
        let c = ServiceConfig::default();
        assert_eq!(c.listen.to_string(), "127.0.0.1:27183");
        // `ezd-host run` with only `--source`: same prefs as `Prefs::default()` except the link
        // kind that the CLI always sets (usb), which is also the default.
        assert_eq!(c.prefs(), Prefs::default());
        assert_eq!(c.platform_options(), PlatformOptions::default());
    }

    #[test]
    fn prefs_follow_the_fields() {
        let c = ServiceConfig {
            max_refresh_mhz: 60_000,
            link: LinkChoice::Network,
            bitrate_kbps: Some(20_000),
            encoder: Some("h264_nvenc".into()),
            scale_permille: 750,
            hdr: HdrChoice::Off,
            ..ServiceConfig::default()
        };
        assert_eq!(
            c.prefs(),
            Prefs {
                max_refresh_mhz: 60_000,
                link: LinkKind::Network,
                bitrate_kbps: Some(20_000),
                encoder: Some("h264_nvenc".into()),
                scale_permille: 750,
                hdr: HdrMode::Off,
                ..Prefs::default()
            }
        );
    }

    #[test]
    fn wifi_sessions_use_the_network_link_kind() {
        let c = ServiceConfig::default();
        assert_eq!(c.prefs().link, LinkKind::Usb);
        assert_eq!(
            c.wifi_prefs(),
            Prefs {
                link: LinkKind::Network,
                ..c.prefs()
            }
        );
    }

    #[test]
    fn platform_options_follow_the_fields() {
        let c = ServiceConfig {
            adapter: Some("card1".into()),
            prefer_dgpu: true,
            output_name: Some("DP-1".into()),
            detach_grace_ms: 5000,
            ..ServiceConfig::default()
        };
        assert_eq!(
            c.platform_options(),
            PlatformOptions {
                adapter: Some("card1".into()),
                prefer_dgpu: true,
                output_name: Some("DP-1".into()),
                detach_grace: Duration::from_secs(5),
            }
        );
    }

    #[test]
    fn session_options_follow_the_fields() {
        let (_tx, rx) = watch::channel(false);
        let o = ServiceConfig::default().session_options(rx.clone(), None);
        assert_eq!(
            o.video_credit,
            VideoCredit::OnAck {
                window: 1,
                timeout: Duration::from_millis(500)
            }
        );
        assert!(o.shutdown.is_some() && o.input.is_some() && o.events.is_none());
        assert_eq!((o.pen_mode, o.telemetry), (PenMode::Tablet, false));

        let c = ServiceConfig {
            video_window: 4,
            telemetry: true,
            input: false,
            pen_mode: PenChoice::Mouse,
            ..ServiceConfig::default()
        };
        let (ev, _ev_rx) = tokio::sync::mpsc::unbounded_channel();
        let o = c.session_options(rx, Some(ev));
        assert_eq!(
            o.video_credit,
            VideoCredit::OnAck {
                window: 4,
                timeout: Duration::from_millis(500)
            }
        );
        assert!(o.input.is_none() && o.events.is_some());
        assert_eq!((o.pen_mode, o.telemetry), (PenMode::Mouse, true));
    }

    #[test]
    fn loads_with_missing_and_unknown_keys() {
        let c: ServiceConfig = serde_json::from_str(
            r#"{ "listen": "0.0.0.0:4000", "adb": { "serial": "ABC" }, "future_key": 1 }"#,
        )
        .unwrap();
        assert_eq!(c.listen.port(), 4000);
        assert_eq!(c.adb, AdbMode::Serial("ABC".into()));
        assert_eq!(
            ServiceConfig {
                listen: c.listen,
                adb: c.adb.clone(),
                ..ServiceConfig::default()
            },
            c
        );
        let empty: ServiceConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, ServiceConfig::default());
    }

    #[test]
    fn hdr_defaults_to_auto_and_reads_its_serde_name() {
        let empty: ServiceConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(empty.hdr, HdrChoice::Auto);
        assert_eq!(empty.prefs().hdr, HdrMode::Auto);
        let off: ServiceConfig = serde_json::from_str(r#"{ "hdr": "off" }"#).unwrap();
        assert_eq!(off.hdr, HdrChoice::Off);
        assert_eq!(off.prefs().hdr, HdrMode::Off);
    }

    #[test]
    fn audio_defaults_to_auto_and_reads_its_serde_name() {
        assert_eq!(ServiceConfig::default().audio, AudioChoice::Auto);
        let empty: ServiceConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(empty.audio, AudioChoice::Auto);
        assert_eq!(empty.prefs().audio, AudioMode::Auto);
        for (name, choice, mode) in [
            ("auto", AudioChoice::Auto, AudioMode::Auto),
            ("pcm", AudioChoice::Pcm, AudioMode::Pcm),
            ("opus", AudioChoice::Opus, AudioMode::Opus),
            ("off", AudioChoice::Off, AudioMode::Off),
        ] {
            let c: ServiceConfig =
                serde_json::from_str(&format!(r#"{{ "audio": "{name}" }}"#)).unwrap();
            assert_eq!(c.audio, choice);
            assert_eq!(c.prefs().audio, mode);
            assert_eq!(c.wifi_prefs().audio, mode);
            let back: ServiceConfig =
                serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
            assert_eq!(back, c);
        }
        assert!(serde_json::from_str::<ServiceConfig>(r#"{ "audio": "loud" }"#).is_err());
    }

    #[test]
    fn audio_off_creates_no_audio_backend() {
        let (_tx, rx) = watch::channel(false);
        assert!(
            ServiceConfig::default()
                .session_options(rx.clone(), None)
                .audio
                .is_some()
        );
        let off = ServiceConfig {
            audio: AudioChoice::Off,
            ..ServiceConfig::default()
        };
        assert!(off.session_options(rx, None).audio.is_none());
    }

    #[test]
    fn aoa_defaults_to_auto_and_reads_its_serde_name() {
        assert_eq!(ServiceConfig::default().aoa, AoaMode::Auto);
        let empty: ServiceConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(empty.aoa, AoaMode::Auto);
        let off: ServiceConfig = serde_json::from_str(r#"{ "aoa": "off" }"#).unwrap();
        assert_eq!(off.aoa, AoaMode::Off);
        let auto: ServiceConfig = serde_json::from_str(r#"{ "aoa": "auto" }"#).unwrap();
        assert_eq!(auto.aoa, AoaMode::Auto);
        assert!(serde_json::from_str::<ServiceConfig>(r#"{ "aoa": "sometimes" }"#).is_err());
    }

    #[test]
    fn aoa_round_trips_and_an_old_config_keeps_the_default() {
        let c = ServiceConfig {
            aoa: AoaMode::Off,
            ..ServiceConfig::default()
        };
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains(r#""aoa":"off""#), "{json}");
        let back: ServiceConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back, c);
        // A file written before the key existed.
        let old: ServiceConfig =
            serde_json::from_str(r#"{ "listen": "127.0.0.1:4000", "adb": "off" }"#).unwrap();
        assert_eq!(old.aoa, AoaMode::Auto);
    }

    #[test]
    fn round_trips() {
        let c = ServiceConfig {
            adb: AdbMode::Off,
            source: SourceKind::Synthetic,
            pen_mode: PenChoice::Mouse,
            link: LinkChoice::Network,
            ..ServiceConfig::default()
        };
        let back: ServiceConfig =
            serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back, c);
    }
}
