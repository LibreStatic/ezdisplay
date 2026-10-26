//! Registry side of the touch keyboard suppression, see [`crate::touch_keyboard`].
//!
//! Everything here is best effort: a failure is logged and never breaks the session.

use super::util::{from_wide, wide};
use crate::touch_keyboard::{
    OWN_KEY, SAVED_VALUE, TABLET_TIP_KEY, TAP_INVOKE_NEVER, TAP_INVOKE_VALUE, TouchKeyboardGuard,
    decode_saved, encode_saved,
};
use windows::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, LPARAM, WIN32_ERROR, WPARAM,
};
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_DWORD, REG_SZ, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegDeleteKeyValueW,
    RegGetValueW, RegSetKeyValueW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE,
};
use windows::core::PCWSTR;

/// How long one window may take to handle the settings broadcast.
const BROADCAST_TIMEOUT_MS: u32 = 100;

fn check(r: WIN32_ERROR) -> Result<(), String> {
    if r == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(format!("registry error {}", r.0))
    }
}

/// Reads a value of the current user; `Ok(None)` if it does not exist.
fn read_dword(subkey: &str, name: &str) -> Result<Option<u32>, String> {
    let (sk, n) = (wide(subkey), wide(name));
    let (mut v, mut bytes) = (0u32, 4u32);
    // SAFETY: valid NUL-terminated strings; `v` is a valid 4-byte output buffer.
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(sk.as_ptr()),
            PCWSTR(n.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut v as *mut u32).cast()),
            Some(&mut bytes),
        )
    };
    if r == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    check(r).map(|()| Some(v))
}

fn read_string(subkey: &str, name: &str) -> Result<Option<String>, String> {
    let (sk, n) = (wide(subkey), wide(name));
    let mut buf = [0u16; 64];
    let mut bytes = std::mem::size_of_val(&buf) as u32;
    // SAFETY: valid NUL-terminated strings; `buf` is `bytes` bytes long.
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(sk.as_ptr()),
            PCWSTR(n.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
    };
    if r == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    check(r).map(|()| Some(from_wide(&buf)))
}

fn write_dword(subkey: &str, name: &str, v: u32) -> Result<(), String> {
    let (sk, n) = (wide(subkey), wide(name));
    // SAFETY: valid NUL-terminated strings; the data is a live 4-byte DWORD.
    check(unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            PCWSTR(sk.as_ptr()),
            PCWSTR(n.as_ptr()),
            REG_DWORD.0,
            Some((&v as *const u32).cast()),
            4,
        )
    })
}

fn write_string(subkey: &str, name: &str, s: &str) -> Result<(), String> {
    let (sk, n, data) = (wide(subkey), wide(name), wide(s));
    // SAFETY: valid NUL-terminated strings; `data` is `2 * len` bytes including the NUL.
    check(unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            PCWSTR(sk.as_ptr()),
            PCWSTR(n.as_ptr()),
            REG_SZ.0,
            Some(data.as_ptr().cast()),
            (data.len() * 2) as u32,
        )
    })
}

/// Deletes a value; a value that does not exist counts as deleted.
fn delete_value(subkey: &str, name: &str) -> Result<(), String> {
    let (sk, n) = (wide(subkey), wide(name));
    // SAFETY: valid NUL-terminated strings.
    let r =
        unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, PCWSTR(sk.as_ptr()), PCWSTR(n.as_ptr())) };
    if r == ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    check(r)
}

/// Tells running programs (the touch keyboard host) that `TabletTip` settings changed. Done on
/// a short-lived thread because a hung top-level window could stall the caller.
fn broadcast_change() {
    let _ = std::thread::Builder::new()
        .name("ezd-settingchange".into())
        .spawn(|| {
            let area = wide("TabletTip");
            // SAFETY: `area` outlives the synchronous call; lParam is a pointer to it as
            // WM_SETTINGCHANGE requires.
            unsafe {
                SendMessageTimeoutW(
                    HWND_BROADCAST,
                    WM_SETTINGCHANGE,
                    WPARAM(0),
                    LPARAM(area.as_ptr() as isize),
                    SMTO_ABORTIFHUNG,
                    BROADCAST_TIMEOUT_MS,
                    None,
                );
            }
        });
}

