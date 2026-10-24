//! Host-side ADB helper (not available on Android).
//!
//! Wraps the `adb` command line: locating the binary, listing devices and
//! managing `adb reverse` port forwards. All calls are blocking
//! (`std::process`); call them from `spawn_blocking` inside async code.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Default port used for the host listener and `adb reverse`.
pub const DEFAULT_PORT: u16 = ezd_proto::DEFAULT_PORT;

/// Errors from ADB invocations.
#[derive(Debug, thiserror::Error)]
pub enum AdbError {
    /// `adb` could not be found.
    #[error(
        "adb not found (looked in PATH, $ANDROID_HOME, $ANDROID_SDK_ROOT and default SDK locations)"
    )]
    NotFound,
    /// Failed to spawn the process.
    #[error("failed to run adb: {0}")]
    Io(#[from] io::Error),
    /// adb exited with a non-zero status.
    #[error("adb {args} failed ({status}): {stderr}")]
    Failed {
        /// Arguments passed to adb.
        args: String,
        /// Exit status description.
        status: String,
        /// Trimmed stderr (or stdout if stderr was empty).
        stderr: String,
    },
}

/// Connection state reported by `adb devices`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceState {
    /// Ready (`device`).
    Device,
    /// `offline`.
    Offline,
    /// `unauthorized` (RSA prompt not accepted).
    Unauthorized,
    /// Anything else (`recovery`, `no permissions`, ...), raw first token.
    Other(String),
}

/// How a device is attached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// USB cable.
    Usb,
    /// TCP/IP, e.g. `192.168.0.5:44371` (wireless debugging).
    Tcp,
    /// Android emulator (`emulator-5554`).
    Emulator,
}

/// One line of `adb devices -l`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdbDevice {
    /// Serial; for TCP devices this is `host:port`.
    pub serial: String,
    /// Connection state.
    pub state: DeviceState,
    /// `model:` property, if reported.
    pub model: Option<String>,
    /// `product:` property, if reported.
    pub product: Option<String>,
    /// `transport_id:` property, if reported.
    pub transport_id: Option<String>,
    /// Attachment kind derived from the serial and the `usb:` field.
    pub transport: Transport,
}

impl AdbDevice {
    /// True if the device is in the `device` state.
    pub fn is_ready(&self) -> bool {
        self.state == DeviceState::Device
    }
}

/// Parses the output of `adb devices -l`. Daemon chatter (`* daemon ...`) and
/// the header are ignored.
pub fn parse_devices(output: &str) -> Vec<AdbDevice> {
    output
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('*') && !l.starts_with("List of devices"))
        .filter_map(|line| {
            let mut it = line.split_whitespace();
            let serial = it.next()?.to_string();
            let state = match it.next()? {
                "device" => DeviceState::Device,
                "offline" => DeviceState::Offline,
                "unauthorized" => DeviceState::Unauthorized,
                other => DeviceState::Other(other.to_string()),
            };
            let (mut model, mut product, mut transport_id, mut has_usb) = (None, None, None, false);
            for tok in it {
                if let Some((k, v)) = tok.split_once(':') {
                    match k {
                        "model" => model = Some(v.to_string()),
                        "product" => product = Some(v.to_string()),
                        "transport_id" => transport_id = Some(v.to_string()),
                        "usb" => has_usb = true,
                        _ => {}
                    }
                }
            }
            let transport = if serial.starts_with("emulator-") {
                Transport::Emulator
            } else if has_usb {
                Transport::Usb
            } else if serial.contains(':') || serial.contains("._adb-tls-connect.") {
                Transport::Tcp
            } else {
                Transport::Usb
            };
            Some(AdbDevice {
                serial,
                state,
                model,
                product,
                transport_id,
                transport,
            })
        })
        .collect()
}

