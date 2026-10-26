//! Sway: a headless virtual output through the i3 IPC socket (`$SWAYSOCK`), captured directly over
//! Wayland (`wl_capture.rs`), and the input mapping of the touch, pen and pointer devices.
//!
//! The IPC framing is `"i3-ipc"`, payload length (u32, native endian), message type (u32, native
//! endian), payload. A virtual output is `create_output` (sway adds a `HEADLESS-n` output, also on
//! a DRM session), then `output HEADLESS-n mode --custom WxH@RHz`. Sway places the new output to
//! the right of the existing ones by itself: an explicit `position` would mark the output as
//! configured, and sway then moves the user's own unconfigured outputs behind it. Dropping the
//! guard unplugs the output.

use crate::kscreen::KOutput;
use crate::portal::SourceKind;
use crate::provider::{FrameSource, OpenedOutput, OutputRequest, VirtualOutputProvider};
use crate::uinput_sink::{
    DesktopInput, MappedDevices, PEN_NAME, PEN_PRODUCT, POINTER_NAME, POINTER_PRODUCT, TOUCH_NAME,
    TOUCH_PRODUCT, VENDOR,
};
use ezd_host_core::{DisplayMode, OutputInfo, SourceError};
use serde_json::Value;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const MAGIC: &[u8; 6] = b"i3-ipc";
/// `RUN_COMMAND`.
pub const RUN_COMMAND: u32 = 0;
/// `GET_OUTPUTS`.
pub const GET_OUTPUTS: u32 = 3;
/// `GET_VERSION`.
pub const GET_VERSION: u32 = 7;
/// Replies larger than this are a protocol error.
const MAX_PAYLOAD: usize = 16 << 20;
const IO_TIMEOUT: Duration = Duration::from_secs(5);
const OUTPUT_TIMEOUT: Duration = Duration::from_secs(3);

/// One IPC message: magic, length, type, payload.
pub fn encode_message(ty: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(14 + payload.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(payload.len() as u32).to_ne_bytes());
    out.extend_from_slice(&ty.to_ne_bytes());
    out.extend_from_slice(payload);
    out
}

/// Reads one message: `(type, payload)`.
pub fn read_message(r: &mut impl Read) -> io::Result<(u32, Vec<u8>)> {
    let mut head = [0u8; 14];
    r.read_exact(&mut head)?;
    if &head[..6] != MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "not an i3-ipc message",
        ));
    }
    let len = u32::from_ne_bytes([head[6], head[7], head[8], head[9]]) as usize;
    let ty = u32::from_ne_bytes([head[10], head[11], head[12], head[13]]);
    if len > MAX_PAYLOAD {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("i3-ipc payload of {len} bytes"),
        ));
    }
    let mut payload = vec![0u8; len];
    r.read_exact(&mut payload)?;
    Ok((ty, payload))
}

/// Result of a `RUN_COMMAND`: the first failure's message, if any.
pub fn parse_command_reply(payload: &[u8]) -> Result<(), String> {
    let v: Value = serde_json::from_slice(payload).map_err(|e| format!("bad reply: {e}"))?;
    let items = v.as_array().ok_or("bad reply: not an array")?;
    for item in items {
        if item.get("success").and_then(Value::as_bool) != Some(true) {
            let msg = item
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("the command failed");
            return Err(msg.to_string());
        }
    }
    Ok(())
}

/// An output as `GET_OUTPUTS` reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct SwayOutput {
    pub name: String,
    pub active: bool,
    pub scale: f64,
    /// Position and size in the layout (logical pixels).
    pub rect: (i32, i32, u32, u32),
    /// Current mode: width, height (pixels), refresh in millihertz (0 when the output has none).
    pub mode: Option<(u32, u32, u32)>,
}