/// Writes `value` (`None` = delete) to `TouchKeyboardTapInvoke`.
fn set_tap_invoke(value: Option<u32>) -> Result<(), String> {
    match value {
        Some(v) => write_dword(TABLET_TIP_KEY, TAP_INVOKE_VALUE, v),
        None => delete_value(TABLET_TIP_KEY, TAP_INVOKE_VALUE),
    }
}

/// Restores the setting from the crash marker left by an earlier run that did not exit cleanly.
/// Call once when the input backend is created.
pub fn recover_after_crash() {
    let saved = match read_string(OWN_KEY, SAVED_VALUE) {
        Ok(Some(s)) => s,
        Ok(None) => return,
        Err(e) => {
            tracing::warn!("touch keyboard: cannot read the saved setting: {e}");
            return;
        }
    };
    match decode_saved(&saved) {
        Some(original) => match set_tap_invoke(original) {
            Ok(()) => {
                tracing::info!(
                    "touch keyboard: restored TouchKeyboardTapInvoke to {original:?} after an unclean exit"
                );
                broadcast_change();
            }
            Err(e) => {
                tracing::warn!("touch keyboard: restore after an unclean exit failed: {e}");
                return; // keep the marker for the next start
            }
        },
        None => tracing::warn!("touch keyboard: ignoring malformed saved setting {saved:?}"),
    }
    if let Err(e) = delete_value(OWN_KEY, SAVED_VALUE) {
        tracing::warn!("touch keyboard: cannot clear the saved setting: {e}");
    }
}

/// Current value of `TouchKeyboardTapInvoke` and whether our marker says we override it, for
/// `ezd-host diag`.
pub fn diag_json() -> serde_json::Value {
    use serde_json::json;
    let current = match read_dword(TABLET_TIP_KEY, TAP_INVOKE_VALUE) {
        Ok(v) => json!(v),
        Err(e) => json!(format!("unreadable: {e}")),
    };
    let overridden = matches!(read_string(OWN_KEY, SAVED_VALUE), Ok(Some(_)));
    json!({ "touch_keyboard_tap_invoke": current, "overridden": overridden })
}

/// Keeps the touch keyboard from opening while the client has a hardware keyboard.
/// Restores the setting on [`restore`](Self::restore) and on drop.
#[derive(Default)]
pub struct TouchKeyboard {
    guard: TouchKeyboardGuard,
}

impl TouchKeyboard {
    /// A suppressor that has changed nothing yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// The client's hardware keyboard state changed.
    pub fn set_attached(&mut self, attached: bool) {
        if attached {
            self.suppress();
        } else {
            self.restore();
        }
    }

    fn suppress(&mut self) {
        let current = match read_dword(TABLET_TIP_KEY, TAP_INVOKE_VALUE) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("touch keyboard: cannot read TouchKeyboardTapInvoke: {e}");
                return;
            }
        };
        let Some(original) = self.guard.attach(current) else {
            return;
        };
        // Persist the original first so a crash cannot leave the user stuck at "never".
        if let Err(e) = write_string(OWN_KEY, SAVED_VALUE, &encode_saved(original)) {
            tracing::warn!(
                "touch keyboard: cannot save the original setting, not changing it: {e}"
            );
            self.guard.detach();
            return;
        }
        match set_tap_invoke(Some(TAP_INVOKE_NEVER)) {
            Ok(()) => {
                tracing::info!(
                    "touch keyboard: hardware keyboard attached, TouchKeyboardTapInvoke {original:?} -> 0"
                );
                broadcast_change();
            }
            Err(e) => {
                tracing::warn!("touch keyboard: cannot set TouchKeyboardTapInvoke: {e}");
                self.guard.detach();
                let _ = delete_value(OWN_KEY, SAVED_VALUE);
            }
        }
    }

    /// Puts the original setting back, if we changed it.
    pub fn restore(&mut self) {
        let Some(original) = self.guard.detach() else {
            return;
        };
        match set_tap_invoke(original) {
            Ok(()) => {
                tracing::info!("touch keyboard: restored TouchKeyboardTapInvoke to {original:?}");
                if let Err(e) = delete_value(OWN_KEY, SAVED_VALUE) {
                    tracing::warn!("touch keyboard: cannot clear the saved setting: {e}");
                }
                broadcast_change();
            }
            Err(e) => {
                // The marker stays, so the next start restores it.
                tracing::warn!("touch keyboard: cannot restore TouchKeyboardTapInvoke: {e}");
            }
        }
    }
}

impl Drop for TouchKeyboard {
    fn drop(&mut self) {
        self.restore();
    }
}
