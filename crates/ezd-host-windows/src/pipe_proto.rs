//! Wire format of the Virtual-Display-Driver control pipe `\\.\pipe\MTTVirtualDisplayPipe`.
//!
//! Framing, as implemented by `HandleClient`/`NamedPipeServer` in the driver's `Driver.cpp`:
//!
//! * The pipe is a message-mode duplex pipe (`PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE`),
//!   created with SDDL `D:(A;;GA;;;WD)` (everyone), 512 byte buffers.
//! * **One command per connection.** The driver does a single `ReadFile` into a
//!   128-`wchar_t` buffer (so at most 127 UTF-16 code units), terminates it itself,
//!   matches the prefix with a `wcsncmp` chain, handles it, then calls
//!   `DisconnectNamedPipe` + `CloseHandle`. The next client waits for a fresh instance.
//! * The request is **UTF-16LE, no NUL terminator**, e.g. `PING`, `RELOAD_DRIVER`,
//!   `SETGPU "NVIDIA GeForce RTX 3050 Laptop GPU"` (the driver strips the first and
//!   last character after `SETGPU `, so a NUL or trailing space would eat the closing quote),
//!   `SETDISPLAYCOUNT 2`, `HARDWARECURSOR true`.
//! * Responses are **not UTF-16**: `PING` answers the raw ASCII bytes `PONG`; every other
//!   command only produces *log lines* (`YYYY-MM-DD HH:MM:SS [TYPE] text\n`, UTF-8)
//!   which the driver mirrors to the connected client, and only when driver logging and
//!   `SendLogsThroughPipe` are enabled. The one exception is `GETSETTINGS`, which writes
//!   a UTF-16LE NUL-terminated `SETTINGS DEBUG=<bool> LOG=<bool>`.
//! * End of response is the driver closing the pipe (the client sees `ERROR_BROKEN_PIPE`).
//!
//! Consequence: `GETASSIGNEDGPU`/`IDDCXVERSION` may legitimately return nothing when the
//! driver's logging is off, so callers fall back to `vdd_settings.xml` and to DXGI.

/// Maximum request length in UTF-16 code units (128 - terminator).
pub const MAX_REQUEST_UNITS: usize = 127;

/// Pipe name.
pub const PIPE_NAME: &str = r"\\.\pipe\MTTVirtualDisplayPipe";

/// A command understood by the driver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Heartbeat; answers `PONG`.
    Ping,
    /// Re-reads settings and restarts the adapter and monitors.
    ReloadDriver,
    /// Writes `<gpu><friendlyname>` itself and reloads.
    SetGpu(String),
    /// Writes `<monitors><count>` and reloads.
    SetDisplayCount(u32),
    /// Logs `ASSIGNED GPU: name (LUID: low-high)`.
    GetAssignedGpu,
    /// Logs `GPU Name: .. Memory: .. MB` for every adapter.
    GetAllGpus,
    /// Logs the D3D device GPU.
    D3dDeviceGpu,
    /// Logs `IDDCX Version: 0x..`.
    IddCxVersion,
    /// Answers `SETTINGS DEBUG=.. LOG=..` (UTF-16).
    GetSettings,
    /// Toggles debug logging.
    LogDebug(bool),
    /// Toggles logging.
    Logging(bool),
    /// Toggles the hardware cursor (and reloads).
    HardwareCursor(bool),
}

/// Request could not be encoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodeError(pub String);

impl std::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for EncodeError {}

