//! DXGI adapter and output enumeration.

use super::util::{err_str, from_wide, luid_u64};
use crate::hdr_math::OutputColor;
use crate::policy::AdapterInfo;
use serde_json::{Value, json};
use windows::Win32::Foundation::{HMODULE, RECT};
use windows::Win32::Graphics::Dxgi::Common::DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020;
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ADAPTER_DESC1, DXGI_ERROR_NOT_FOUND, DXGI_GPU_PREFERENCE,
    DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE, DXGI_GPU_PREFERENCE_MINIMUM_POWER, IDXGIAdapter1,
    IDXGIFactory1, IDXGIFactory6, IDXGIOutput, IDXGIOutput6,
};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITORINFO};

/// `MONITORINFOF_PRIMARY`.
const MONITORINFOF_PRIMARY: u32 = 1;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_SDK_VERSION,
    D3D11CreateDevice, ID3D11Device, ID3D11Multithread,
};
use windows::core::Interface;

/// One `IDXGIOutput` with the facts we need.
#[derive(Clone)]
pub struct OutputEntry {
    pub output: IDXGIOutput,
    /// GDI device name, e.g. `\\.\DISPLAY5`.
    pub device_name: String,
    pub attached: bool,
    pub rect: RECT,
    pub rotation: i32,
    pub primary: bool,
    /// Colour volume reported by `IDXGIOutput6::GetDesc1` (`None` before Windows 10 1703).
    pub color: Option<OutputColor>,
    /// SDR white level in cd/m2 (the "SDR content brightness" setting; 80 when unknown).
    pub sdr_white_nits: f32,
}

/// What `GetDesc1` says about `output`: whether Windows HDR is on and the display's colour volume.
pub fn output_color(output: &IDXGIOutput) -> Option<OutputColor> {
    let o6 = output.cast::<IDXGIOutput6>().ok()?;
    // SAFETY: plain COM call returning a value.
    let d = unsafe { o6.GetDesc1() }.ok()?;
    Some(OutputColor {
        hdr: d.ColorSpace == DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020,
        red: d.RedPrimary,
        green: d.GreenPrimary,
        blue: d.BluePrimary,
        white: d.WhitePoint,
        min_luminance: d.MinLuminance,
        max_luminance: d.MaxLuminance,
        max_full_frame_luminance: d.MaxFullFrameLuminance,
    })
}

/// Current colour state and SDR white of the output `device_name`, read through a fresh factory
/// (an output enumerated earlier keeps the state it had then). `None` if it is gone.
pub fn fresh_output_color(device_name: &str) -> Option<(OutputColor, f32)> {
    // SAFETY: plain COM calls.
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.ok()?;
    for i in 0.. {
        // SAFETY: plain COM call.
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(i) }) else {
            break;
        };
        for o in 0.. {
            // SAFETY: plain COM calls.
            let Ok(output) = (unsafe { adapter.EnumOutputs(o) }) else {
                break;
            };
            // SAFETY: plain COM call returning a value.
            let Ok(desc) = (unsafe { output.GetDesc() }) else {
                continue;
            };
            if from_wide(&desc.DeviceName).eq_ignore_ascii_case(device_name) {
                let color = output_color(&output)?;
                let white = super::display::sdr_white_nits_of(
                    &super::display::sdr_white_levels(),
                    device_name,
                );
                return Some((color, white));
            }
        }
    }
    None
}

impl OutputEntry {
    pub fn width(&self) -> i32 {
        self.rect.right - self.rect.left
    }
    pub fn height(&self) -> i32 {
        self.rect.bottom - self.rect.top
    }
    pub fn to_json(&self) -> Value {
        json!({
            "device_name": self.device_name,
            "attached_to_desktop": self.attached,
            "desktop_rect": {
                "left": self.rect.left, "top": self.rect.top,
                "right": self.rect.right, "bottom": self.rect.bottom,
                "width": self.width(), "height": self.height(),
            },
            "rotation": match self.rotation { 1 => "identity", 2 => "rotate90", 3 => "rotate180", 4 => "rotate270", _ => "unspecified" },
            "primary": self.primary,
            "hdr": self.color.map(|c| c.hdr),
            "color_volume": self.color.map(|c| json!({
                "red_primary": c.red,
                "green_primary": c.green,
                "blue_primary": c.blue,
                "white_point": c.white,
                "min_luminance_nits": c.min_luminance,
                "max_luminance_nits": c.max_luminance,
                "max_full_frame_luminance_nits": c.max_full_frame_luminance,
            })),
            "sdr_white_nits": self.sdr_white_nits,
        })
    }
}

/// An adapter with its outputs.
#[derive(Clone)]
pub struct AdapterEntry {
    pub info: AdapterInfo,
    pub adapter: IDXGIAdapter1,
    pub outputs: Vec<OutputEntry>,
}

fn adapter_desc(a: &IDXGIAdapter1) -> windows::core::Result<DXGI_ADAPTER_DESC1> {
    // SAFETY: plain COM call returning a value.
    unsafe { a.GetDesc1() }
}