/// Parses a `GET_OUTPUTS` reply.
pub fn parse_outputs(payload: &[u8]) -> Result<Vec<SwayOutput>, String> {
    let v: Value = serde_json::from_slice(payload).map_err(|e| format!("bad reply: {e}"))?;
    let items = v.as_array().ok_or("bad reply: not an array")?;
    let int = |v: &Value, k: &str| v.get(k).and_then(Value::as_i64).unwrap_or(0);
    Ok(items
        .iter()
        .filter_map(|o| {
            let name = o.get("name")?.as_str()?.to_string();
            let rect = o.get("rect");
            let r = |k: &str| rect.map_or(0, |r| int(r, k));
            let mode = o.get("current_mode").filter(|m| m.is_object()).map(|m| {
                (
                    int(m, "width") as u32,
                    int(m, "height") as u32,
                    int(m, "refresh") as u32,
                )
            });
            Some(SwayOutput {
                name,
                active: o.get("active").and_then(Value::as_bool).unwrap_or(false),
                scale: o.get("scale").and_then(Value::as_f64).unwrap_or(1.0),
                rect: (
                    r("x") as i32,
                    r("y") as i32,
                    r("width") as u32,
                    r("height") as u32,
                ),
                mode,
            })
        })
        .collect())
}

/// The `human_readable` version of a `GET_VERSION` reply.
pub fn parse_version(payload: &[u8]) -> Option<String> {
    let v: Value = serde_json::from_slice(payload).ok()?;
    v.get("human_readable")?.as_str().map(str::to_string)
}

/// The output that is in `after` but not in `before` (the one `create_output` added).
pub fn new_output(before: &[SwayOutput], after: &[SwayOutput]) -> Option<String> {
    after
        .iter()
        .find(|a| !before.iter().any(|b| b.name == a.name))
        .map(|a| a.name.clone())
}