/// Candidate locations of the adb binary, in lookup order. Pure so it can be tested.
pub fn adb_candidates(
    path_var: Option<&OsString>,
    android_home: Option<&OsString>,
    sdk_root: Option<&OsString>,
    home: Option<&OsString>,
    local_app_data: Option<&OsString>,
    exe_dir: Option<&Path>,
) -> Vec<PathBuf> {
    let exe = if cfg!(windows) { "adb.exe" } else { "adb" };
    let mut v = Vec::new();
    if let Some(p) = path_var {
        v.extend(std::env::split_paths(p).map(|d| d.join(exe)));
    }
    for sdk in [android_home, sdk_root].into_iter().flatten() {
        v.push(Path::new(sdk).join("platform-tools").join(exe));
    }
    if let Some(h) = home {
        v.push(
            Path::new(h)
                .join("Android")
                .join("Sdk")
                .join("platform-tools")
                .join(exe),
        );
    }
    if let Some(l) = local_app_data {
        v.push(
            Path::new(l)
                .join("Android")
                .join("Sdk")
                .join("platform-tools")
                .join(exe),
        );
    }
    // Bundled copy (installer layout), last so an SDK adb server of another version is not
    // fought over.
    if let Some(d) = exe_dir {
        v.push(d.join("adb").join(exe));
    }
    v
}

/// Finds adb in `PATH`, `$ANDROID_HOME`, `$ANDROID_SDK_ROOT`, `~/Android/Sdk/platform-tools`,
/// `%LOCALAPPDATA%\Android\Sdk\platform-tools` and, last, `adb/adb(.exe)` next to the running
/// executable.
pub fn find_adb() -> Option<PathBuf> {
    let var = |k: &str| std::env::var_os(k);
    let home = var("HOME").or_else(|| var("USERPROFILE"));
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    adb_candidates(
        var("PATH").as_ref(),
        var("ANDROID_HOME").as_ref(),
        var("ANDROID_SDK_ROOT").as_ref(),
        home.as_ref(),
        var("LOCALAPPDATA").as_ref(),
        exe_dir.as_deref(),
    )
    .into_iter()
    .find(|p| p.is_file())
}

/// Every adb process is spawned through here: on Windows it must not flash a console window when
/// the caller is a GUI-subsystem process (`CREATE_NO_WINDOW`).
fn command(path: &Path) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(path);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

/// Handle to an adb binary.
#[derive(Debug, Clone)]
pub struct Adb {
    path: PathBuf,
}

impl Adb {
    /// Locates adb with [`find_adb`].
    pub fn locate() -> Result<Adb, AdbError> {
        find_adb().map(Adb::new).ok_or(AdbError::NotFound)
    }

    /// Uses an explicit adb path.
    pub fn new(path: impl Into<PathBuf>) -> Adb {
        Adb { path: path.into() }
    }