impl Command {
    /// The request text.
    pub fn text(&self) -> String {
        fn b(v: bool) -> &'static str {
            if v { "true" } else { "false" }
        }
        match self {
            Command::Ping => "PING".into(),
            Command::ReloadDriver => "RELOAD_DRIVER".into(),
            Command::SetGpu(n) => format!("SETGPU \"{n}\""),
            Command::SetDisplayCount(n) => format!("SETDISPLAYCOUNT {n}"),
            Command::GetAssignedGpu => "GETASSIGNEDGPU".into(),
            Command::GetAllGpus => "GETALLGPUS".into(),
            Command::D3dDeviceGpu => "D3DDEVICEGPU".into(),
            Command::IddCxVersion => "IDDCXVERSION".into(),
            Command::GetSettings => "GETSETTINGS".into(),
            Command::LogDebug(v) => format!("LOG_DEBUG {}", b(*v)),
            Command::Logging(v) => format!("LOGGING {}", b(*v)),
            Command::HardwareCursor(v) => format!("HARDWARECURSOR {}", b(*v)),
        }
    }

    /// Request bytes: UTF-16LE, no terminator.
    pub fn encode(&self) -> Result<Vec<u8>, EncodeError> {
        let text = self.text();
        if let Command::SetGpu(n) = self
            && (n.contains('"') || n.is_empty())
        {
            return Err(EncodeError(format!("invalid GPU name {n:?} for SETGPU")));
        }
        let units: Vec<u16> = text.encode_utf16().collect();
        if units.len() > MAX_REQUEST_UNITS {
            return Err(EncodeError(format!(
                "command is {} UTF-16 units, the driver reads at most {MAX_REQUEST_UNITS}",
                units.len()
            )));
        }
        Ok(units.iter().flat_map(|u| u.to_le_bytes()).collect())
    }

    /// True if the driver restarts its adapter after this command, so the
    /// virtual output disappears and reappears.
    pub fn reloads_driver(&self) -> bool {
        matches!(
            self,
            Command::ReloadDriver
                | Command::SetGpu(_)
                | Command::SetDisplayCount(_)
                | Command::HardwareCursor(_)
        )
    }
}

/// Decodes one pipe message: UTF-16LE (`GETSETTINGS`) or UTF-8/ASCII (log lines, `PONG`).
pub fn decode_message(bytes: &[u8]) -> String {
    let looks_utf16 = bytes.len() >= 2
        && bytes.len() % 2 == 0
        && bytes.iter().skip(1).step_by(2).all(|&b| b == 0)
        && bytes.iter().step_by(2).any(|&b| b != 0);
    let s = if looks_utf16 {
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    };
    s.trim_matches(|c: char| c == '\0' || c.is_whitespace())
        .to_string()
}

/// True if any message is the `PONG` heartbeat answer.
pub fn is_pong(messages: &[String]) -> bool {
    messages.iter().any(|m| m.contains("PONG"))
}

/// GPU reported by `GETASSIGNEDGPU`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignedGpu {
    /// Adapter description.
    pub name: String,
    /// LUID as `(high << 32) | low`, when reported.
    pub luid: Option<u64>,
}

/// Parses `ASSIGNED GPU: <name> (LUID: <low>-<high>)` out of the messages.
pub fn assigned_gpu(messages: &[String]) -> Option<AssignedGpu> {
    for m in messages {
        for line in m.lines() {
            let Some(i) = line.find("ASSIGNED GPU:") else {
                continue;
            };
            let rest = line[i + "ASSIGNED GPU:".len()..].trim();
            return Some(match rest.rfind("(LUID:") {
                Some(j) => {
                    let name = rest[..j].trim().to_string();
                    let luid = rest[j + "(LUID:".len()..]
                        .trim()
                        .trim_end_matches(')')
                        .trim()
                        .split_once('-')
                        .and_then(|(lo, hi)| {
                            let lo: u32 = lo.trim().parse().ok()?;
                            let hi: i32 = hi.trim().parse().ok()?;
                            Some(((hi as u32 as u64) << 32) | lo as u64)
                        });
                    AssignedGpu { name, luid }
                }
                None => AssignedGpu {
                    name: rest.to_string(),
                    luid: None,
                },
            });
        }
    }
    None
}

/// Parses `IDDCX Version: 0x...`.
pub fn iddcx_version(messages: &[String]) -> Option<String> {
    messages.iter().flat_map(|m| m.lines()).find_map(|l| {
        let i = l.find("IDDCX Version:")?;
        Some(l[i + "IDDCX Version:".len()..].trim().to_string())
    })
}

/// Parses `GPU Name: <name> Memory: <n> MB` lines (`GETALLGPUS`, `D3DDEVICEGPU`).
pub fn gpu_lines(messages: &[String]) -> Vec<String> {
    messages
        .iter()
        .flat_map(|m| m.lines())
        .filter_map(|l| {
            let i = l.find("GPU Name:")?;
            Some(l[i..].trim().to_string())
        })
        .collect()
}

