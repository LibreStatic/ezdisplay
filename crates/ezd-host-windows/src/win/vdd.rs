//! Virtual-Display-Driver (VirtualDrivers/Virtual-Display-Driver, IddCx) control:
//! config directory, `vdd_settings.xml`, the control pipe, `vdd setup/revert/status`
//! and the start-time preparation of the virtual monitor.

use super::display::{self, VddMonitor};
use super::dxgi::{self, AdapterEntry, OutputEntry};
use super::util::{err_str, poll, reg_dword, reg_string, wide};
use crate::pipe_proto::{self, Command, PIPE_NAME};
use crate::policy::{self, AdapterInfo};
use crate::vdd_xml::{self, Mode, Summary};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_BROKEN_PIPE, ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_PIPE_BUSY,
    GENERIC_READ, GENERIC_WRITE, HANDLE,
};
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_MODE, OPEN_EXISTING, ReadFile, WriteFile,
};
use windows::Win32::System::Pipes::{
    NAMED_PIPE_MODE, PIPE_READMODE_MESSAGE, PeekNamedPipe, SetNamedPipeHandleState, WaitNamedPipeW,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::PCWSTR;

/// Registry key of the driver (HKLM, 64-bit view).
pub const REG_KEY: &str = r"SOFTWARE\MikeTheTech\VirtualDisplayDriver";
/// Default config directory when the registry has no `VDDPATH`.
pub const DEFAULT_DIR: &str = r"C:\VirtualDisplayDriver";
/// SID of BUILTIN\Users.
pub const USERS_SID: &str = "*S-1-5-32-545";
/// Install hint used in error messages.
pub const INSTALL_HINT: &str = "reinstall ezdisplay or use Repair in the app settings";

/// Where the config dir comes from.
pub fn config_dir() -> (PathBuf, &'static str) {
    match reg_string(REG_KEY, "VDDPATH").filter(|s| !s.trim().is_empty()) {
        Some(p) => (PathBuf::from(p.trim()), "registry VDDPATH"),
        None => (PathBuf::from(DEFAULT_DIR), "default"),
    }
}

/// Path of `vdd_settings.xml`.
pub fn settings_path() -> PathBuf {
    config_dir().0.join("vdd_settings.xml")
}

/// Whether this process can open the settings file for writing (does not modify it).
pub fn settings_writable() -> bool {
    std::fs::OpenOptions::new()
        .write(true)
        .open(settings_path())
        .is_ok()
}

/// True if the process token is elevated.
pub fn is_elevated() -> bool {
    let mut token = HANDLE::default();
    // SAFETY: valid out-pointer; the handle is closed below.
    unsafe {
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elev = TOKEN_ELEVATION::default();
        let mut ret = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some((&mut elev as *mut TOKEN_ELEVATION).cast()),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut ret,
        )
        .is_ok();
        let _ = CloseHandle(token);
        ok && elev.TokenIsElevated != 0
    }
}

// ---------------------------------------------------------------- pipe client

/// Why a pipe command failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PipeError {
    /// The pipe does not exist: driver missing, disabled or reloading.
    NotPresent,
    /// Any other failure.
    Other(String),
}

impl std::fmt::Display for PipeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PipeError::NotPresent => write!(f, "pipe {PIPE_NAME} does not exist"),
            PipeError::Other(s) => f.write_str(s),
        }
    }
}

/// What the driver sent back.
#[derive(Debug, Clone, Default)]
pub struct PipeReply {
    /// Decoded messages (see [`pipe_proto`]).
    pub messages: Vec<String>,
    /// The driver did not close the pipe within the read timeout.
    pub timed_out: bool,
}

struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: we own the handle.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