    /// Path of the adb binary.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn run(&self, args: &[&str]) -> Result<String, AdbError> {
        let out = command(&self.path).args(args).output()?;
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        if !out.status.success() {
            let mut stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            if stderr.is_empty() {
                stderr = stdout.trim().to_string();
            }
            return Err(AdbError::Failed {
                args: args.join(" "),
                status: out.status.to_string(),
                stderr,
            });
        }
        Ok(stdout)
    }

    /// `adb version` output (first line), useful for diagnostics.
    pub fn version(&self) -> Result<String, AdbError> {
        Ok(self
            .run(&["version"])?
            .lines()
            .next()
            .unwrap_or("")
            .to_string())
    }

    /// Lists attached devices (`adb devices -l`).
    pub fn devices(&self) -> Result<Vec<AdbDevice>, AdbError> {
        Ok(parse_devices(&self.run(&["devices", "-l"])?))
    }

    /// `adb -s SERIAL reverse tcp:DEVICE_PORT tcp:HOST_PORT`: connections the
    /// device makes to its `localhost:device_port` reach `host_port` on the PC.
    pub fn reverse(&self, serial: &str, device_port: u16, host_port: u16) -> Result<(), AdbError> {
        let (d, h) = (format!("tcp:{device_port}"), format!("tcp:{host_port}"));
        self.run(&["-s", serial, "reverse", &d, &h]).map(drop)
    }

    /// Removes a reverse forward set up by [`reverse`](Self::reverse).
    pub fn remove_reverse(&self, serial: &str, device_port: u16) -> Result<(), AdbError> {
        let d = format!("tcp:{device_port}");
        self.run(&["-s", serial, "reverse", "--remove", &d])
            .map(drop)
    }

    /// Runs `adb -s SERIAL shell ARGS...` and returns stdout.
    pub fn shell(&self, serial: &str, args: &[&str]) -> Result<String, AdbError> {
        let mut full = vec!["-s", serial, "shell"];
        full.extend_from_slice(args);
        self.run(&full)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "* daemon not running; starting now at tcp:5037\n\
* daemon started successfully\n\
List of devices attached\n\
R52W80XXXXX            device usb:1-2 product:gta7litexx model:SM_X110 device:gta7lite transport_id:3\n\
192.168.0.5:44371      device product:tangorpro model:Pixel_Tablet device:tangorpro transport_id:5\n\
emulator-5554          device product:sdk_gphone64_x86_64 model:sdk_gphone64_x86_64 device:emu64xa transport_id:1\n\
0123456789ABCDEF       unauthorized usb:1-1 transport_id:2\n\
ZZZ                    offline\n\
adb-RZ8N-abc._adb-tls-connect._tcp device product:x model:Pixel_8 device:shiba transport_id:7\n\
\n";

    #[test]
    fn parses_sample() {
        let d = parse_devices(SAMPLE);
        assert_eq!(d.len(), 6);
        assert_eq!(d[0].serial, "R52W80XXXXX");
        assert!(d[0].is_ready());
        assert_eq!(d[0].model.as_deref(), Some("SM_X110"));
        assert_eq!(d[0].transport, Transport::Usb);
        assert_eq!(d[0].transport_id.as_deref(), Some("3"));
        assert_eq!(d[1].serial, "192.168.0.5:44371");
        assert_eq!(d[1].transport, Transport::Tcp);
        assert_eq!(d[1].model.as_deref(), Some("Pixel_Tablet"));
        assert_eq!(d[2].transport, Transport::Emulator);
        assert_eq!(d[3].state, DeviceState::Unauthorized);
        assert!(!d[3].is_ready());
        assert_eq!(d[4].state, DeviceState::Offline);
        assert_eq!(d[4].model, None);
        assert_eq!(d[5].transport, Transport::Tcp);
    }

    #[test]
    fn parses_empty_and_unusual_states() {
        assert!(parse_devices("List of devices attached\n\n").is_empty());
        let d = parse_devices(
            "List of devices attached\nabc no permissions (user in plugdev group); see [x]\n",
        );
        assert_eq!(d[0].state, DeviceState::Other("no".into()));
    }

    #[test]
    fn candidate_order() {
        let sep = if cfg!(windows) { ";" } else { ":" };
        let path = OsString::from(format!("/a{sep}/b"));
        let (ah, sr, home, lad) = (
            OsString::from("/ah"),
            OsString::from("/sr"),
            OsString::from("/home/u"),
            OsString::from("/lad"),
        );
        let c = adb_candidates(
            Some(&path),
            Some(&ah),
            Some(&sr),
            Some(&home),
            Some(&lad),
            Some(Path::new("/opt/ezd")),
        );
        let exe = if cfg!(windows) { "adb.exe" } else { "adb" };
        let want: Vec<PathBuf> = vec![
            Path::new("/a").join(exe),
            Path::new("/b").join(exe),
            Path::new("/ah/platform-tools").join(exe),
            Path::new("/sr/platform-tools").join(exe),
            Path::new("/home/u/Android/Sdk/platform-tools").join(exe),
            Path::new("/lad/Android/Sdk/platform-tools").join(exe),
            Path::new("/opt/ezd/adb").join(exe),
        ];
        assert_eq!(c, want);
        assert!(adb_candidates(None, None, None, None, None, None).is_empty());
    }
}
