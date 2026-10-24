//! Installs and removes the Virtual Display Driver for the ezdisplay installer: root device
//! node and driver through `nefconw`, our own config folder and the ownership record that keeps
//! us from ever touching a VDD somebody else installed.

use super::util::{err_str, reg_dword, reg_string, wide};
use super::vdd::{self, REG_KEY, is_elevated};
use crate::vdd_install::{
    HARDWARE_ID, INF_NAME, InstallError, InstallOutcome, NefconStatus, UninstallOutcome,
    has_hardware_id, install_args, map_exit_code, parse_multi_sz, remove_device_args,
    remove_package_args,
};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    GUID_DEVCLASS_DISPLAY, SETUP_DI_GET_CLASS_DEVS_FLAGS, SP_DEVINFO_DATA, SPDRP_HARDWAREID,
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo, SetupDiGetClassDevsW,
    SetupDiGetDeviceRegistryPropertyW,
};
use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS};
use windows::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE, KEY_SET_VALUE, KEY_WOW64_64KEY, REG_DWORD,
    REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteKeyW, RegDeleteValueW,
    RegOpenKeyExW, RegQueryInfoKeyW, RegSetValueExW,
};
use windows::core::PCWSTR;

/// State key (HKLM, 64-bit view): `VddOwned` (DWORD), `VddDir` (SZ, our config folder) and
/// `DriverDir` (SZ, where `nefconw.exe` and the driver package were installed from).
const STATE_KEY: &str = r"SOFTWARE\ezdisplay";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn fail<T>(msg: impl Into<String>) -> Result<T, InstallError> {
    Err(InstallError::Failed(msg.into()))
}

/// `Some(true)` when ezdisplay installed the VDD, `Some(false)` when it found someone else's,
/// `None` when no state was recorded.
pub fn owned() -> Option<bool> {
    reg_dword(STATE_KEY, "VddOwned").map(|v| v == 1)
}

/// Driver folder recorded by `install`.
pub fn stored_driver_dir() -> Option<String> {
    reg_string(STATE_KEY, "DriverDir")
}

// ------------------------------------------------------------------ registry writes

struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: we own the open key.
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

fn open_key(subkey: &str, create: bool, access: u32) -> Result<Option<Key>, String> {
    let sk = wide(subkey);
    let mut h = HKEY::default();
    let sam = windows::Win32::System::Registry::REG_SAM_FLAGS(access | KEY_WOW64_64KEY.0);
    // SAFETY: NUL-terminated name and a valid out-pointer; the key is closed by `Key`.
    let r = unsafe {
        if create {
            RegCreateKeyExW(
                HKEY_LOCAL_MACHINE,
                PCWSTR(sk.as_ptr()),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                sam,
                None,
                &mut h,
                None,
            )
        } else {
            RegOpenKeyExW(HKEY_LOCAL_MACHINE, PCWSTR(sk.as_ptr()), None, sam, &mut h)
        }
    };
    match r {
        ERROR_SUCCESS => Ok(Some(Key(h))),
        e if !create && e.0 == 2 => Ok(None), // ERROR_FILE_NOT_FOUND
        e => Err(format!("registry key HKLM\\{subkey}: error {}", e.0)),
    }
}

fn set_value(
    subkey: &str,
    name: &str,
    ty: windows::Win32::System::Registry::REG_VALUE_TYPE,
    data: &[u8],
) -> Result<(), String> {
    let key = open_key(subkey, true, KEY_SET_VALUE.0)?.ok_or("registry key missing")?;
    let n = wide(name);
    // SAFETY: valid key and NUL-terminated name; `data` is the raw value of type `ty`.
    let r = unsafe { RegSetValueExW(key.0, PCWSTR(n.as_ptr()), None, ty, Some(data)) };
    if r == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(format!("cannot write registry value {name}: error {}", r.0))
    }
}

fn set_dword(subkey: &str, name: &str, v: u32) -> Result<(), String> {
    set_value(subkey, name, REG_DWORD, &v.to_le_bytes())
}

fn set_sz(subkey: &str, name: &str, v: &str) -> Result<(), String> {
    let bytes: Vec<u8> = wide(v).iter().flat_map(|c| c.to_le_bytes()).collect();
    set_value(subkey, name, REG_SZ, &bytes)
}