fn open_pipe() -> Result<OwnedHandle, PipeError> {
    let name = wide(PIPE_NAME);
    for _ in 0..8 {
        // SAFETY: NUL-terminated name; no security attributes or template.
        let r = unsafe {
            CreateFileW(
                PCWSTR(name.as_ptr()),
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_MODE(0),
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        };
        match r {
            Ok(h) => return Ok(OwnedHandle(h)),
            Err(e) if e.code() == ERROR_FILE_NOT_FOUND.to_hresult() => {
                return Err(PipeError::NotPresent);
            }
            Err(e) if e.code() == ERROR_PIPE_BUSY.to_hresult() => {
                // The driver serves one client at a time.
                // SAFETY: NUL-terminated name.
                let _ = unsafe { WaitNamedPipeW(PCWSTR(name.as_ptr()), 2000) };
            }
            Err(e) => return Err(PipeError::Other(format!("open pipe: {}", err_str(&e)))),
        }
    }
    Err(PipeError::Other("pipe stayed busy".into()))
}

/// Sends one command (one connection per command) and collects the reply until
/// the driver closes the pipe or `read_timeout` passes.
pub fn pipe_send(cmd: &Command, read_timeout: Duration) -> Result<PipeReply, PipeError> {
    let bytes = cmd.encode().map_err(|e| PipeError::Other(e.to_string()))?;
    let h = open_pipe()?;
    let mode = PIPE_READMODE_MESSAGE;
    // SAFETY: valid handle and pointer to a mode value.
    unsafe { SetNamedPipeHandleState(h.0, Some(&mode as *const NAMED_PIPE_MODE), None, None) }
        .map_err(|e| PipeError::Other(format!("SetNamedPipeHandleState: {}", err_str(&e))))?;
    let mut written = 0u32;
    // SAFETY: valid handle and buffer.
    unsafe { WriteFile(h.0, Some(&bytes), Some(&mut written), None) }
        .map_err(|e| PipeError::Other(format!("write to pipe: {}", err_str(&e))))?;

    let broken = ERROR_BROKEN_PIPE.to_hresult();
    let more = ERROR_MORE_DATA.to_hresult();
    let deadline = Instant::now() + read_timeout;
    let mut reply = PipeReply::default();
    'outer: loop {
        let mut avail = 0u32;
        // SAFETY: valid handle and out-pointer.
        match unsafe { PeekNamedPipe(h.0, None, 0, None, Some(&mut avail), None) } {
            Err(e) if e.code() == broken => break,
            Err(e) => return Err(PipeError::Other(format!("PeekNamedPipe: {}", err_str(&e)))),
            Ok(()) if avail == 0 => {
                if Instant::now() >= deadline {
                    reply.timed_out = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
            Ok(()) => {}
        }
        let mut msg = Vec::new();
        loop {
            let mut buf = [0u8; 4096];
            let mut n = 0u32;
            // SAFETY: valid handle and buffer.
            let r = unsafe { ReadFile(h.0, Some(&mut buf), Some(&mut n), None) };
            msg.extend_from_slice(&buf[..n as usize]);
            match r {
                Ok(()) => break,
                Err(e) if e.code() == more => continue,
                Err(e) if e.code() == broken => {
                    if !msg.is_empty() {
                        reply.messages.push(pipe_proto::decode_message(&msg));
                    }
                    break 'outer;
                }
                Err(e) => return Err(PipeError::Other(format!("read from pipe: {}", err_str(&e)))),
            }
        }
        reply.messages.push(pipe_proto::decode_message(&msg));
    }
    reply.messages.retain(|m| !m.is_empty());
    Ok(reply)
}

/// `PING` -> `PONG`.
pub fn ping() -> Result<bool, PipeError> {
    Ok(pipe_proto::is_pong(
        &pipe_send(&Command::Ping, Duration::from_millis(1500))?.messages,
    ))
}

// ------------------------------------------------------------------- settings

/// Reads and parses `vdd_settings.xml`.
pub fn read_settings() -> Result<(String, Summary), String> {
    let p = settings_path();
    let xml =
        std::fs::read_to_string(&p).map_err(|e| format!("cannot read {}: {e}", p.display()))?;
    let s = vdd_xml::parse(&xml).map_err(|e| format!("{}: {e}", p.display()))?;
    Ok((xml, s))
}

fn reg_flag(name: &str) -> Value {
    json!(reg_dword(REG_KEY, name))
}

/// JSON for `diag` and `vdd status`. Best effort, does not change anything.
pub fn status_json() -> Value {
    let (dir, dir_src) = config_dir();
    let path = settings_path();
    let settings = match read_settings() {
        Ok((_, s)) => s.to_json(),
        Err(e) => json!({ "error": e }),
    };
    let pipe = match pipe_send(&Command::Ping, Duration::from_millis(1500)) {
        Ok(r) => {
            let query =
                |c: Command| pipe_send(&c, Duration::from_millis(1500)).map_err(|e| e.to_string());
            let assigned = query(Command::GetAssignedGpu);
            let iddcx = query(Command::IddCxVersion);
            let d3d = query(Command::D3dDeviceGpu);
            let st = query(Command::GetSettings);
            json!({
                "present": true,
                "ping_pong": pipe_proto::is_pong(&r.messages),
                "iddcx_version": iddcx.as_ref().ok().and_then(|r| pipe_proto::iddcx_version(&r.messages)),
                "assigned_gpu": assigned.as_ref().ok().and_then(|r| pipe_proto::assigned_gpu(&r.messages)).map(|g| json!({"name": g.name, "luid": g.luid.map(|l| format!("0x{l:016x}"))})),
                "d3d_device_gpu": d3d.as_ref().ok().map(|r| pipe_proto::gpu_lines(&r.messages)),
                "driver_settings": st.as_ref().ok().and_then(|r| pipe_proto::settings(&r.messages)).map(|(d, l)| json!({"debug_logging": d, "logging": l})),
                "raw_messages": {
                    "ping": r.messages,
                    "assigned_gpu": assigned.as_ref().map(|r| r.messages.clone()).map_err(|e| e.clone()),
                    "iddcx_version": iddcx.as_ref().map(|r| r.messages.clone()).map_err(|e| e.clone()),
                },
                "note": "answers other than PONG are driver log lines: they are empty when the driver's logging or SendLogsThroughPipe is off",
            })
        }
        Err(PipeError::NotPresent) => {
            json!({ "present": false, "error": PipeError::NotPresent.to_string() })
        }
        Err(e) => json!({ "present": true, "error": e.to_string() }),
    };
    json!({
        "install_hint": INSTALL_HINT,
        "owned": super::vdd_install::owned(),
        "driver_dir": super::vdd_install::stored_driver_dir(),
        "elevated": is_elevated(),
        "config_dir": dir.display().to_string(),
        "config_dir_source": dir_src,
        "config_dir_exists": dir.is_dir(),
        "settings_file": path.display().to_string(),
        "settings_file_exists": path.is_file(),
        "settings_writable_by_this_process": settings_writable(),
        "settings": settings,
        "registry_toggles": {
            "HARDWARECURSOR": reg_flag("HARDWARECURSOR"),
            "LOGS": reg_flag("LOGS"),
            "SENDLOGSTHROUGHPIPE": reg_flag("SENDLOGSTHROUGHPIPE"),
        },
        "pipe": pipe,
        "monitors": display::find_vdd_monitors().into_iter().map(|m| json!({"gdi_name": m.gdi_name, "how": m.how, "attached": m.attached})).collect::<Vec<_>>(),
    })
}

// ------------------------------------------------------------------ setup/revert

/// Grants BUILTIN\Users Modify on one file (not its directory: the directory may hold tools an
/// administrator runs later, and a writable directory would let any user replace them).
pub fn grant_users_modify(path: &Path) -> Result<String, String> {
    let file_s = path.display().to_string();
    let grant = format!("{USERS_SID}:M");
    run_icacls(&[&file_s, "/grant", &grant])?;
    Ok(grant)
}

fn run_icacls(args: &[&str]) -> Result<String, String> {
    let sys = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    let exe = format!(r"{sys}\System32\icacls.exe");
    let out = std::process::Command::new(&exe)
        .args(args)
        .output()
        .map_err(|e| format!("cannot run {exe}: {e}"))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if out.status.success() {
        Ok(text)
    } else {
        Err(format!(
            "icacls {} failed ({}): {}",
            args.join(" "),
            out.status,
            text.trim()
        ))
    }
}

fn require_elevated(what: &str) -> Result<(), String> {
    if is_elevated() {
        Ok(())
    } else {
        Err(format!(
            "`ezd-host vdd {what}` must run elevated: open a terminal with \"Run as administrator\" and repeat the \
             command. It is needed once; later runs do not need administrator rights."
        ))
    }
}

/// Resolves `--gpu NAME` to the exact adapter description (or `default`).
fn resolve_gpu(spec: &str) -> Result<String, String> {
    if spec.eq_ignore_ascii_case("default") {
        return Ok("default".into());
    }
    let adapters = dxgi::enumerate()?;
    let infos: Vec<AdapterInfo> = adapters.iter().map(|a| a.info.clone()).collect();
    match policy::match_adapters(&infos, spec).first() {
        Some(a) => Ok(a.name.clone()),
        None => Err(format!(
            "no GPU matches {spec:?}; available: {}",
            infos
                .iter()
                .filter(|a| !a.is_software())
                .map(|a| a.name.clone())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// `ezd-host vdd setup`.
pub fn setup(gpu: Option<&str>, modes: &[Mode]) -> Result<Vec<String>, String> {
    require_elevated("setup")?;
    let (dir, src) = config_dir();
    let path = settings_path();
    if !dir.is_dir() || !path.is_file() {
        return Err(format!(
            "VDD config not found ({} does not exist). To install the driver: {INSTALL_HINT}",
            path.display()
        ));
    }
    let mut out = vec![format!("VDD config dir: {} ({src})", dir.display())];
    let gpu_name = gpu.map(resolve_gpu).transpose()?;

    // Only the settings file, see `grant_users_modify`.
    let file_s = path.display().to_string();
    let grant = grant_users_modify(&path)?;
    out.push(format!(
        "changed: granted BUILTIN\\Users (S-1-5-32-545) Modify on {file_s} \
         (icacls \"{file_s}\" /grant {grant}). Undo with `ezd-host vdd revert`."
    ));

    let mut xml = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let original = xml.clone();
    if let Some(name) = &gpu_name {
        let cur = vdd_xml::parse(&xml).map_err(|e| e.to_string())?.gpu;
        if cur.as_deref() == Some(name.as_str()) {
            out.push(format!(
                "unchanged: <gpu><friendlyname> is already {name:?}"
            ));
        } else {
            xml = vdd_xml::set_gpu(&xml, name).map_err(|e| e.to_string())?;
            out.push(format!(
                "changed: <gpu><friendlyname> {:?} -> {name:?}",
                cur.unwrap_or_default()
            ));
        }
    }
    for m in modes {
        match vdd_xml::add_mode(&xml, *m).map_err(|e| e.to_string())? {
            Some(n) => {
                xml = n;
                out.push(format!("changed: added mode {m} to <resolutions>"));
            }
            None => out.push(format!("unchanged: mode {m} is already offered")),
        }
    }
    if xml != original {
        let bak = path.with_file_name("vdd_settings.xml.ezd-backup");
        if !bak.exists() {
            std::fs::write(&bak, &original)
                .map_err(|e| format!("cannot write {}: {e}", bak.display()))?;
            out.push(format!(
                "created backup of the original settings: {}",
                bak.display()
            ));
        }
        std::fs::write(&path, &xml).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        out.push(format!("wrote {}", path.display()));
        match pipe_send(&Command::ReloadDriver, Duration::from_secs(5)) {
            Ok(_) => out.push("sent RELOAD_DRIVER to the VDD (the virtual monitor restarts)".into()),
            Err(e) => out.push(format!(
                "warning: could not send RELOAD_DRIVER ({e}); disable/enable the \"Virtual Display Driver\" in Device Manager or reboot to apply"
            )),
        }
    } else {
        out.push("settings file unchanged, driver not reloaded".into());
    }
    if let Ok((_, s)) = read_settings() {
        if s.hardware_cursor == Some(false) {
            out.push(
                "note: <HardwareCursor> is false in the settings; the pointer will be drawn into the video instead of travelling on the cursor channel".into(),
            );
        }
    }
    Ok(out)
}

/// `ezd-host vdd revert`.
pub fn revert() -> Result<Vec<String>, String> {
    require_elevated("revert")?;
    let path = settings_path();
    if !path.is_file() {
        return Err(format!(
            "{} does not exist, nothing to revert",
            path.display()
        ));
    }
    let file_s = path.display().to_string();
    run_icacls(&[&file_s, "/remove:g", USERS_SID])?;
    Ok(vec![format!(
        "changed: removed the explicit grant of BUILTIN\\Users (S-1-5-32-545) on {file_s} \
         (icacls \"{file_s}\" /remove:g {USERS_SID}). Settings edits made by `setup` are kept; the original is in vdd_settings.xml.ezd-backup if it exists."
    )])
}

// -------------------------------------------------------------- start preparation

/// The virtual (or forced) output ready to be captured.
pub struct Prepared {
    pub adapter: AdapterEntry,
    pub output: OutputEntry,
    /// Human readable notes about what was done / warnings.
    pub notes: Vec<String>,
}

fn name_matches(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim().to_lowercase(), b.trim().to_lowercase());
    !a.is_empty() && !b.is_empty() && (a == b || a.contains(&b) || b.contains(&a))
}

pub fn find_vdd_output(
    adapters: &[AdapterEntry],
    monitors: &[VddMonitor],
) -> Option<(usize, usize)> {
    for m in monitors {
        for (ai, a) in adapters.iter().enumerate() {
            if let Some(oi) = a
                .outputs
                .iter()
                .position(|o| o.device_name.eq_ignore_ascii_case(&m.gdi_name))
            {
                return Some((ai, oi));
            }
        }
    }
    None
}

fn snapshot() -> Result<(Vec<AdapterEntry>, Vec<VddMonitor>), String> {
    Ok((dxgi::enumerate()?, display::find_vdd_monitors()))
}

/// Waits until the VDD output exists (optionally on `luid`) and returns a fresh snapshot.
fn wait_for_vdd(
    timeout: Duration,
    luid: Option<u64>,
    require_attached: bool,
) -> Option<(Vec<AdapterEntry>, Vec<VddMonitor>)> {
    poll(timeout, Duration::from_millis(500), || {
        let (adapters, monitors) = snapshot().ok()?;
        let (ai, oi) = find_vdd_output(&adapters, &monitors)?;
        let ok = luid.is_none_or(|l| adapters[ai].info.luid == l)
            && (!require_attached || adapters[ai].outputs[oi].attached);
        ok.then_some((adapters, monitors))
    })
}

/// Makes the VDD output exist on the adapter `chosen`, with `mode` active.
pub fn prepare(chosen: &AdapterInfo, mode: Mode) -> Result<Prepared, String> {
    let mut notes = Vec::new();
    let (_, summary) = read_settings().map_err(|e| {
        format!("{e}. Is the Virtual Display Driver installed? To install it: {INSTALL_HINT}")
    })?;
    match ping() {
        Ok(true) => {}
        Ok(false) => notes.push("VDD pipe answered but not with PONG".to_string()),
        Err(e) => {
            return Err(format!(
                "cannot talk to the Virtual Display Driver ({e}). Check that it is installed and enabled \
                 (Device Manager > Display adapters > Virtual Display Driver) and run `ezd-host vdd status`."
            ));
        }
    }
    if summary.hardware_cursor == Some(false) && reg_dword(REG_KEY, "HARDWARECURSOR") != Some(1) {
        let m = "VDD <HardwareCursor> is false: the pointer is drawn into the video, not sent on the cursor channel";
        tracing::warn!("{m}");
        notes.push(m.to_string());
    }

    // 1. The driver must render on the chosen adapter.
    let (adapters, monitors) = snapshot()?;
    let assigned = pipe_send(&Command::GetAssignedGpu, Duration::from_millis(1500))
        .ok()
        .and_then(|r| pipe_proto::assigned_gpu(&r.messages));
    let on_chosen_output =
        find_vdd_output(&adapters, &monitors).map(|(ai, _)| adapters[ai].info.luid == chosen.luid);
    let gpu_ok = if let Some(a) = &assigned {
        a.luid == Some(chosen.luid) || name_matches(&a.name, &chosen.name)
    } else if summary
        .gpu
        .as_deref()
        .is_some_and(|g| g != "default" && name_matches(g, &chosen.name))
    {
        true
    } else {
        on_chosen_output == Some(true)
    };
    if !gpu_ok {
        let m = format!(
            "assigning the VDD to {:?} (SETGPU) and waiting for it to restart",
            chosen.name
        );
        tracing::info!("{m}");
        notes.push(m);
        pipe_send(
            &Command::SetGpu(chosen.name.clone()),
            Duration::from_secs(5),
        )
        .map_err(|e| format!("SETGPU failed: {e}"))?;
        std::thread::sleep(Duration::from_secs(1));
        if wait_for_vdd(Duration::from_secs(20), Some(chosen.luid), false).is_none() {
            let m = "the VDD output did not come back on the chosen GPU within 20 s".to_string();
            tracing::warn!("{m}");
            notes.push(m);
        }
    }

    // 2. The mode must be offered by the driver.
    let (xml, summary) = read_settings()?;
    if !summary.supports(mode) {
        if !settings_writable() {
            return Err(format!(
                "the VDD does not offer {mode} and {} is not writable by this user. Run once, as administrator: \
                 `ezd-host vdd setup --mode {}x{}@{}` (it grants write access so later runs need no elevation).",
                settings_path().display(),
                mode.width,
                mode.height,
                vdd_xml::format_refresh(mode.refresh_mhz)
            ));
        }
        let new_xml = vdd_xml::add_mode(&xml, mode)
            .map_err(|e| e.to_string())?
            .ok_or("internal error: mode reported both missing and present")?;
        std::fs::write(settings_path(), new_xml)
            .map_err(|e| format!("cannot write {}: {e}", settings_path().display()))?;
        let m = format!("added {mode} to vdd_settings.xml and reloading the driver");
        tracing::info!("{m}");
        notes.push(m);
        pipe_send(&Command::ReloadDriver, Duration::from_secs(5))
            .map_err(|e| format!("RELOAD_DRIVER failed: {e}"))?;
        std::thread::sleep(Duration::from_secs(1));
        if wait_for_vdd(Duration::from_secs(20), Some(chosen.luid), false).is_none() {
            notes.push("the VDD output did not reappear within 20 s after RELOAD_DRIVER".into());
        }
    }

    // 3. Locate the output, attach it, set the mode.
    let (adapters, monitors) = snapshot()?;
    let Some(mon) = monitors.first().cloned() else {
        return Err(format!(
            "no virtual monitor found. The driver pipe answers but Windows lists no VDD monitor; check \
             Device Manager and `ezd-host vdd status` (<monitors><count> must be at least 1). To install the driver: {INSTALL_HINT}"
        ));
    };
    let existing = find_vdd_output(&adapters, &monitors).map(|(ai, oi)| &adapters[ai].outputs[oi]);
    let already = existing.is_some_and(|o| {
        o.attached && o.width() == mode.width as i32 && o.height() == mode.height as i32
    });
    if !already {
        let attached = existing.map_or(mon.attached, |o| o.attached);
        let x = (!attached).then(|| dxgi::desktop_right_edge(&adapters));
        let r = display::set_mode(&mon.gdi_name, mode, x)?;
        tracing::info!("{r}");
        notes.push(r);
    }
    let found = poll(Duration::from_secs(10), Duration::from_millis(300), || {
        let (adapters, monitors) = snapshot().ok()?;
        let (ai, oi) = find_vdd_output(&adapters, &monitors)?;
        let o = &adapters[ai].outputs[oi];
        (o.attached && o.width() == mode.width as i32 && o.height() == mode.height as i32)
            .then(|| (adapters[ai].clone(), o.clone()))
    });
    let Some((adapter, output)) = found else {
        let now = display::current_mode(&mon.gdi_name);
        return Err(format!(
            "the virtual monitor {} did not reach {mode} (current: {now:?})",
            mon.gdi_name
        ));
    };
    let adapter_mismatch = adapter.info.luid != chosen.luid;
    if adapter_mismatch {
        let m = format!(
            "WARNING: the VDD output {} lives on {:?} ({}) but the policy chose {:?} ({}). Capture and encode \
             will run on {:?}; frames are not copied between GPUs, but that GPU is not the preferred one. \
             Fix with `ezd-host vdd setup --gpu \"{}\"`.",
            output.device_name,
            adapter.info.name,
            adapter.info.luid_hex(),
            chosen.name,
            chosen.luid_hex(),
            adapter.info.name,
            chosen.name
        );
        tracing::warn!("{m}");
        notes.push(m);
    }
    Ok(Prepared {
        adapter,
        output,
        notes,
    })
}
