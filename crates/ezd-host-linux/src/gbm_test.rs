//! Minimal libgbm binding for tests, loaded with `dlopen` so the crate has no link-time dependency
//! on it. It allocates scanout-style buffers on the NVIDIA GBM backend to feed the NVENC DMA-BUF
//! path with something the compositor would hand over.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::os::fd::{FromRawFd, OwnedFd};

pub const GBM_FORMAT_XRGB8888: u32 = 0x3432_5258;
pub const GBM_BO_USE_RENDERING: u32 = 1 << 2;
pub const GBM_BO_TRANSFER_WRITE: u32 = 1 << 1;
pub const GBM_BO_USE_LINEAR: u32 = 1 << 4;
pub const DRM_FORMAT_MOD_LINEAR: u64 = 0;

type CreateDevice = unsafe extern "C" fn(c_int) -> *mut c_void;
type DestroyDevice = unsafe extern "C" fn(*mut c_void);
type BackendName = unsafe extern "C" fn(*mut c_void) -> *const c_char;
type BoCreate =
    unsafe extern "C" fn(*mut c_void, u32, u32, u32, *const u64, u32, u32) -> *mut c_void;
type BoCreatePlain = unsafe extern "C" fn(*mut c_void, u32, u32, u32, u32) -> *mut c_void;
type BoGetFd = unsafe extern "C" fn(*mut c_void) -> c_int;
type BoGetU32 = unsafe extern "C" fn(*mut c_void) -> u32;
type BoGetOffset = unsafe extern "C" fn(*mut c_void, c_int) -> u32;
type BoGetModifier = unsafe extern "C" fn(*mut c_void) -> u64;
type BoMap = unsafe extern "C" fn(
    *mut c_void,
    u32,
    u32,
    u32,
    u32,
    u32,
    *mut u32,
    *mut *mut c_void,
) -> *mut c_void;
type BoUnmap = unsafe extern "C" fn(*mut c_void, *mut c_void);
type BoDestroy = unsafe extern "C" fn(*mut c_void);

/// A GBM device on a render node. The library handle is intentionally never closed.
pub struct Gbm {
    dev: *mut c_void,
    // Kept open for the device's lifetime: GBM's backend needs the node's fd.
    _fd: OwnedFd,
    destroy_device: DestroyDevice,
    backend_name: BackendName,
    bo_create: BoCreate,
    bo_create_plain: BoCreatePlain,
    bo_get_fd: BoGetFd,
    bo_get_stride: BoGetU32,
    bo_get_offset: BoGetOffset,
    bo_get_modifier: BoGetModifier,
    bo_map: BoMap,
    bo_unmap: BoUnmap,
    bo_destroy: BoDestroy,
}

/// A GBM buffer object; destroy it before its [`Gbm`].
pub struct Bo<'a> {
    gbm: &'a Gbm,
    bo: *mut c_void,
}

impl Gbm {
    /// Opens `node` through libgbm; the error says why that is not possible here.
    pub fn open(node: &str) -> Result<Gbm, String> {
        // SAFETY: dlopen/dlsym with NUL-terminated names; every symbol is transmuted to the
        // signature documented in gbm.h and the library stays loaded for the process lifetime.
        unsafe {
            let lib = libc::dlopen(c"libgbm.so.1".as_ptr(), libc::RTLD_NOW);
            if lib.is_null() {
                return Err("libgbm.so.1 not available".into());
            }
            macro_rules! sym {
                ($name:literal, $ty:ty) => {{
                    let p = libc::dlsym(lib, $name.as_ptr());
                    if p.is_null() {
                        return Err(format!("libgbm lacks {:?}", $name));
                    }
                    std::mem::transmute::<*mut c_void, $ty>(p)
                }};
            }
            let create_device: CreateDevice = sym!(c"gbm_create_device", CreateDevice);
            let cnode = std::ffi::CString::new(node).map_err(|_| "bad node path")?;
            let fd = libc::open(cnode.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC);
            if fd < 0 {
                return Err(format!("cannot open {node}"));
            }
            let fd = OwnedFd::from_raw_fd(fd);
            let dev = create_device(std::os::fd::AsRawFd::as_raw_fd(&fd));
            if dev.is_null() {
                return Err(format!("gbm_create_device failed on {node}"));
            }
            Ok(Gbm {
                dev,
                _fd: fd,
                destroy_device: sym!(c"gbm_device_destroy", DestroyDevice),
                backend_name: sym!(c"gbm_device_get_backend_name", BackendName),
                bo_create: sym!(c"gbm_bo_create_with_modifiers2", BoCreate),
                bo_create_plain: sym!(c"gbm_bo_create", BoCreatePlain),
                bo_get_fd: sym!(c"gbm_bo_get_fd", BoGetFd),
                bo_get_stride: sym!(c"gbm_bo_get_stride", BoGetU32),
                bo_get_offset: sym!(c"gbm_bo_get_offset", BoGetOffset),
                bo_get_modifier: sym!(c"gbm_bo_get_modifier", BoGetModifier),
                bo_map: sym!(c"gbm_bo_map", BoMap),
                bo_unmap: sym!(c"gbm_bo_unmap", BoUnmap),
                bo_destroy: sym!(c"gbm_bo_destroy", BoDestroy),
            })
        }
    }

