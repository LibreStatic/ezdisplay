//! DMA-BUF import into Vulkan with a dedicated allocation, for the NVENC path.
//!
//! FFmpeg's DRM to Vulkan map (`av_hwframe_map`, n8.1 and master as of October 2026) binds the
//! imported memory with a dedicated allocation only when the driver asks for one. The NVIDIA driver
//! (595) does not ask, yet some imports made without one sample as zeros for the whole life of the
//! image: with KWin's buffers about one frame in 16 came out black. The pattern depends only on the
//! sequence of allocations, not on timing, and a dedicated allocation removes it. This module
//! imports single-plane buffers that way and hands FFmpeg the image as an `AVVkFrame`.

use crate::frame::Plane;
use ash::vk::{self, Handle};
use ezd_ffmpeg::sys as ff;
use std::ffi::c_void;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};
use std::sync::Arc;

/// The functions of an FFmpeg Vulkan device that importing needs.
pub struct VkDma {
    dev: ash::Device,
    fd_fns: ash::khr::external_memory_fd::Device,
    memory: vk::PhysicalDeviceMemoryProperties,
    /// Queue families FFmpeg uses with images; images are shared between them.
    queue_families: Vec<u32>,
}

impl VkDma {
    /// Loads the functions of `dev`, an `AV_HWDEVICE_TYPE_VULKAN` device context.
    ///
    /// # Safety
    /// `dev` must be a valid Vulkan device context that outlives the result and every frame it
    /// imports.
    pub unsafe fn new(dev: *const ff::AVBufferRef) -> Result<VkDma, String> {
        // SAFETY: per the contract, `dev` is a Vulkan device context, so `hwctx` is an
        // AVVulkanDeviceContext whose handles and loader stay valid.
        unsafe {
            let hw = &*((*dev).data as *const ff::AVHWDeviceContext);
            let vkc = &*(hw.hwctx as *const ff::AVVulkanDeviceContext);
            let gipa = vkc
                .get_proc_addr
                .ok_or("FFmpeg Vulkan device has no loader")?;
            // Same C signature; only the handle types are spelled differently.
            let static_fn = ash::StaticFn {
                get_instance_proc_addr: std::mem::transmute::<
                    unsafe extern "C" fn(
                        ff::VkInstance,
                        *const std::ffi::c_char,
                    ) -> ff::PFN_vkVoidFunction,
                    vk::PFN_vkGetInstanceProcAddr,
                >(gipa),
            };
            let inst = ash::Instance::load(&static_fn, vk::Instance::from_raw(vkc.inst as u64));
            let dev = ash::Device::load(inst.fp_v1_0(), vk::Device::from_raw(vkc.act_dev as u64));
            let fd_fns = ash::khr::external_memory_fd::Device::new(&inst, &dev);
            let memory = inst.get_physical_device_memory_properties(vk::PhysicalDevice::from_raw(
                vkc.phys_dev as u64,
            ));
            let queue_families = ezd_ffmpeg::vulkan_queue_families(vkc);
            Ok(VkDma {
                dev,
                fd_fns,
                memory,
                queue_families,
            })
        }
    }

