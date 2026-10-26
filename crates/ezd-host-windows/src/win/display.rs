//! Display devices (GDI / CCD): finding the VDD monitor and setting its mode.

use super::util::{from_wide, luid_u64, wide};
use crate::display_topology::{PathKey, plan_extend, source_is_cloned};
use crate::ident::looks_like_vdd;
use crate::vdd_xml::{Mode, format_refresh};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL, DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
    DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME, DISPLAYCONFIG_DEVICE_INFO_HEADER,
    DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SDR_WHITE_LEVEL,
    DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TARGET_DEVICE_NAME, DisplayConfigGetDeviceInfo,
    GetDisplayConfigBufferSizes, QDC_ALL_PATHS, QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
    SDC_ALLOW_PATH_ORDER_CHANGES, SDC_APPLY, SDC_TOPOLOGY_EXTEND, SDC_TOPOLOGY_SUPPLIED,
    SetDisplayConfig,
};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::Graphics::Gdi::{
    CDS_NORESET, CDS_TYPE, CDS_UPDATEREGISTRY, ChangeDisplaySettingsExW, DEVMODEW,
    DISP_CHANGE_SUCCESSFUL, DISPLAY_DEVICEW, DM_BITSPERPEL, DM_DISPLAYFREQUENCY, DM_PELSHEIGHT,
    DM_PELSWIDTH, DM_POSITION, ENUM_CURRENT_SETTINGS, ENUM_DISPLAY_SETTINGS_FLAGS,
    ENUM_DISPLAY_SETTINGS_MODE, EnumDisplayDevicesW, EnumDisplaySettingsExW,
};
use windows::core::PCWSTR;

const DISPLAY_DEVICE_ATTACHED_TO_DESKTOP: u32 = 1;
const DISPLAY_DEVICE_PRIMARY_DEVICE: u32 = 4;
const DISPLAYCONFIG_PATH_ACTIVE: u32 = 1;
const DISPLAYCONFIG_PATH_MODE_IDX_INVALID: u32 = u32::MAX;

/// A monitor we believe is the VDD.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VddMonitor {
    /// GDI device name (`\\.\DISPLAYn`).
    pub gdi_name: String,
    /// How it was identified.
    pub how: String,
    /// Part of the desktop.
    pub attached: bool,
    /// The GDI source also drives a monitor that is not the VDD (Windows is in Duplicate mode), so
    /// it is the other monitor's output and must not be treated as the VDD's.
    pub cloned: bool,
}

/// One `EnumDisplayDevicesW` entry (adapter level) with its monitors.
#[derive(Debug, Clone)]
pub struct DisplayDevice {
    pub name: String,
    pub description: String,
    pub device_id: String,
    pub state_flags: u32,
    /// `(device id, description)` of the monitors attached to this device.
    pub monitors: Vec<(String, String)>,
}

fn enum_device(parent: PCWSTR, i: u32) -> Option<DISPLAY_DEVICEW> {
    let mut d = DISPLAY_DEVICEW {
        cb: size_of::<DISPLAY_DEVICEW>() as u32,
        ..Default::default()
    };
    // SAFETY: `d` is a correctly sized, initialised out-parameter.
    unsafe { EnumDisplayDevicesW(parent, i, &mut d, 0) }
        .as_bool()
        .then_some(d)
}

/// All GDI display devices, including ones not attached to the desktop.
pub fn display_devices() -> Vec<DisplayDevice> {
    let mut out = Vec::new();
    for i in 0..64 {
        let Some(d) = enum_device(PCWSTR::null(), i) else {
            break;
        };
        let name = from_wide(&d.DeviceName);
        let wname = wide(&name);
        let mut monitors = Vec::new();
        for j in 0..16 {
            let Some(m) = enum_device(PCWSTR(wname.as_ptr()), j) else {
                break;
            };
            monitors.push((from_wide(&m.DeviceID), from_wide(&m.DeviceString)));
        }
        out.push(DisplayDevice {
            name,
            description: from_wide(&d.DeviceString),
            device_id: from_wide(&d.DeviceID),
            state_flags: d.StateFlags.0,
            monitors,
        });
    }
    out
}

/// A target of the display configuration (CCD) with its GDI source name.
#[derive(Debug, Clone)]
pub struct CcdTarget {
    pub friendly_name: String,
    pub device_path: String,
    pub gdi_name: Option<String>,
    pub active: bool,
    pub target_available: bool,
}