/// Deletes a value; a missing key or value is fine.
fn delete_value(subkey: &str, name: &str) -> Result<(), String> {
    let Some(key) = open_key(subkey, false, KEY_SET_VALUE.0)? else {
        return Ok(());
    };
    let n = wide(name);
    // SAFETY: valid key and NUL-terminated name.
    let r = unsafe { RegDeleteValueW(key.0, PCWSTR(n.as_ptr())) };
    if r == ERROR_SUCCESS || r.0 == 2 {
        Ok(())
    } else {
        Err(format!(
            "cannot delete registry value {name}: error {}",
            r.0
        ))
    }
}

/// Deletes the key when it has no values and no subkeys.
fn delete_key_if_empty(subkey: &str) -> Result<(), String> {
    let Some(key) = open_key(subkey, false, KEY_QUERY_VALUE.0)? else {
        return Ok(());
    };
    let (mut subkeys, mut values) = (0u32, 0u32);
    // SAFETY: valid key; only the two count out-pointers are requested.
    let r = unsafe {
        RegQueryInfoKeyW(
            key.0,
            None,
            None,
            None,
            Some(&mut subkeys),
            None,
            None,
            Some(&mut values),
            None,
            None,
            None,
            None,
        )
    };
    drop(key);
    if r != ERROR_SUCCESS || subkeys != 0 || values != 0 {
        return Ok(());
    }
    let sk = wide(subkey);
    // SAFETY: NUL-terminated name. Not the WOW64 variant: the key lives in the 64-bit view
    // and this process is 64-bit.
    let _ = unsafe { RegDeleteKeyW(HKEY_LOCAL_MACHINE, PCWSTR(sk.as_ptr())) };
    Ok(())
}

// ------------------------------------------------------------------------ device node

/// Whether a root device node with the VDD hardware ID exists, present or not.
fn node_exists() -> Result<bool, String> {
    // SAFETY: class GUID constant; no enumerator, parent window or flags (all devices of the
    // class, including ones that are not currently present).
    let set = unsafe {
        SetupDiGetClassDevsW(
            Some(&GUID_DEVCLASS_DISPLAY),
            PCWSTR::null(),
            None,
            SETUP_DI_GET_CLASS_DEVS_FLAGS(0),
        )
    }
    .map_err(|e| format!("SetupDiGetClassDevsW: {}", err_str(&e)))?;
    let mut found = false;
    let mut index = 0u32;
    loop {
        let mut data = SP_DEVINFO_DATA {
            cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
            ..Default::default()
        };
        // SAFETY: valid device set and a SP_DEVINFO_DATA with cbSize set.
        if unsafe { SetupDiEnumDeviceInfo(set, index, &mut data) }.is_err() {
            break; // ERROR_NO_MORE_ITEMS (or a failure: nothing more to see either way)
        }
        index += 1;
        let mut buf = vec![0u8; 512];
        loop {
            let mut required = 0u32;
            // SAFETY: valid set and element; `buf` is the output buffer.
            let r = unsafe {
                SetupDiGetDeviceRegistryPropertyW(
                    set,
                    &data,
                    SPDRP_HARDWAREID,
                    None,
                    Some(&mut buf),
                    Some(&mut required),
                )
            };
            match r {
                Ok(()) => {
                    let units: Vec<u16> = buf
                        .chunks_exact(2)
                        .map(|c| u16::from_le_bytes([c[0], c[1]]))
                        .collect();
                    if has_hardware_id(&parse_multi_sz(&units), HARDWARE_ID) {
                        found = true;
                    }
                    break;
                }
                Err(e)
                    if e.code() == ERROR_INSUFFICIENT_BUFFER.to_hresult()
                        && required as usize > buf.len() =>
                {
                    buf.resize(required as usize, 0);
                }
                Err(_) => break, // no hardware ID property on this node
            }
        }
        if found {
            break;
        }
    }
    // SAFETY: the set was created above and is not used afterwards.
    let _ = unsafe { SetupDiDestroyDeviceInfoList(set) };
    Ok(found)
}

// ---------------------------------------------------------------------------- paths

fn program_data() -> Result<PathBuf, InstallError> {
    match std::env::var_os("ProgramData").filter(|v| !v.is_empty()) {
        Some(p) => Ok(PathBuf::from(p)),
        None => fail("the ProgramData environment variable is not set"),
    }
}

fn vdd_dir() -> Result<PathBuf, InstallError> {
    Ok(program_data()?.join("ezdisplay").join("vdd"))
}

fn log_file(name: &str) -> Result<PathBuf, InstallError> {
    let dir = program_data()?.join("ezdisplay").join("logs");
    std::fs::create_dir_all(&dir)
        .map_err(|e| InstallError::Failed(format!("cannot create {}: {e}", dir.display())))?;
    Ok(dir.join(name))
}