    /// Imports a single-plane DMA-BUF of `w`x`h` and `format` (with DRM `modifier`) into `dst`, a
    /// fresh frame: its `data[0]` becomes an `AVVkFrame` owned by `buf[0]`, which destroys the image
    /// once FFmpeg has dropped every reference. The caller sets `hw_frames_ctx` to a Vulkan frames
    /// context of the same format and must have waited for the buffer's writers (the import adds no
    /// synchronization).
    ///
    /// # Safety
    /// `dst` must be a valid frame with no buffers attached.
    pub unsafe fn import(
        self: &Arc<Self>,
        dst: *mut ff::AVFrame,
        w: u32,
        h: u32,
        format: vk::Format,
        modifier: u64,
        plane: &Plane,
    ) -> Result<(), String> {
        let mut img = Image {
            vk: self.clone(),
            image: vk::Image::null(),
            memory: vk::DeviceMemory::null(),
            sem: vk::Semaphore::null(),
        };
        let layouts = [vk::SubresourceLayout {
            offset: plane.offset as u64,
            row_pitch: plane.pitch as u64,
            ..Default::default()
        }];
        let mut drm = vk::ImageDrmFormatModifierExplicitCreateInfoEXT::default()
            .drm_format_modifier(modifier)
            .plane_layouts(&layouts);
        let mut external = vk::ExternalMemoryImageCreateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        let sharing = if self.queue_families.len() > 1 {
            vk::SharingMode::CONCURRENT
        } else {
            vk::SharingMode::EXCLUSIVE
        };
        let info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(vk::Extent3D {
                width: w,
                height: h,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
            .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_SRC)
            .sharing_mode(sharing)
            .queue_family_indices(&self.queue_families)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .push_next(&mut external)
            .push_next(&mut drm);
        // SAFETY: valid create info; the image is destroyed by `img` on every path.
        img.image = unsafe { self.dev.create_image(&info, None) }
            .map_err(|e| format!("vkCreateImage for a DMA-BUF: {e}"))?;
        // SAFETY: a valid image.
        let req = unsafe { self.dev.get_image_memory_requirements(img.image) };

        // Vulkan takes ownership of the fd once the import succeeds.
        // SAFETY: F_DUPFD_CLOEXEC on a valid fd; a non-negative result is a new fd we own.
        let fd = unsafe { libc::fcntl(plane.fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
        if fd < 0 {
            return Err("cannot dup the DMA-BUF fd".into());
        }
        // SAFETY: fresh fd from fcntl.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        let mut fd_props = vk::MemoryFdPropertiesKHR::default();
        // SAFETY: a valid DMA-BUF fd.
        unsafe {
            self.fd_fns.get_memory_fd_properties(
                vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT,
                fd.as_raw_fd(),
                &mut fd_props,
            )
        }
        .map_err(|e| format!("vkGetMemoryFdPropertiesKHR: {e}"))?;
        let type_index = self
            .memory_type(req.memory_type_bits & fd_props.memory_type_bits)
            .ok_or("no memory type can hold this DMA-BUF")?;
        let mut import = vk::ImportMemoryFdInfoKHR::default()
            .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT)
            .fd(fd.as_raw_fd());
        let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(img.image);
        let alloc = vk::MemoryAllocateInfo::default()
            .allocation_size(req.size)
            .memory_type_index(type_index)
            .push_next(&mut import)
            .push_next(&mut dedicated);
        // SAFETY: valid allocate info; on success the memory owns the fd.
        img.memory = unsafe { self.dev.allocate_memory(&alloc, None) }
            .map_err(|e| format!("import DMA-BUF memory: {e}"))?;
        let _ = fd.into_raw_fd();
        // SAFETY: image and memory are valid and unbound.
        unsafe { self.dev.bind_image_memory(img.image, img.memory, 0) }
            .map_err(|e| format!("vkBindImageMemory: {e}"))?;
        let mut timeline =
            vk::SemaphoreTypeCreateInfo::default().semaphore_type(vk::SemaphoreType::TIMELINE);
        // SAFETY: valid create info.
        img.sem = unsafe {
            self.dev.create_semaphore(
                &vk::SemaphoreCreateInfo::default().push_next(&mut timeline),
                None,
            )
        }
        .map_err(|e| format!("vkCreateSemaphore: {e}"))?;

        // SAFETY: av_vk_frame_alloc returns a zeroed frame (or null); the buffer created below owns
        // it and `img` from then on, and `dst` has no buffers per the contract.
        unsafe {
            let f = ff::av_vk_frame_alloc();
            if f.is_null() {
                return Err("av_vk_frame_alloc failed".into());
            }
            (*f).img[0] = img.image.as_raw();
            (*f).tiling = vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT.as_raw() as _;
            (*f).mem[0] = img.memory.as_raw();
            (*f).size[0] = req.size as usize;
            (*f).flags = self.memory.memory_types[type_index as usize]
                .property_flags
                .as_raw() as _;
            (*f).layout[0] = vk::ImageLayout::UNDEFINED.as_raw() as _;
            (*f).sem[0] = img.sem.as_raw();
            // The first barrier acquires the image from its external owner.
            (*f).queue_family[0] = vk::QUEUE_FAMILY_EXTERNAL;
            let owner = Box::into_raw(Box::new(Imported { img, frame: f }));
            let buf = ff::av_buffer_create(
                f as *mut u8,
                std::mem::size_of::<ff::AVVkFrame>(),
                Some(free_imported),
                owner as *mut c_void,
                0,
            );
            if buf.is_null() {
                free_imported(owner as *mut c_void, f as *mut u8);
                return Err("av_buffer_create failed".into());
            }
            (*dst).buf[0] = buf;
            (*dst).data[0] = f as *mut u8;
            (*dst).format = ff::AVPixelFormat::AV_PIX_FMT_VULKAN as i32;
            (*dst).width = w as i32;
            (*dst).height = h as i32;
        }
        Ok(())
    }

    /// First memory type in `bits`, preferring device-local ones.
    fn memory_type(&self, bits: u32) -> Option<u32> {
        let types = &self.memory.memory_types[..self.memory.memory_type_count as usize];
        let ok = |i: &u32| bits & (1 << i) != 0;
        (0..types.len() as u32)
            .filter(ok)
            .find(|&i| {
                types[i as usize]
                    .property_flags
                    .contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
            })
            .or_else(|| (0..types.len() as u32).find(ok))
    }
}

/// Vulkan objects of one import, destroyed on drop (null handles are skipped).
struct Image {
    vk: Arc<VkDma>,
    image: vk::Image,
    memory: vk::DeviceMemory,
    sem: vk::Semaphore,
}

impl Drop for Image {
    fn drop(&mut self) {
        // SAFETY: the handles were created on this device and nothing uses them any more (FFmpeg
        // dropped its last reference, or the import failed before handing them out).
        unsafe {
            self.vk.dev.destroy_semaphore(self.sem, None);
            self.vk.dev.destroy_image(self.image, None);
            self.vk.dev.free_memory(self.memory, None);
        }
    }
}

/// What the `AVBufferRef` of an imported frame owns.
struct Imported {
    img: Image,
    frame: *mut ff::AVVkFrame,
}

unsafe extern "C" fn free_imported(opaque: *mut c_void, _data: *mut u8) {
    // SAFETY: `opaque` is the box made in `import`, freed exactly once (by FFmpeg or on failure).
    let owner = unsafe { Box::from_raw(opaque as *mut Imported) };
    let f = owner.frame;
    drop(owner.img);
    // `av_vk_frame_alloc` also allocates the frame's internal state: a pthread mutex, which holds
    // no resources on Linux, and CUDA state that is only set when the frame is exported to CUDA,
    // which these input frames never are.
    // SAFETY: both allocations come from av_vk_frame_alloc and nothing references them any more.
    unsafe {
        ff::av_free((*f).internal as *mut c_void);
        ff::av_free(f as *mut c_void);
    }
}