/// All paths of the display configuration (`QDC_ALL_PATHS`).
pub fn ccd_targets() -> Result<Vec<CcdTarget>, String> {
    let (mut np, mut nm) = (0u32, 0u32);
    // SAFETY: valid out-pointers.
    let r = unsafe { GetDisplayConfigBufferSizes(QDC_ALL_PATHS, &mut np, &mut nm) };
    if r != ERROR_SUCCESS {
        return Err(format!("GetDisplayConfigBufferSizes: error {}", r.0));
    }
    let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
    let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
    // SAFETY: arrays sized as reported above.
    let r = unsafe {
        QueryDisplayConfig(
            QDC_ALL_PATHS,
            &mut np,
            paths.as_mut_ptr(),
            &mut nm,
            modes.as_mut_ptr(),
            None,
        )
    };
    if r != ERROR_SUCCESS {
        return Err(format!("QueryDisplayConfig: error {}", r.0));
    }
    let mut out = Vec::new();
    for p in &paths[..np as usize] {
        let mut t = DISPLAYCONFIG_TARGET_DEVICE_NAME {
            header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
                size: size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32,
                adapterId: p.targetInfo.adapterId,
                id: p.targetInfo.id,
            },
            ..Default::default()
        };
        // SAFETY: the header is the first field of the (larger) request structure.
        let rc = unsafe {
            DisplayConfigGetDeviceInfo(
                (&mut t as *mut DISPLAYCONFIG_TARGET_DEVICE_NAME)
                    .cast::<DISPLAYCONFIG_DEVICE_INFO_HEADER>(),
            )
        };
        if rc != 0 {
            continue;
        }
        let mut s = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
            header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                size: size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                adapterId: p.sourceInfo.adapterId,
                id: p.sourceInfo.id,
            },
            ..Default::default()
        };
        // SAFETY: as above.
        let rs = unsafe {
            DisplayConfigGetDeviceInfo(
                (&mut s as *mut DISPLAYCONFIG_SOURCE_DEVICE_NAME)
                    .cast::<DISPLAYCONFIG_DEVICE_INFO_HEADER>(),
            )
        };
        out.push(CcdTarget {
            friendly_name: from_wide(&t.monitorFriendlyDeviceName),
            device_path: from_wide(&t.monitorDevicePath),
            gdi_name: (rs == 0).then(|| from_wide(&s.viewGdiDeviceName)),
            active: p.flags & DISPLAYCONFIG_PATH_ACTIVE != 0,
            target_available: p.targetInfo.targetAvailable.as_bool(),
        });
    }
    Ok(out)
}

/// Raw `SDRWhiteLevel` (1000 = 80 cd/m2) of every active display, keyed by its GDI device name
/// (`\\.\DISPLAYn`, the DXGI output's `DeviceName`). Displays Windows does not answer for are
/// left out.
pub fn sdr_white_levels() -> Vec<(String, u32)> {
    let (mut np, mut nm) = (0u32, 0u32);
    // SAFETY: valid out-pointers.
    if unsafe { GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm) }
        != ERROR_SUCCESS
    {
        return Vec::new();
    }
    let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
    let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
    // SAFETY: arrays sized as reported above.
    let r = unsafe {
        QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut np,
            paths.as_mut_ptr(),
            &mut nm,
            modes.as_mut_ptr(),
            None,
        )
    };
    if r != ERROR_SUCCESS {
        return Vec::new();
    }
    let mut out = Vec::new();
    for p in &paths[..np as usize] {
        let mut s = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
            header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                size: size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                adapterId: p.sourceInfo.adapterId,
                id: p.sourceInfo.id,
            },
            ..Default::default()
        };
        // SAFETY: the header is the first field of the (larger) request structure.
        let rs = unsafe {
            DisplayConfigGetDeviceInfo(
                (&mut s as *mut DISPLAYCONFIG_SOURCE_DEVICE_NAME)
                    .cast::<DISPLAYCONFIG_DEVICE_INFO_HEADER>(),
            )
        };
        if rs != 0 {
            continue;
        }
        let mut w = DISPLAYCONFIG_SDR_WHITE_LEVEL {
            header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL,
                size: size_of::<DISPLAYCONFIG_SDR_WHITE_LEVEL>() as u32,
                adapterId: p.targetInfo.adapterId,
                id: p.targetInfo.id,
            },
            SDRWhiteLevel: 0,
        };
        // SAFETY: as above.
        let rw = unsafe {
            DisplayConfigGetDeviceInfo(
                (&mut w as *mut DISPLAYCONFIG_SDR_WHITE_LEVEL)
                    .cast::<DISPLAYCONFIG_DEVICE_INFO_HEADER>(),
            )
        };
        if rw == 0 {
            out.push((from_wide(&s.viewGdiDeviceName), w.SDRWhiteLevel));
        }
    }
    out
}