    pub fn backend(&self) -> String {
        // SAFETY: returns a static NUL-terminated string owned by the device.
        unsafe { CStr::from_ptr((self.backend_name)(self.dev)) }
            .to_string_lossy()
            .into_owned()
    }

    /// Allocates an XRGB8888 buffer for rendering, restricted to `modifiers`. A linear-only
    /// request falls back to `GBM_BO_USE_LINEAR`: the NVIDIA backend refuses LINEAR with
    /// `GBM_BO_USE_RENDERING` through the modifier entry point on some setups.
    pub fn create_bo(&self, w: u32, h: u32, modifiers: &[u64]) -> Result<Bo<'_>, String> {
        // SAFETY: the modifier list outlives the call.
        let mut bo = unsafe {
            (self.bo_create)(
                self.dev,
                w,
                h,
                GBM_FORMAT_XRGB8888,
                modifiers.as_ptr(),
                modifiers.len() as u32,
                GBM_BO_USE_RENDERING,
            )
        };
        if bo.is_null() && modifiers == [DRM_FORMAT_MOD_LINEAR] {
            // SAFETY: plain allocation on a valid device.
            bo = unsafe {
                (self.bo_create_plain)(self.dev, w, h, GBM_FORMAT_XRGB8888, GBM_BO_USE_LINEAR)
            };
        }
        if bo.is_null() {
            Err(format!(
                "gbm_bo_create_with_modifiers2 refused {modifiers:x?}"
            ))
        } else {
            Ok(Bo { gbm: self, bo })
        }
    }
}

impl Drop for Gbm {
    fn drop(&mut self) {
        // SAFETY: owned device, no buffer objects outlive it.
        unsafe { (self.destroy_device)(self.dev) };
    }
}

impl Bo<'_> {
    /// A new dma-buf fd for the buffer.
    pub fn fd(&self) -> Result<OwnedFd, String> {
        // SAFETY: returns a fresh fd owned by the caller (or a negative error).
        let fd = unsafe { (self.gbm.bo_get_fd)(self.bo) };
        if fd < 0 {
            Err("gbm_bo_get_fd failed".into())
        } else {
            // SAFETY: valid, uniquely owned fd.
            Ok(unsafe { OwnedFd::from_raw_fd(fd) })
        }
    }

    pub fn stride(&self) -> u32 {
        // SAFETY: valid bo.
        unsafe { (self.gbm.bo_get_stride)(self.bo) }
    }

    pub fn offset(&self) -> u32 {
        // SAFETY: valid bo, plane 0.
        unsafe { (self.gbm.bo_get_offset)(self.bo, 0) }
    }

    pub fn modifier(&self) -> u64 {
        // SAFETY: valid bo.
        unsafe { (self.gbm.bo_get_modifier)(self.bo) }
    }

    /// Maps the whole `w`x`h` buffer for writing and calls `fill(base, stride)`. Fails for
    /// block-linear buffers.
    pub fn write_mapped(
        &self,
        w: u32,
        h: u32,
        fill: impl FnOnce(*mut u8, usize),
    ) -> Result<(), String> {
        let (mut stride, mut data) = (0u32, std::ptr::null_mut());
        // SAFETY: map/unmap pair on a valid bo; `fill` only gets a mapping of stride * h bytes.
        unsafe {
            let p = (self.gbm.bo_map)(
                self.bo,
                0,
                0,
                w,
                h,
                GBM_BO_TRANSFER_WRITE,
                &mut stride,
                &mut data,
            );
            if p.is_null() {
                return Err("gbm_bo_map failed".into());
            }
            fill(p as *mut u8, stride as usize);
            (self.gbm.bo_unmap)(self.bo, data);
        }
        Ok(())
    }
}

impl Drop for Bo<'_> {
    fn drop(&mut self) {
        // SAFETY: owned bo.
        unsafe { (self.gbm.bo_destroy)(self.bo) };
    }
}