/// Parses the `GETSETTINGS` answer into `(debug, log)`.
pub fn settings(messages: &[String]) -> Option<(bool, bool)> {
    let m = messages.iter().find(|m| m.contains("SETTINGS"))?;
    let flag = |key: &str| m.split_whitespace().find_map(|t| t.strip_prefix(key));
    Some((flag("DEBUG=")? == "true", flag("LOG=")? == "true"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
    }

    #[test]
    fn commands_are_utf16le_without_terminator() {
        assert_eq!(Command::Ping.encode().unwrap(), utf16("PING"));
        assert_eq!(
            Command::ReloadDriver.encode().unwrap(),
            utf16("RELOAD_DRIVER")
        );
        assert_eq!(
            Command::SetGpu("NVIDIA GeForce RTX 3050 Laptop GPU".into())
                .encode()
                .unwrap(),
            utf16("SETGPU \"NVIDIA GeForce RTX 3050 Laptop GPU\"")
        );
        assert_eq!(Command::SetDisplayCount(2).text(), "SETDISPLAYCOUNT 2");
        assert_eq!(Command::LogDebug(true).text(), "LOG_DEBUG true");
        assert_eq!(Command::Logging(false).text(), "LOGGING false");
        assert_eq!(Command::HardwareCursor(true).text(), "HARDWARECURSOR true");
        // The driver matches prefixes: every text starts with its verb.
        assert!(Command::GetAssignedGpu.text().starts_with("GETASSIGNEDGPU"));
    }

    #[test]
    fn oversized_or_malformed_requests_are_rejected() {
        assert!(Command::SetGpu("x".repeat(200)).encode().is_err());
        assert!(Command::SetGpu("a\"b".into()).encode().is_err());
        assert!(Command::SetGpu(String::new()).encode().is_err());
        let longest_ok = Command::SetGpu("x".repeat(MAX_REQUEST_UNITS - 9));
        assert!(longest_ok.encode().is_ok());
    }

    #[test]
    fn reload_classification() {
        assert!(Command::SetGpu("x".into()).reloads_driver());
        assert!(Command::ReloadDriver.reloads_driver());
        assert!(!Command::Ping.reloads_driver());
        assert!(!Command::GetAssignedGpu.reloads_driver());
    }

    #[test]
    fn decodes_ascii_utf8_and_utf16_messages() {
        assert_eq!(decode_message(b"PONG"), "PONG");
        assert_eq!(
            decode_message(b"2024-05-01 10:00:00 [COMPANION] hi\n"),
            "2024-05-01 10:00:00 [COMPANION] hi"
        );
        let mut s = utf16("SETTINGS DEBUG=false LOG=true");
        s.extend([0, 0]);
        assert_eq!(decode_message(&s), "SETTINGS DEBUG=false LOG=true");
        assert_eq!(decode_message(b""), "");
    }

    #[test]
    fn parses_answers() {
        let msgs = vec![
            "2024-05-01 10:00:00 [PIPE] Client Handling Enabled".to_string(),
            "2024-05-01 10:00:00 [INFO] ASSIGNED GPU: NVIDIA GeForce RTX 3050 Laptop GPU (LUID: 12345-0)"
                .to_string(),
            "2024-05-01 10:00:00 [INFO] IDDCX Version: 0x1a00".to_string(),
        ];
        let g = assigned_gpu(&msgs).unwrap();
        assert_eq!(g.name, "NVIDIA GeForce RTX 3050 Laptop GPU");
        assert_eq!(g.luid, Some(12345));
        assert_eq!(iddcx_version(&msgs).as_deref(), Some("0x1a00"));
        assert!(!is_pong(&msgs));
        assert!(is_pong(&["PONG".to_string()]));
        let neg = vec!["[INFO] ASSIGNED GPU: X (LUID: 7--1)".to_string()];
        // HighPart is a signed LONG: -1 sets the upper 32 bits.
        let g = assigned_gpu(&neg).unwrap();
        assert_eq!(g.name, "X");
        assert_eq!(g.luid, Some(0xFFFF_FFFF_0000_0007));
        assert_eq!(assigned_gpu(&["nothing".to_string()]), None);
        assert_eq!(
            settings(&["SETTINGS DEBUG=true LOG=false".to_string()]),
            Some((true, false))
        );
        let lines = vec!["[COMPANION] GPU Name: Intel(R) UHD Graphics Memory: 128 MB".to_string()];
        assert_eq!(
            gpu_lines(&lines),
            ["GPU Name: Intel(R) UHD Graphics Memory: 128 MB"]
        );
    }
}