/// SDR white level in cd/m2 of the display `gdi_name` (80 when Windows does not say).
pub fn sdr_white_nits_of(levels: &[(String, u32)], gdi_name: &str) -> f32 {
    crate::hdr_math::sdr_white_nits(
        levels
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(gdi_name))
            .map_or(0, |(_, l)| *l),
    )
}

/// Monitors that look like the VDD, best candidates first (not cloned before cloned, attached
/// before not).
pub fn find_vdd_monitors() -> Vec<VddMonitor> {
    let mut found: Vec<VddMonitor> = Vec::new();
    let mut add = |name: String, how: String, attached: bool, cloned: bool| {
        if let Some(m) = found
            .iter_mut()
            .find(|m| m.gdi_name.eq_ignore_ascii_case(&name))
        {
            m.cloned |= cloned;
        } else {
            found.push(VddMonitor {
                gdi_name: name,
                how,
                attached,
                cloned,
            });
        }
    };
    for d in display_devices() {
        let attached = d.state_flags & DISPLAY_DEVICE_ATTACHED_TO_DESKTOP != 0;
        let flags: Vec<bool> = d
            .monitors
            .iter()
            .map(|(id, desc)| looks_like_vdd(id) || looks_like_vdd(desc))
            .collect();
        let cloned = source_is_cloned(&flags);
        if looks_like_vdd(&d.description) || looks_like_vdd(&d.device_id) {
            add(
                d.name.clone(),
                format!("display device {:?}", d.description),
                attached,
                cloned,
            );
        }
        for (id, desc) in &d.monitors {
            if looks_like_vdd(id) || looks_like_vdd(desc) {
                add(
                    d.name.clone(),
                    format!("monitor {desc:?} ({id})"),
                    attached,
                    cloned,
                );
            }
        }
    }
    if let Ok(targets) = ccd_targets() {
        let is_vdd =
            |t: &CcdTarget| looks_like_vdd(&t.friendly_name) || looks_like_vdd(&t.device_path);
        for t in &targets {
            let Some(g) = &t.gdi_name else { continue };
            if is_vdd(t) && t.target_available {
                // A source that an active, non-VDD target also uses is shared, whether or not this
                // (possibly inactive) VDD path is the one in use.
                let mut flags = vec![true];
                flags.extend(
                    targets
                        .iter()
                        .filter(|o| o.active && !is_vdd(o))
                        .filter_map(|o| {
                            o.gdi_name
                                .as_deref()
                                .is_some_and(|og| og.eq_ignore_ascii_case(g))
                                .then_some(false)
                        }),
                );
                add(
                    g.clone(),
                    format!("display config target {:?}", t.friendly_name),
                    t.active,
                    source_is_cloned(&flags),
                );
            }
        }
    }
    found.sort_by_key(|m| (m.cloned, !m.attached));
    found
}

/// Friendly name and device path of the target of a CCD path.
fn target_names(p: &DISPLAYCONFIG_PATH_INFO) -> Option<(String, String)> {
    let mut t = DISPLAYCONFIG_TARGET_DEVICE_NAME {
        header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
            size: size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32,
            adapterId: p.targetInfo.adapterId,
            id: p.targetInfo.id,
        },
        ..Default::default()
    };
    // SAFETY: the header is the first field of the (larger) request structure.
    let rc = unsafe {
        DisplayConfigGetDeviceInfo(
            (&mut t as *mut DISPLAYCONFIG_TARGET_DEVICE_NAME)
                .cast::<DISPLAYCONFIG_DEVICE_INFO_HEADER>(),
        )
    };
    (rc == 0).then(|| {
        (
            from_wide(&t.monitorFriendlyDeviceName),
            from_wide(&t.monitorDevicePath),
        )
    })
}