fn preference_order(factory: &IDXGIFactory1, pref: DXGI_GPU_PREFERENCE) -> Vec<u64> {
    let Ok(f6) = factory.cast::<IDXGIFactory6>() else {
        return Vec::new();
    };
    let mut v = Vec::new();
    for i in 0.. {
        // SAFETY: plain COM call.
        match unsafe { f6.EnumAdapterByGpuPreference::<IDXGIAdapter1>(i, pref) } {
            Ok(a) => {
                if let Ok(d) = adapter_desc(&a) {
                    v.push(luid_u64(d.AdapterLuid));
                }
            }
            Err(_) => break,
        }
    }
    v
}

fn outputs_of(adapter: &IDXGIAdapter1, white_levels: &[(String, u32)]) -> Vec<OutputEntry> {
    let mut v = Vec::new();
    for i in 0.. {
        // SAFETY: plain COM calls; `desc`/`mi` are valid out-parameters.
        unsafe {
            let output = match adapter.EnumOutputs(i) {
                Ok(o) => o,
                Err(e) if e.code() == DXGI_ERROR_NOT_FOUND => break,
                Err(_) => break,
            };
            let Ok(desc) = output.GetDesc() else {
                continue;
            };
            let mut mi = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            let primary = GetMonitorInfoW(desc.Monitor, &mut mi).as_bool()
                && mi.dwFlags & MONITORINFOF_PRIMARY != 0;
            let device_name = from_wide(&desc.DeviceName);
            v.push(OutputEntry {
                color: output_color(&output),
                sdr_white_nits: super::display::sdr_white_nits_of(white_levels, &device_name),
                output,
                device_name,
                attached: desc.AttachedToDesktop.as_bool(),
                rect: desc.DesktopCoordinates,
                rotation: desc.Rotation.0,
                primary,
            });
        }
    }
    v
}

/// Enumerates every adapter (software ones included, flagged) with its outputs.
pub fn enumerate() -> Result<Vec<AdapterEntry>, String> {
    // SAFETY: plain COM calls.
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }
        .map_err(|e| format!("CreateDXGIFactory1: {}", err_str(&e)))?;
    let high = preference_order(&factory, DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE);
    let low = preference_order(&factory, DXGI_GPU_PREFERENCE_MINIMUM_POWER);
    let white_levels = super::display::sdr_white_levels();
    let mut out = Vec::new();
    for i in 0.. {
        // SAFETY: plain COM call.
        let adapter = match unsafe { factory.EnumAdapters1(i) } {
            Ok(a) => a,
            Err(_) => break,
        };
        let Ok(d) = adapter_desc(&adapter) else {
            continue;
        };
        let luid = luid_u64(d.AdapterLuid);
        let outputs = outputs_of(&adapter, &white_levels);
        let info = AdapterInfo {
            name: from_wide(&d.Description),
            vendor_id: d.VendorId,
            device_id: d.DeviceId,
            luid,
            dedicated_vram: d.DedicatedVideoMemory as u64,
            flags: d.Flags,
            high_perf_rank: high.iter().position(|&l| l == luid),
            min_power_rank: low.iter().position(|&l| l == luid),
            owns_primary: outputs.iter().any(|o| o.primary),
        };
        out.push(AdapterEntry {
            info,
            adapter,
            outputs,
        });
    }
    Ok(out)
}

/// Finds the output whose GDI device name equals `name` (case-insensitive).
pub fn find_output<'a>(
    adapters: &'a [AdapterEntry],
    name: &str,
) -> Option<(&'a AdapterEntry, &'a OutputEntry)> {
    adapters.iter().find_map(|a| {
        a.outputs
            .iter()
            .find(|o| o.device_name.eq_ignore_ascii_case(name))
            .map(|o| (a, o))
    })
}

/// Rightmost desktop edge among attached outputs (where a new monitor goes).
pub fn desktop_right_edge(adapters: &[AdapterEntry]) -> i32 {
    adapters
        .iter()
        .flat_map(|a| a.outputs.iter())
        .filter(|o| o.attached)
        .map(|o| o.rect.right)
        .max()
        .unwrap_or(0)
}

/// Creates a D3D11 device on `adapter` with BGRA and video support and multithread protection
/// (the capture thread, the encoder and FFmpeg all use the immediate context).
pub fn create_device(adapter: &IDXGIAdapter1) -> Result<ID3D11Device, String> {
    let mut device = None;
    let levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
    // SAFETY: valid adapter, level list and out-pointer.
    unsafe {
        D3D11CreateDevice(
            adapter,
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
            Some(&levels),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            None,
        )
    }
    .map_err(|e| format!("D3D11CreateDevice: {}", err_str(&e)))?;
    let device = device.ok_or("D3D11CreateDevice returned no device")?;
    match device.cast::<ID3D11Multithread>() {
        Ok(mt) => {
            // SAFETY: plain COM call.
            let _ = unsafe { mt.SetMultithreadProtected(true) };
        }
        Err(e) => tracing::warn!("ID3D11Multithread unavailable: {}", err_str(&e)),
    }
    Ok(device)
}
