//! Small helpers shared by the Windows modules.

use serde_json::{Value, json};
use windows::Win32::Foundation::{ERROR_SUCCESS, LUID};
use windows::Win32::System::Registry::{
    HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ,
    RRF_SUBKEY_WOW6464KEY, RegGetValueW,
};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetAwarenessFromDpiAwarenessContext,
    GetThreadDpiAwarenessContext, SetProcessDpiAwarenessContext,
};
use windows::core::PCWSTR;

/// UTF-16 with a trailing NUL.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// UTF-16 up to the first NUL.
pub fn from_wide(b: &[u16]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf16_lossy(&b[..end])
}

/// LUID as `(high << 32) | low`.
pub fn luid_u64(l: LUID) -> u64 {
    ((l.HighPart as u32 as u64) << 32) | l.LowPart as u64
}

/// Formats a Windows error as `0x80070005 (message)`.
pub fn err_str(e: &windows::core::Error) -> String {
    format!("0x{:08X} ({})", e.code().0 as u32, e.message().trim())
}

/// Reads a REG_SZ / REG_EXPAND_SZ value from HKLM (64-bit view).
pub fn reg_string(subkey: &str, name: &str) -> Option<String> {
    let (sk, n) = (wide(subkey), wide(name));
    let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_SUBKEY_WOW6464KEY;
    let mut bytes = 0u32;
    // SAFETY: valid NUL-terminated strings; size query with a null data pointer.
    let r = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(sk.as_ptr()),
            PCWSTR(n.as_ptr()),
            flags,
            None,
            None,
            Some(&mut bytes),
        )
    };
    if r != ERROR_SUCCESS || bytes == 0 {
        return None;
    }
    let mut buf = vec![0u16; (bytes as usize).div_ceil(2)];
    // SAFETY: buf is at least `bytes` bytes long.
    let r = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(sk.as_ptr()),
            PCWSTR(n.as_ptr()),
            flags,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
    };
    (r == ERROR_SUCCESS).then(|| from_wide(&buf))
}

/// Reads a REG_DWORD value from HKLM (64-bit view).
pub fn reg_dword(subkey: &str, name: &str) -> Option<u32> {
    let (sk, n) = (wide(subkey), wide(name));
    let mut v = 0u32;
    let mut bytes = 4u32;
    // SAFETY: `v` is a valid 4-byte output buffer.
    let r = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(sk.as_ptr()),
            PCWSTR(n.as_ptr()),
            RRF_RT_REG_DWORD | RRF_SUBKEY_WOW6464KEY,
            None,
            Some((&mut v as *mut u32).cast()),
            Some(&mut bytes),
        )
    };
    (r == ERROR_SUCCESS).then_some(v)
}

/// Makes the process Per-Monitor DPI aware v2, so DXGI/GDI report real pixels.
/// Failure is expected when the awareness was already set (manifest or earlier call).
pub fn ensure_dpi_aware() -> String {
    // SAFETY: plain Win32 call without pointers.
    match unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) } {
        Ok(()) => "set per-monitor-v2".into(),
        Err(e) => format!("not changed: {}", err_str(&e)),
    }
}

/// Current DPI awareness of the calling thread, for the diagnostics report.
pub fn dpi_awareness() -> &'static str {
    // SAFETY: plain Win32 calls without pointers.
    let a = unsafe { GetAwarenessFromDpiAwarenessContext(GetThreadDpiAwarenessContext()) };
    match a.0 {
        0 => "unaware",
        1 => "system",
        2 => "per-monitor",
        _ => "invalid",
    }
}

/// Windows version from the registry (not subject to manifest-based version lying).
pub fn windows_version() -> Value {
    let key = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    json!({
        "product": reg_string(key, "ProductName"),
        "display_version": reg_string(key, "DisplayVersion"),
        "build": reg_string(key, "CurrentBuildNumber"),
        "ubr": reg_dword(key, "UBR"),
        "edition": reg_string(key, "EditionID"),
    })
}

/// Sleeps in small steps until `f` returns `Some` or `timeout` elapses.
pub fn poll<T>(
    timeout: std::time::Duration,
    step: std::time::Duration,
    mut f: impl FnMut() -> Option<T>,
) -> Option<T> {
    let start = std::time::Instant::now();
    loop {
        if let Some(v) = f() {
            return Some(v);
        }
        if start.elapsed() >= timeout {
            return None;
        }
        std::thread::sleep(step);
    }
}