/// Runs `nefconw.exe` from `driver_dir` and maps its exit code.
fn run_nefcon(driver_dir: &Path, args: &[String]) -> Result<NefconStatus, InstallError> {
    let exe = driver_dir.join("nefconw.exe");
    if !exe.is_file() {
        return fail(format!("{} not found", exe.display()));
    }
    let status = std::process::Command::new(&exe)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map_err(|e| InstallError::Failed(format!("cannot run {}: {e}", exe.display())))?;
    let Some(code) = status.code() else {
        return fail(format!("{} ended without an exit code", exe.display()));
    };
    map_exit_code(code).map_err(|c| {
        InstallError::Failed(format!(
            "{} {} failed with exit code {c} (see the log in %ProgramData%\\ezdisplay\\logs)",
            exe.display(),
            args.first().map_or("", String::as_str)
        ))
    })
}

fn reboot(s: NefconStatus) -> bool {
    s == NefconStatus::RebootRequired
}

fn require_elevation() -> Result<(), InstallError> {
    if is_elevated() {
        Ok(())
    } else {
        Err(InstallError::NotElevated)
    }
}

// ------------------------------------------------------------------- install/uninstall

/// Installs the VDD from `driver_dir` (`MttVDD.inf` and `nefconw.exe` live there).
pub fn install(driver_dir: &Path) -> Result<InstallOutcome, InstallError> {
    require_elevation()?;
    let inf = driver_dir.join(INF_NAME);
    if !inf.is_file() {
        return fail(format!("{} not found", inf.display()));
    }
    let exists = node_exists()?;
    if exists && owned() != Some(true) {
        set_dword(STATE_KEY, "VddOwned", 0)?;
        return Ok(InstallOutcome::PreexistingLeftAlone);
    }

    let dir = vdd_dir()?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| InstallError::Failed(format!("cannot create {}: {e}", dir.display())))?;
    // Keep an edited settings file across upgrades.
    let settings = dir.join("vdd_settings.xml");
    if !settings.exists() {
        let src = driver_dir.join("vdd_settings.xml");
        std::fs::copy(&src, &settings).map_err(|e| {
            InstallError::Failed(format!(
                "cannot copy {} to {}: {e}",
                src.display(),
                settings.display()
            ))
        })?;
    }
    vdd::grant_users_modify(&settings)?;
    // Before the node exists, so the driver reads our folder when it starts.
    set_sz(REG_KEY, "VDDPATH", &dir.display().to_string())?;

    let log = log_file("vdd-install.log")?;
    let status = run_nefcon(driver_dir, &install_args(&inf, &log))?;

    set_dword(STATE_KEY, "VddOwned", 1)?;
    set_sz(STATE_KEY, "VddDir", &dir.display().to_string())?;
    set_sz(STATE_KEY, "DriverDir", &driver_dir.display().to_string())?;
    Ok(InstallOutcome::Installed {
        reboot_required: reboot(status),
    })
}

/// Removes a VDD installed by [`install`]. `driver_dir` locates `nefconw.exe`; without it the
/// folder recorded at install time is used.
pub fn uninstall(driver_dir: Option<&Path>) -> Result<UninstallOutcome, InstallError> {
    require_elevation()?;
    if owned() != Some(true) {
        return Ok(UninstallOutcome::NotOurs);
    }
    let driver_dir = match driver_dir {
        Some(d) => d.to_path_buf(),
        None => match stored_driver_dir() {
            Some(d) => PathBuf::from(d),
            None => return fail("no driver folder recorded; pass the driver folder explicitly"),
        },
    };
    let log = log_file("vdd-uninstall.log")?;
    let mut reboot_required = reboot(run_nefcon(&driver_dir, &remove_device_args(&log))?);
    reboot_required |= reboot(run_nefcon(&driver_dir, &remove_package_args(&log))?);

    let dir = vdd_dir()?;
    let dir_s = dir.display().to_string();
    if reg_string(REG_KEY, "VDDPATH").is_some_and(|p| p.trim().eq_ignore_ascii_case(&dir_s)) {
        delete_value(REG_KEY, "VDDPATH")?;
    }
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return fail(format!("cannot remove {}: {e}", dir.display())),
    }
    for v in ["VddOwned", "VddDir", "DriverDir"] {
        delete_value(STATE_KEY, v)?;
    }
    delete_key_if_empty(STATE_KEY)?;
    Ok(UninstallOutcome::Removed { reboot_required })
}