/// Output names with spaces need quotes in sway commands.
fn quote_name(name: &str) -> String {
    if name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_.:*".contains(c))
    {
        name.to_string()
    } else {
        format!("\"{}\"", name.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

/// The command that sets a custom mode.
pub fn mode_command(name: &str, width: u32, height: u32, refresh_mhz: u32) -> String {
    format!(
        "output {} mode --custom {width}x{height}@{}Hz",
        quote_name(name),
        crate::provider::rate_text(refresh_mhz.max(1000))
    )
}

/// A connection to sway's IPC socket.
pub struct SwayIpc {
    stream: UnixStream,
}

/// The socket path from `$SWAYSOCK`.
pub fn socket_path() -> Result<PathBuf, String> {
    std::env::var_os("SWAYSOCK")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| "SWAYSOCK is not set (is sway running in this session?)".into())
}

impl SwayIpc {
    /// Connects to `$SWAYSOCK`.
    pub fn connect() -> Result<SwayIpc, String> {
        SwayIpc::connect_to(&socket_path()?)
    }

    /// Connects to the socket at `path`.
    pub fn connect_to(path: &Path) -> Result<SwayIpc, String> {
        let stream = UnixStream::connect(path)
            .map_err(|e| format!("cannot connect to {}: {e}", path.display()))?;
        let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
        let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
        Ok(SwayIpc { stream })
    }

    /// Sends one request and returns the payload of its reply.
    pub fn request(&mut self, ty: u32, payload: &[u8]) -> Result<Vec<u8>, String> {
        self.stream
            .write_all(&encode_message(ty, payload))
            .map_err(|e| format!("sway IPC write: {e}"))?;
        loop {
            let (t, p) =
                read_message(&mut self.stream).map_err(|e| format!("sway IPC read: {e}"))?;
            // Events (high bit set) are only sent to subscribers; skip any that show up.
            if t == ty {
                return Ok(p);
            }
            if t & 0x8000_0000 == 0 {
                return Err(format!(
                    "sway IPC: reply type {t} to a request of type {ty}"
                ));
            }
        }
    }

    /// Runs a sway command.
    pub fn command(&mut self, cmd: &str) -> Result<(), String> {
        let reply = self.request(RUN_COMMAND, cmd.as_bytes())?;
        parse_command_reply(&reply).map_err(|e| format!("sway `{cmd}`: {e}"))
    }

    /// The outputs sway knows.
    pub fn outputs(&mut self) -> Result<Vec<SwayOutput>, String> {
        parse_outputs(&self.request(GET_OUTPUTS, b"")?)
    }

    /// Sway's version string.
    pub fn version(&mut self) -> Result<String, String> {
        parse_version(&self.request(GET_VERSION, b"")?).ok_or_else(|| "no version in reply".into())
    }
}

/// The output description of `name`.
pub fn output_info(outputs: &[SwayOutput], name: &str, fallback_mhz: u32) -> Option<OutputInfo> {
    let o = outputs.iter().find(|o| o.name == name)?;
    let (w, h, hz) = o.mode.unwrap_or((o.rect.2, o.rect.3, 0));
    Some(OutputInfo {
        name: o.name.clone(),
        mode: DisplayMode {
            width: w,
            height: h,
            refresh_mhz: if hz == 0 { fallback_mhz } else { hz },
        },
        origin: (o.rect.0, o.rect.1),
        adapter: None,
    })
}

/// Unplugs a virtual output when dropped.
struct OutputGuard {
    socket: PathBuf,
    name: String,
}

impl Drop for OutputGuard {
    fn drop(&mut self) {
        let r = SwayIpc::connect_to(&self.socket)
            .and_then(|mut ipc| ipc.command(&format!("output {} unplug", quote_name(&self.name))));
        match r {
            Ok(()) => tracing::info!(output = %self.name, "virtual output unplugged"),
            Err(e) => tracing::warn!("cannot unplug {}: {e}", self.name),
        }
    }
}

/// Provider for sway.
#[derive(Default)]
pub struct SwayProvider;

impl VirtualOutputProvider for SwayProvider {
    fn name(&self) -> &'static str {
        "sway"
    }

    fn open(&mut self, req: &OutputRequest) -> Result<OpenedOutput, SourceError> {
        let socket = socket_path().map_err(SourceError::msg)?;
        let mut ipc = SwayIpc::connect_to(&socket).map_err(SourceError::msg)?;
        if req.kind == SourceKind::Monitor {
            let name = req
                .output_name
                .clone()
                .ok_or_else(|| SourceError::msg("no output name to capture"))?;
            let outputs = ipc.outputs().map_err(SourceError::msg)?;
            let info = output_info(&outputs, &name, req.refresh_mhz).ok_or_else(|| {
                SourceError::msg(format!(
                    "sway has no output called {name} (outputs: {})",
                    outputs
                        .iter()
                        .map(|o| o.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            })?;
            return Ok(OpenedOutput {
                frames: FrameSource::Wayland { output: name },
                info: Some(info),
                guard: Box::new(()),
            });
        }

        let before = ipc.outputs().map_err(SourceError::msg)?;
        ipc.command("create_output").map_err(SourceError::msg)?;
        let deadline = Instant::now() + OUTPUT_TIMEOUT;
        let name = loop {
            let after = ipc.outputs().map_err(SourceError::msg)?;
            if let Some(n) = new_output(&before, &after) {
                break n;
            }
            if Instant::now() >= deadline {
                return Err(SourceError::msg("sway did not add a virtual output"));
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        tracing::info!(output = %name, "sway virtual output created");
        // From here on the guard removes the output on every early return.
        let guard = OutputGuard {
            socket,
            name: name.clone(),
        };
        ipc.command(&mode_command(&name, req.width, req.height, req.refresh_mhz))
            .map_err(SourceError::msg)?;
        let mut info = None;
        let deadline = Instant::now() + OUTPUT_TIMEOUT;
        loop {
            let outputs = ipc.outputs().map_err(SourceError::msg)?;
            info = output_info(&outputs, &name, req.refresh_mhz).or(info);
            if info
                .as_ref()
                .is_some_and(|i| (i.mode.width, i.mode.height) == (req.width, req.height))
            {
                break;
            }
            if Instant::now() >= deadline {
                tracing::warn!(output = %name, ?info, "the mode was not applied in time");
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Ok(OpenedOutput {
            frames: FrameSource::Wayland { output: name },
            info,
            guard: Box::new(guard),
        })
    }
}

/// Sway's identifier of an input device: `<vendor>:<product>:<name>` in decimal, with spaces (and
/// unprintable characters) in the name replaced by underscores.
pub fn input_identifier(vendor: u16, product: u16, name: &str) -> String {
    let name: String = name
        .trim()
        .chars()
        .map(|c| if c == ' ' || c.is_control() { '_' } else { c })
        .collect();
    format!("{vendor}:{product}:{name}")
}

/// The identifier of the device behind a label (`touch`, `pen`, `pointer`).
pub fn device_identifier(label: &str) -> Option<String> {
    let (product, name) = match label {
        "touch" => (TOUCH_PRODUCT, TOUCH_NAME),
        "pen" => (PEN_PRODUCT, PEN_NAME),
        "pointer" => (POINTER_PRODUCT, POINTER_NAME),
        _ => return None,
    };
    Some(input_identifier(VENDOR, product, name))
}

/// `input <identifier> map_to_output <output>`.
pub fn map_command(identifier: &str, output: &str) -> String {
    format!(
        "input {} map_to_output {}",
        quote_name(identifier),
        quote_name(output)
    )
}

/// Undoes the mappings when dropped: `map_to_output *` names no output, which sway treats as no
/// mapping. The devices are gone by then, but their configuration stays in sway.
struct Restore {
    identifiers: Vec<String>,
}

impl Drop for Restore {
    fn drop(&mut self) {
        let Ok(mut ipc) = SwayIpc::connect() else {
            return;
        };
        for id in &self.identifiers {
            if let Err(e) = ipc.command(&map_command(id, "*")) {
                tracing::debug!("cannot clear the output mapping of {id}: {e}");
            }
        }
    }
}

/// Input mapping for sway: every device is tied to the virtual output, so the absolute pointer
/// needs no workspace mapping.
#[derive(Default)]
pub struct SwayDesktop;

impl DesktopInput for SwayDesktop {
    fn map_devices(&self, devices: &[(&str, &str)], output: &str) -> MappedDevices {
        let mut ipc = match SwayIpc::connect() {
            Ok(i) => i,
            Err(e) => {
                return MappedDevices {
                    results: devices.iter().map(|_| Err(e.clone())).collect(),
                    guard: None,
                };
            }
        };
        let mut restore = Restore {
            identifiers: Vec::new(),
        };
        let results = devices
            .iter()
            .map(|(label, _)| {
                let id =
                    device_identifier(label).ok_or_else(|| format!("unknown device {label}"))?;
                ipc.command(&map_command(&id, output))?;
                tracing::info!(device = *label, %id, %output, "sway output mapping set");
                restore.identifiers.push(id);
                Ok(())
            })
            .collect();
        MappedDevices {
            results,
            guard: (!restore.identifiers.is_empty()).then(|| Box::new(restore) as Box<dyn Send>),
        }
    }

    fn outputs(&self) -> Option<Vec<KOutput>> {
        None
    }

    fn maps_pointer(&self) -> bool {
        true
    }
}

/// Sway's part of `ezd-host diag`.
pub fn diag() -> Value {
    let probe = || -> Result<(String, Vec<SwayOutput>), String> {
        let mut ipc = SwayIpc::connect()?;
        Ok((ipc.version()?, ipc.outputs()?))
    };
    match probe() {
        Ok((version, outputs)) => serde_json::json!({
            "sway_version": version,
            "outputs": outputs.iter().map(|o| serde_json::json!({
                "name": o.name,
                "active": o.active,
                "size": o.mode.map(|m| [m.0, m.1]),
                "position": [o.rect.0, o.rect.1],
                "scale": o.scale,
            })).collect::<Vec<_>>(),
        }),
        Err(e) => serde_json::json!({ "error": e }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    const OUTPUTS: &str = include_str!("../testdata/sway-get-outputs.json");
    const VERSION: &str = include_str!("../testdata/sway-get-version.json");

    #[test]
    fn framing_is_magic_length_type_payload() {
        // Captured from a real sway: a GET_VERSION reply header (136 bytes, type 7).
        let real = [
            0x69, 0x33, 0x2d, 0x69, 0x70, 0x63, 0x88, 0x00, 0x00, 0x00, 0x07, 0x00, 0x00, 0x00,
        ];
        let mut msg = real.to_vec();
        msg.extend(std::iter::repeat_n(b' ', 0x88));
        let (ty, payload) = read_message(&mut msg.as_slice()).unwrap();
        assert_eq!((ty, payload.len()), (GET_VERSION, 0x88));
        let enc = encode_message(RUN_COMMAND, b"create_output");
        assert_eq!(&enc[..6], b"i3-ipc");
        assert_eq!(u32::from_ne_bytes(enc[6..10].try_into().unwrap()), 13);
        assert_eq!(u32::from_ne_bytes(enc[10..14].try_into().unwrap()), 0);
        assert_eq!(&enc[14..], b"create_output");
        let (ty, payload) = read_message(&mut enc.as_slice()).unwrap();
        assert_eq!((ty, payload), (RUN_COMMAND, b"create_output".to_vec()));
    }

    #[test]
    fn rejects_bad_magic_truncation_and_huge_payloads() {
        let mut bad = encode_message(0, b"x");
        bad[0] = b'j';
        assert!(read_message(&mut bad.as_slice()).is_err());
        let mut short = encode_message(3, b"hello");
        short.truncate(short.len() - 1);
        assert!(read_message(&mut short.as_slice()).is_err());
        let mut huge = encode_message(3, b"");
        huge[6..10].copy_from_slice(&u32::MAX.to_ne_bytes());
        assert!(read_message(&mut huge.as_slice()).is_err());
    }

    #[test]
    fn command_replies() {
        assert_eq!(parse_command_reply(br#"[{"success":true}]"#), Ok(()));
        // Real error reply of `output NOPE-9 unplug`.
        let err = parse_command_reply(
            br#"[{"success":false,"parse_error":true,"error":"Cannot unplug unknown output NOPE-9"}]"#,
        )
        .unwrap_err();
        assert!(err.contains("unknown output NOPE-9"));
        assert!(parse_command_reply(b"{}").is_err());
        assert!(parse_command_reply(b"nope").is_err());
    }

    #[test]
    fn parses_real_get_outputs() {
        let outs = parse_outputs(OUTPUTS.as_bytes()).unwrap();
        assert_eq!(outs.len(), 2);
        assert_eq!(outs[0].name, "HEADLESS-1");
        assert_eq!(outs[0].mode, Some((1280, 720, 0)));
        let o = &outs[1];
        assert_eq!(o.name, "HEADLESS-2");
        assert!(o.active);
        assert_eq!(o.rect, (1280, 0, 1280, 720));
        assert_eq!(o.mode, Some((1280, 720, 60000)));
        assert_eq!(o.scale, 1.0);
    }

    #[test]
    fn diffing_finds_the_created_output() {
        let outs = parse_outputs(OUTPUTS.as_bytes()).unwrap();
        assert_eq!(new_output(&outs[..1], &outs), Some("HEADLESS-2".into()));
        assert_eq!(new_output(&outs, &outs), None);
        assert_eq!(new_output(&[], &outs[..1]), Some("HEADLESS-1".into()));
    }

    #[test]
    fn output_info_reads_geometry_and_falls_back_for_a_missing_refresh() {
        let outs = parse_outputs(OUTPUTS.as_bytes()).unwrap();
        let i = output_info(&outs, "HEADLESS-2", 90_000).unwrap();
        assert_eq!(i.origin, (1280, 0));
        assert_eq!(
            (i.mode.width, i.mode.height, i.mode.refresh_mhz),
            (1280, 720, 60_000)
        );
        // HEADLESS-1 reports refresh 0.
        assert_eq!(
            output_info(&outs, "HEADLESS-1", 90_000)
                .unwrap()
                .mode
                .refresh_mhz,
            90_000
        );
        assert!(output_info(&outs, "DP-9", 1).is_none());
    }

    #[test]
    fn version_text() {
        assert_eq!(parse_version(VERSION.as_bytes()).as_deref(), Some("1.12"));
        assert_eq!(parse_version(b"[]"), None);
    }

    #[test]
    fn mode_commands() {
        assert_eq!(
            mode_command("HEADLESS-2", 2560, 1600, 60_000),
            "output HEADLESS-2 mode --custom 2560x1600@60Hz"
        );
        assert_eq!(
            mode_command("HEADLESS-2", 1920, 1080, 59_940),
            "output HEADLESS-2 mode --custom 1920x1080@59.94Hz"
        );
        assert_eq!(
            mode_command("My Monitor", 800, 600, 120_000),
            "output \"My Monitor\" mode --custom 800x600@120Hz"
        );
    }

    #[test]
    fn input_identifiers_are_decimal_with_underscores() {
        assert_eq!(
            device_identifier("touch").unwrap(),
            "4617:60881:ezdisplay_touch"
        );
        assert_eq!(
            device_identifier("pen").unwrap(),
            "4617:60882:ezdisplay_pen"
        );
        assert_eq!(
            device_identifier("pointer").unwrap(),
            "4617:60883:ezdisplay_pointer"
        );
        assert_eq!(device_identifier("keyboard"), None);
        assert_eq!(input_identifier(1, 2, " a b\tc "), "1:2:a_b_c");
    }

    #[test]
    fn mapping_commands() {
        assert_eq!(
            map_command("4617:60881:ezdisplay_touch", "HEADLESS-2"),
            "input 4617:60881:ezdisplay_touch map_to_output HEADLESS-2"
        );
        assert_eq!(
            map_command("4617:60881:ezdisplay_touch", "*"),
            "input 4617:60881:ezdisplay_touch map_to_output *"
        );
    }

    /// The whole pipeline in a headless sway: output, direct capture, libx264. Run it with
    /// `tools/wlroots-headless-test.sh`.
    #[test]
    fn headless_sway_encoded_frames() {
        use crate::source::LinuxSource;
        use ezd_host_core::{CursorSink, PlatformOptions, VideoSource};
        use ezd_proto::{Codec, StreamConfig};
        if !crate::wlroots_test::enabled() {
            return;
        }
        let mut src = LinuxSource::new(&PlatformOptions::default(), Box::new(SwayProvider));
        let cfg = StreamConfig {
            width: 1280,
            height: 720,
            refresh_mhz: 60_000,
            codec: Codec::H264,
            bitrate_kbps: 8000,
            encoder_name: "libx264".into(),
        };
        src.start(&cfg, CursorSink::null()).expect("start");
        let out = src.output().expect("output info");
        eprintln!("output: {out:?}");
        assert_eq!((out.mode.width, out.mode.height), (1280, 720));
        let _driver = crate::wlroots_test::Driver::start(&out.name);
        // Input mapping: sway accepts the commands for the three devices.
        {
            let mapped = SwayDesktop.map_devices(
                &[
                    ("touch", "event0"),
                    ("pen", "event1"),
                    ("pointer", "event2"),
                ],
                &out.name,
            );
            assert!(
                mapped.results.iter().all(Result::is_ok),
                "{:?}",
                mapped.results
            );
        }
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut frames = Vec::new();
        while frames.len() < 5 && Instant::now() < deadline {
            if let Some(f) = src.next_frame(Duration::from_millis(500)).expect("frame") {
                frames.push(f);
            }
        }
        src.stop();
        eprintln!("frames: {}", frames.len());
        assert!(frames.len() >= 3, "got {} frames", frames.len());
        assert!(frames[0].keyframe && frames[0].codec_config.is_some());
        let outputs = SwayIpc::connect().unwrap().outputs().unwrap();
        assert!(!outputs.iter().any(|o| o.name == out.name));
    }
}