/// Takes the VDD out of Duplicate mode: moves its path to a source no other monitor uses (the
/// "Extend" topology), keeping every other active path as is. Falls back to Windows' own
/// Extend (Win+P) when no such path exists or applying it fails. `Ok(None)`: no active VDD path
/// shares its source, so nothing is changed (inactive paths pairing the VDD with another source
/// are always listed and do not count).
pub fn extend_cloned_vdd() -> Result<Option<String>, String> {
    let (mut np, mut nm) = (0u32, 0u32);
    // SAFETY: valid out-pointers.
    let r = unsafe { GetDisplayConfigBufferSizes(QDC_ALL_PATHS, &mut np, &mut nm) };
    if r != ERROR_SUCCESS {
        return Err(format!("GetDisplayConfigBufferSizes: error {}", r.0));
    }
    let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
    let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
    // SAFETY: arrays sized as reported above.
    let r = unsafe {
        QueryDisplayConfig(
            QDC_ALL_PATHS,
            &mut np,
            paths.as_mut_ptr(),
            &mut nm,
            modes.as_mut_ptr(),
            None,
        )
    };
    if r != ERROR_SUCCESS {
        return Err(format!("QueryDisplayConfig: error {}", r.0));
    }
    paths.truncate(np as usize);
    let keys: Vec<PathKey> = paths
        .iter()
        .map(|p| PathKey {
            source: (luid_u64(p.sourceInfo.adapterId), p.sourceInfo.id),
            target: (luid_u64(p.targetInfo.adapterId), p.targetInfo.id),
            active: p.flags & DISPLAYCONFIG_PATH_ACTIVE != 0,
            target_is_vdd: target_names(p)
                .is_some_and(|(n, d)| looks_like_vdd(&n) || looks_like_vdd(&d)),
        })
        .collect();
    let plan = plan_extend(&keys);
    if plan.replacements.is_empty() && !plan.unresolved {
        return Ok(None);
    }

    let mut applied = None;
    if !plan.replacements.is_empty() {
        let mut topology: Vec<DISPLAYCONFIG_PATH_INFO> = Vec::new();
        for (i, p) in paths.iter().enumerate() {
            if !keys[i].active {
                continue;
            }
            let mut q = plan
                .replacements
                .iter()
                .find(|(from, _)| *from == i)
                .map_or(*p, |&(_, to)| paths[to]);
            q.flags |= DISPLAYCONFIG_PATH_ACTIVE;
            q.sourceInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
            q.targetInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
            topology.push(q);
        }
        // SAFETY: `topology` is a valid path array; no mode array is allowed with SDC_TOPOLOGY_SUPPLIED.
        let rc = unsafe {
            SetDisplayConfig(
                Some(&topology),
                None,
                SDC_APPLY | SDC_TOPOLOGY_SUPPLIED | SDC_ALLOW_PATH_ORDER_CHANGES,
            )
        };
        if rc == 0 {
            applied = Some(format!(
                "Windows was duplicating the virtual monitor; moved it to its own display source ({} path{})",
                plan.replacements.len(),
                if plan.replacements.len() == 1 {
                    ""
                } else {
                    "s"
                }
            ));
        } else {
            tracing::warn!("SetDisplayConfig with the VDD on its own source failed: error {rc}");
        }
    }
    if let Some(note) = applied {
        return Ok(Some(note));
    }
    let why = if plan.replacements.is_empty() {
        "no free display source for the virtual monitor"
    } else {
        "applying the VDD's own display source failed"
    };
    tracing::warn!("{why}; asking Windows for the Extend topology instead");
    // SAFETY: no arrays are passed with SDC_TOPOLOGY_EXTEND.
    let rc = unsafe { SetDisplayConfig(None, None, SDC_APPLY | SDC_TOPOLOGY_EXTEND) };
    if rc == 0 {
        Ok(Some(format!(
            "Windows was duplicating the virtual monitor ({why}); switched to the Extend topology"
        )))
    } else {
        Err(format!(
            "{why}, and SetDisplayConfig(SDC_TOPOLOGY_EXTEND) failed with error {rc}"
        ))
    }
}

