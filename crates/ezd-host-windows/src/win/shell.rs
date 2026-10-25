//! Message pump and elevated launches for the tray app.

use super::util::wide;
use std::path::Path;
use std::time::Duration;
use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{GetExitCodeProcess, INFINITE, WaitForSingleObject};
use windows::Win32::UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MSG, MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, PM_REMOVE,
    PeekMessageW, QS_ALLINPUT, SW_HIDE, TranslateMessage,
};
use windows::core::PCWSTR;

/// Waits up to `timeout` for window messages of the calling thread, then dispatches all that are
/// queued. A tray icon only works while its thread runs a message loop.
pub fn pump_messages(timeout: Duration) {
    let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX - 1);
    // SAFETY: no handles are passed; the call only waits for the thread's message queue.
    unsafe {
        MsgWaitForMultipleObjectsEx(None, ms, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
    }
    let mut msg = MSG::default();
    // SAFETY: `msg` is a valid output buffer; the messages are those of this thread.
    unsafe {
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Runs `exe args` elevated (UAC prompt, hidden window), waits for it and returns its exit code.
/// `Err` carries [`super::super::ELEVATION_CANCELLED`] when the user declined the prompt.
pub fn run_elevated(exe: &Path, args: &str) -> Result<u32, String> {
    let verb = wide("runas");
    let file = wide(&exe.display().to_string());
    let params = wide(args);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    // SAFETY: `info` and the UTF-16 buffers it points to outlive the call.
    if let Err(e) = unsafe { ShellExecuteExW(&mut info) } {
        return Err(if e.code() == ERROR_CANCELLED.to_hresult() {
            super::super::ELEVATION_CANCELLED.to_string()
        } else {
            format!("cannot start {}: {e}", exe.display())
        });
    }
    let process: HANDLE = info.hProcess;
    if process.is_invalid() {
        return Err("the elevated process did not start".into());
    }
    // SAFETY: `process` is a valid process handle that we own (SEE_MASK_NOCLOSEPROCESS).
    unsafe {
        let waited = WaitForSingleObject(process, INFINITE);
        let mut code = 0u32;
        let r = if waited == WAIT_OBJECT_0 {
            GetExitCodeProcess(process, &mut code)
                .map(|()| code)
                .map_err(|e| format!("cannot read the exit code: {e}"))
        } else {
            Err("waiting for the elevated process failed".to_string())
        };
        let _ = CloseHandle(process);
        r
    }
}
