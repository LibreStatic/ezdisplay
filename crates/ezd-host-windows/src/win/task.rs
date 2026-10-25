//! The `ezdisplay` scheduled task (elevated launch without a UAC prompt) and the user helpers it
//! needs. The task is driven through `schtasks.exe`.

use super::util::from_wide;
use crate::task_xml::{self, TASK_NAME};
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows::Win32::System::RemoteDesktop::{
    WTSDomainName, WTSFreeMemory, WTSGetActiveConsoleSessionId, WTSQuerySessionInformationW,
    WTSUserName,
};
use windows::Win32::System::Threading::{CREATE_NO_WINDOW, GetCurrentProcess, OpenProcessToken};
use windows::core::PWSTR;

/// `DOMAIN\name` of the user at the console, so that the elevated installer registers the task
/// for the person who sits at the PC and not for the account that approved UAC. Falls back to
/// the user of this process.
pub fn interactive_user() -> String {
    console_user().unwrap_or_else(|| {
        let name = std::env::var("USERNAME").unwrap_or_default();
        match std::env::var("USERDOMAIN") {
            Ok(d) if !d.is_empty() => format!("{d}\\{name}"),
            _ => name,
        }
    })
}

fn console_user() -> Option<String> {
    // SAFETY: no arguments.
    let session = unsafe { WTSGetActiveConsoleSessionId() };
    if session == u32::MAX {
        return None;
    }
    let query = |class| -> Option<String> {
        let mut buf = PWSTR::null();
        let mut len = 0u32;
        // SAFETY: valid out-pointers; the buffer WTS allocates is freed below.
        unsafe {
            WTSQuerySessionInformationW(None, session, class, &mut buf, &mut len).ok()?;
            let text = from_wide(std::slice::from_raw_parts(buf.0, (len / 2) as usize));
            WTSFreeMemory(buf.0.cast());
            Some(text)
        }
    };
    let name = query(WTSUserName).filter(|n| !n.is_empty())?;
    match query(WTSDomainName) {
        Some(d) if !d.is_empty() => Some(format!("{d}\\{name}")),
        _ => Some(name),
    }
}

/// SID (`S-1-5-21-...`) of the account this process runs as.
pub fn current_user_sid() -> Option<String> {
    let mut token = HANDLE::default();
    // SAFETY: valid out-pointer; the handle is closed below.
    unsafe {
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).ok()?;
        let mut size = 0u32;
        // Fails with "insufficient buffer" and reports the size needed.
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut size);
        // u64 elements: TOKEN_USER needs pointer alignment.
        let mut buf = vec![0u64; (size as usize).div_ceil(8).max(1)];
        let sid = GetTokenInformation(
            token,
            TokenUser,
            Some(buf.as_mut_ptr().cast()),
            size,
            &mut size,
        )
        .ok()
        .and_then(|()| {
            // SAFETY (of the cast): the buffer now holds a TOKEN_USER whose SID points into it.
            let user = &*(buf.as_ptr().cast::<TOKEN_USER>());
            let mut text = PWSTR::null();
            ConvertSidToStringSidW(user.User.Sid, &mut text).ok()?;
            let s = text.to_string().ok();
            let _ = LocalFree(Some(HLOCAL(text.0.cast())));
            s
        });
        let _ = CloseHandle(token);
        sid
    }
}

fn schtasks(args: &[&str]) -> Result<String, String> {
    let sys = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    let exe = format!(r"{sys}\System32\schtasks.exe");
    let out = Command::new(&exe)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW.0)
        .output()
        .map_err(|e| format!("cannot run {exe}: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    if out.status.success() {
        Ok(stdout)
    } else {
        Err(format!(
            "schtasks {} failed ({}): {} {}",
            args.join(" "),
            out.status,
            stdout.trim(),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Creates or replaces the task. Needs elevation (a task that runs elevated cannot be registered
/// by a normal token).
pub fn register(exe: &Path, autostart: bool, elevated: bool) -> Result<(), String> {
    let xml = task_xml::build(
        &interactive_user(),
        &exe.display().to_string(),
        autostart,
        elevated,
    );
    // schtasks only reads UTF-16 task files, with a byte order mark.
    let mut bytes = vec![0xFF, 0xFE];
    for u in xml.encode_utf16() {
        bytes.extend_from_slice(&u.to_le_bytes());
    }
    let file = std::env::temp_dir().join(format!("ezdisplay-task-{}.xml", std::process::id()));
    std::fs::write(&file, bytes).map_err(|e| format!("cannot write {}: {e}", file.display()))?;
    let result = schtasks(&[
        "/create",
        "/tn",
        TASK_NAME,
        "/xml",
        &file.display().to_string(),
        "/f",
    ]);
    let _ = std::fs::remove_file(&file);
    result.map(|_| ())
}

/// Removes the task (succeeds when there is none).
pub fn delete() -> Result<(), String> {
    if !exists() {
        return Ok(());
    }
    schtasks(&["/delete", "/tn", TASK_NAME, "/f"]).map(|_| ())
}

pub fn exists() -> bool {
    schtasks(&["/query", "/tn", TASK_NAME]).is_ok()
}

/// Whether the task exists and has an enabled logon trigger.
pub fn autostart_enabled() -> bool {
    schtasks(&["/query", "/tn", TASK_NAME, "/xml"])
        .map(|xml| task_xml::has_logon_trigger(&xml))
        .unwrap_or(false)
}

/// Whether the task exists and runs with the highest privileges.
pub fn runs_elevated() -> bool {
    schtasks(&["/query", "/tn", TASK_NAME, "/xml"])
        .map(|xml| task_xml::runs_elevated(&xml))
        .unwrap_or(false)
}

/// Starts the task now (returns once the scheduler accepted it, not when the app is up).
pub fn run() -> Result<(), String> {
    schtasks(&["/run", "/tn", TASK_NAME]).map(|_| ())
}