/// Available `(width, height, frequency)` modes of a GDI device.
pub fn device_modes(gdi_name: &str) -> Vec<(u32, u32, u32)> {
    let w = wide(gdi_name);
    let mut out = Vec::new();
    for i in 0..4096 {
        let mut dm = DEVMODEW {
            dmSize: size_of::<DEVMODEW>() as u16,
            ..Default::default()
        };
        // SAFETY: `dm` is an initialised DEVMODEW with dmSize set.
        let ok = unsafe {
            EnumDisplaySettingsExW(
                PCWSTR(w.as_ptr()),
                ENUM_DISPLAY_SETTINGS_MODE(i),
                &mut dm,
                ENUM_DISPLAY_SETTINGS_FLAGS(0),
            )
        };
        if !ok.as_bool() {
            break;
        }
        let m = (dm.dmPelsWidth, dm.dmPelsHeight, dm.dmDisplayFrequency);
        if !out.contains(&m) {
            out.push(m);
        }
    }
    out
}

/// Current mode of a GDI device, if it is active.
pub fn current_mode(gdi_name: &str) -> Option<(u32, u32, u32, i32, i32)> {
    let w = wide(gdi_name);
    let mut dm = DEVMODEW {
        dmSize: size_of::<DEVMODEW>() as u16,
        ..Default::default()
    };
    // SAFETY: `dm` is an initialised DEVMODEW with dmSize set.
    let ok = unsafe {
        EnumDisplaySettingsExW(
            PCWSTR(w.as_ptr()),
            ENUM_CURRENT_SETTINGS,
            &mut dm,
            ENUM_DISPLAY_SETTINGS_FLAGS(0),
        )
    };
    if !ok.as_bool() {
        return None;
    }
    // SAFETY: the position variant of the union is valid when DM_POSITION is set; otherwise
    // the values are merely meaningless, not undefined for plain integers.
    let pos = unsafe { dm.Anonymous1.Anonymous2.dmPosition };
    Some((
        dm.dmPelsWidth,
        dm.dmPelsHeight,
        dm.dmDisplayFrequency,
        pos.x,
        pos.y,
    ))
}

/// Sets `mode` on `gdi_name`. With `attach_at = Some((x, y))` the monitor is also
/// placed at `(x, y)`, which attaches it to the desktop in extend mode.
pub fn set_mode(
    gdi_name: &str,
    mode: Mode,
    attach_at: Option<(i32, i32)>,
) -> Result<String, String> {
    let want_hz = (mode.refresh_mhz as f64 / 1000.0).round() as u32;
    let modes = device_modes(gdi_name);
    let best = modes
        .iter()
        .filter(|m| m.0 == mode.width && m.1 == mode.height)
        .min_by_key(|m| m.2.abs_diff(want_hz))
        .copied();
    let Some((w, h, hz)) = best else {
        let mut avail: Vec<String> = modes
            .iter()
            .map(|m| format!("{}x{}@{}", m.0, m.1, m.2))
            .collect();
        avail.truncate(24);
        return Err(format!(
            "{gdi_name} offers no {}x{} mode (driver modes seen: {}). The VDD has not loaded the new \
             resolution yet; reload the driver or run `ezd-host vdd setup --mode {}x{}@{}`.",
            mode.width,
            mode.height,
            if avail.is_empty() {
                "none".to_string()
            } else {
                avail.join(", ")
            },
            mode.width,
            mode.height,
            format_refresh(mode.refresh_mhz)
        ));
    };
    let mut dm = DEVMODEW {
        dmSize: size_of::<DEVMODEW>() as u16,
        dmPelsWidth: w,
        dmPelsHeight: h,
        dmDisplayFrequency: hz,
        dmBitsPerPel: 32,
        dmFields: DM_PELSWIDTH | DM_PELSHEIGHT | DM_DISPLAYFREQUENCY | DM_BITSPERPEL,
        ..Default::default()
    };
    if let Some((x, y)) = attach_at {
        dm.dmFields |= DM_POSITION;
        dm.Anonymous1.Anonymous2.dmPosition.x = x;
        dm.Anonymous1.Anonymous2.dmPosition.y = y;
    }
    let name = wide(gdi_name);
    // SAFETY: `dm` is a fully initialised DEVMODEW; `name` is NUL-terminated.
    let r = unsafe {
        ChangeDisplaySettingsExW(
            PCWSTR(name.as_ptr()),
            Some(&dm),
            None,
            CDS_UPDATEREGISTRY | CDS_NORESET,
            None,
        )
    };
    if r != DISP_CHANGE_SUCCESSFUL {
        return Err(format!(
            "ChangeDisplaySettingsExW({gdi_name}, {w}x{h}@{hz}) failed: DISP_CHANGE {}",
            r.0
        ));
    }
    // SAFETY: a null device and mode apply the pending registry changes.
    let r = unsafe { ChangeDisplaySettingsExW(PCWSTR::null(), None, None, CDS_TYPE(0), None) };
    if r != DISP_CHANGE_SUCCESSFUL {
        return Err(format!(
            "applying display settings failed: DISP_CHANGE {}",
            r.0
        ));
    }
    Ok(format!(
        "{gdi_name} set to {w}x{h}@{hz}{}",
        attach_at.map_or(String::new(), |(x, y)| format!(" at ({x},{y})"))
    ))
}

/// Takes `gdi_name` off the desktop (a zero-sized mode at the origin), as unplugging the monitor
/// would: Windows moves its windows to the remaining monitors. `set_mode` with `attach_at`
/// attaches it again; [`detached_position`] tells where it was.
pub fn detach(gdi_name: &str) -> Result<String, String> {
    if let Some((_, _, _, x, y)) = current_mode(gdi_name) {
        detached_positions()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(gdi_name.to_ascii_uppercase(), (x, y));
    }
    let dm = DEVMODEW {
        dmSize: size_of::<DEVMODEW>() as u16,
        dmFields: DM_PELSWIDTH | DM_PELSHEIGHT | DM_POSITION,
        ..Default::default()
    };
    let name = wide(gdi_name);
    // SAFETY: `dm` is a fully initialised DEVMODEW; `name` is NUL-terminated.
    let r = unsafe {
        ChangeDisplaySettingsExW(
            PCWSTR(name.as_ptr()),
            Some(&dm),
            None,
            CDS_UPDATEREGISTRY | CDS_NORESET,
            None,
        )
    };
    if r != DISP_CHANGE_SUCCESSFUL {
        return Err(format!(
            "ChangeDisplaySettingsExW({gdi_name}, detach) failed: DISP_CHANGE {}",
            r.0
        ));
    }
    // SAFETY: a null device and mode apply the pending registry changes.
    let r = unsafe { ChangeDisplaySettingsExW(PCWSTR::null(), None, None, CDS_TYPE(0), None) };
    if r != DISP_CHANGE_SUCCESSFUL {
        return Err(format!(
            "applying display settings failed: DISP_CHANGE {}",
            r.0
        ));
    }
    Ok(format!("{gdi_name} detached from the desktop"))
}

fn detached_positions() -> &'static Mutex<HashMap<String, (i32, i32)>> {
    static POSITIONS: OnceLock<Mutex<HashMap<String, (i32, i32)>>> = OnceLock::new();
    POSITIONS.get_or_init(Default::default)
}

/// Where `gdi_name` was on the desktop when this process last detached it.
pub fn detached_position(gdi_name: &str) -> Option<(i32, i32)> {
    detached_positions()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&gdi_name.to_ascii_uppercase())
        .copied()
}

/// JSON of all display devices and CCD targets, for `diag`.
pub fn diag_json() -> Value {
    let devices: Vec<Value> = display_devices()
        .into_iter()
        .map(|d| {
            json!({
                "name": d.name,
                "description": d.description,
                "device_id": d.device_id,
                "attached_to_desktop": d.state_flags & DISPLAY_DEVICE_ATTACHED_TO_DESKTOP != 0,
                "primary": d.state_flags & DISPLAY_DEVICE_PRIMARY_DEVICE != 0,
                "monitors": d.monitors.iter().map(|(id, s)| json!({"id": id, "description": s})).collect::<Vec<_>>(),
            })
        })
        .collect();
    let targets = match ccd_targets() {
        Ok(t) => t
            .into_iter()
            .map(|t| {
                json!({
                    "friendly_name": t.friendly_name,
                    "device_path": t.device_path,
                    "gdi_name": t.gdi_name,
                    "active": t.active,
                    "target_available": t.target_available,
                    "looks_like_vdd": looks_like_vdd(&t.friendly_name) || looks_like_vdd(&t.device_path),
                })
            })
            .collect::<Vec<_>>(),
        Err(e) => vec![json!({ "error": e })],
    };
    json!({
        "devices": devices,
        "ccd_targets": targets,
        "vdd_candidates": find_vdd_monitors().into_iter().map(|m| json!({"gdi_name": m.gdi_name, "how": m.how, "attached": m.attached, "cloned": m.cloned})).collect::<Vec<_>>(),
    })
}
