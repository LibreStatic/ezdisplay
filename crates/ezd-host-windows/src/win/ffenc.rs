//! FFmpeg glue for zero-copy D3D11 encoding (NVENC, AMF).
//!
//! Deliberately free of `windows`-crate types so it only depends on FFmpeg: the
//! D3D11 device and textures cross this boundary as raw COM pointers.
//!
//! Layout of the FFmpeg structures we fill in is declared here
//! (`libavutil/hwcontext_d3d11va.h`), as the sys crate does not bind that header.

use ezd_ffmpeg::sys as ffi;
use std::ffi::{CStr, CString, c_void};
use std::ptr;

/// `AVD3D11VADeviceContext`.
#[repr(C)]
struct D3d11vaDeviceContext {
    device: *mut c_void,
    device_context: *mut c_void,
    video_device: *mut c_void,
    video_context: *mut c_void,
    lock: Option<unsafe extern "C" fn(*mut c_void)>,
    unlock: Option<unsafe extern "C" fn(*mut c_void)>,
    lock_ctx: *mut c_void,
    bind_flags: u32,
    misc_flags: u32,
}

/// `AVD3D11VAFramesContext`.
#[repr(C)]
struct D3d11vaFramesContext {
    texture: *mut c_void,
    bind_flags: u32,
    misc_flags: u32,
    texture_infos: *mut c_void,
}

const D3D11_BIND_SHADER_RESOURCE: u32 = 0x8;
const D3D11_BIND_RENDER_TARGET: u32 = 0x20;
const EAGAIN: i32 = -11;

fn av_err(what: &str, code: i32) -> String {
    let mut buf = [0 as std::os::raw::c_char; 128];
    // SAFETY: `buf` is writable for its length; av_strerror NUL-terminates.
    let text = unsafe {
        ffi::av_strerror(code, buf.as_mut_ptr(), buf.len());
        CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned()
    };
    format!("{what}: {text} (averror {code})")
}

/// One encoded packet.
pub struct Packet {
    pub data: Vec<u8>,
    pub keyframe: bool,
}

/// An opened encoder fed with BGRA D3D11 frames.
pub struct ZeroCopyEncoder {
    ctx: *mut ffi::AVCodecContext,
    device_ref: *mut ffi::AVBufferRef,
    frames_ref: *mut ffi::AVBufferRef,
    pkt: *mut ffi::AVPacket,
    frame: *mut ffi::AVFrame,
    extradata: Vec<u8>,
    pts: i64,
    name: String,
}

// SAFETY: all raw pointers are owned by this value and used through &mut self only.
unsafe impl Send for ZeroCopyEncoder {}

impl ZeroCopyEncoder {
    /// Opens `name` (an encoder accepting `AV_PIX_FMT_D3D11` frames with BGRA surfaces).
    ///
    /// # Safety
    /// `d3d11_device` must be a valid `ID3D11Device*`; this function takes over one
    /// COM reference to it (FFmpeg releases it when the device context is freed,
    /// also on failure).
    ///
    /// With `array_pool` the frames are slices of one texture array; without, every frame is its
    /// own texture (some encoder runtimes reject array slices at the first frame).
    pub unsafe fn open(
        name: &str,
        d3d11_device: *mut c_void,
        width: u32,
        height: u32,
        refresh_mhz: u32,
        bitrate_kbps: u32,
        array_pool: bool,
    ) -> Result<ZeroCopyEncoder, String> {
        let mut enc = ZeroCopyEncoder {
            ctx: ptr::null_mut(),
            device_ref: ptr::null_mut(),
            frames_ref: ptr::null_mut(),
            pkt: ptr::null_mut(),
            frame: ptr::null_mut(),
            extradata: Vec::new(),
            pts: 0,
            name: name.to_string(),
        };
        // SAFETY: see the function contract; every pointer below is checked before use and
        // everything allocated is released by Drop on the error paths.
        unsafe {
            let dev = ffi::av_hwdevice_ctx_alloc(ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA);
            if dev.is_null() {
                // We still own the COM reference; hand it to FFmpeg's release path by hand.
                release_com(d3d11_device);
                return Err("av_hwdevice_ctx_alloc(D3D11VA) failed".into());
            }
            enc.device_ref = dev;
            let hwdev = (*dev).data as *mut ffi::AVHWDeviceContext;
            let dctx = (*hwdev).hwctx as *mut D3d11vaDeviceContext;
            // FFmpeg releases `device` when the device context is freed.
            (*dctx).device = d3d11_device;
            let r = ffi::av_hwdevice_ctx_init(dev);
            if r < 0 {
                return Err(av_err("av_hwdevice_ctx_init", r));
            }

            let frames = ffi::av_hwframe_ctx_alloc(dev);
            if frames.is_null() {
                return Err("av_hwframe_ctx_alloc failed".into());
            }
            enc.frames_ref = frames;
            let fc = (*frames).data as *mut ffi::AVHWFramesContext;
            (*fc).format = ffi::AVPixelFormat::AV_PIX_FMT_D3D11;
            (*fc).sw_format = ffi::AVPixelFormat::AV_PIX_FMT_BGRA;
            (*fc).width = width as i32;
            (*fc).height = height as i32;
            // A fixed pool is one texture array; size 0 makes FFmpeg allocate single textures.
            (*fc).initial_pool_size = if array_pool { 4 } else { 0 };
            let fh = (*fc).hwctx as *mut D3d11vaFramesContext;
            (*fh).bind_flags = D3D11_BIND_RENDER_TARGET | D3D11_BIND_SHADER_RESOURCE;
            let r = ffi::av_hwframe_ctx_init(frames);
            if r < 0 {
                return Err(av_err("av_hwframe_ctx_init", r));
            }

            let cname = CString::new(name).map_err(|_| "encoder name contains NUL")?;
            let codec = ffi::avcodec_find_encoder_by_name(cname.as_ptr());
            if codec.is_null() {
                return Err(format!("encoder {name} not available in this FFmpeg build"));
            }
            let mut last = String::from("no option set");
            for set in ezd_ffmpeg::option_sets(name) {
                match enc.try_open(codec, &set, width, height, refresh_mhz, bitrate_kbps) {
                    Ok(()) => {
                        enc.pkt = ffi::av_packet_alloc();
                        if enc.pkt.is_null() {
                            return Err("av_packet_alloc failed".into());
                        }
                        return Ok(enc);
                    }
                    Err(e) => {
                        tracing::debug!("opening {name} with {set:?} failed: {e}");
                        last = e;
                        ffi::avcodec_free_context(&mut enc.ctx);
                    }
                }
            }
            Err(last)
        }
    }

    unsafe fn try_open(
        &mut self,
        codec: *const ffi::AVCodec,
        preset: &[(&'static str, String)],
        width: u32,
        height: u32,
        refresh_mhz: u32,
        bitrate_kbps: u32,
    ) -> Result<(), String> {
        // SAFETY: `codec`, `device_ref` and `frames_ref` are valid (checked by the caller); the context
        // is freshly allocated and exclusively ours.
        unsafe {
            let ctx = ffi::avcodec_alloc_context3(codec);
            if ctx.is_null() {
                return Err("avcodec_alloc_context3 failed".into());
            }
            self.ctx = ctx;
            let bitrate = bitrate_kbps as i64 * 1000;
            let fps = refresh_mhz as f64 / 1000.0;
            let c = &mut *ctx;
            c.width = width as i32;
            c.height = height as i32;
            c.time_base = ffi::AVRational {
                num: 1000,
                den: refresh_mhz as i32,
            };
            c.framerate = ffi::AVRational {
                num: refresh_mhz as i32,
                den: 1000,
            };
            c.pix_fmt = ffi::AVPixelFormat::AV_PIX_FMT_D3D11;
            c.bit_rate = bitrate;
            c.rc_max_rate = bitrate;
            c.rc_buffer_size = ((bitrate as f64 * 2.0 / fps) as i32).max(1000);
            c.gop_size = i32::MAX;
            c.max_b_frames = 0;
            c.thread_count = 1;
            c.colorspace = ffi::AVColorSpace::AVCOL_SPC_BT709;
            c.color_primaries = ffi::AVColorPrimaries::AVCOL_PRI_BT709;
            c.color_trc = ffi::AVColorTransferCharacteristic::AVCOL_TRC_BT709;
            c.color_range = ffi::AVColorRange::AVCOL_RANGE_MPEG;
            c.flags |= (ffi::AV_CODEC_FLAG_GLOBAL_HEADER | ffi::AV_CODEC_FLAG_LOW_DELAY) as i32;
            c.hw_device_ctx = ffi::av_buffer_ref(self.device_ref);
            c.hw_frames_ctx = ffi::av_buffer_ref(self.frames_ref);

            let mut dict: *mut ffi::AVDictionary = ptr::null_mut();
            for (k, v) in preset {
                let (k, v) = (
                    CString::new(*k).unwrap_or_default(),
                    CString::new(v.as_str()).unwrap_or_default(),
                );
                ffi::av_dict_set(&mut dict, k.as_ptr(), v.as_ptr(), 0);
            }
            let r = ffi::avcodec_open2(ctx, codec, &mut dict);
            let empty = CString::default();
            let mut e: *const ffi::AVDictionaryEntry = ptr::null();
            loop {
                e = ffi::av_dict_get(dict, empty.as_ptr(), e, ffi::AV_DICT_IGNORE_SUFFIX as i32);
                if e.is_null() {
                    break;
                }
                tracing::warn!(
                    "{}: option {}={} not recognised",
                    self.name,
                    CStr::from_ptr((*e).key).to_string_lossy(),
                    CStr::from_ptr((*e).value).to_string_lossy()
                );
            }
            ffi::av_dict_free(&mut dict);
            if r < 0 {
                return Err(av_err(&format!("avcodec_open2({})", self.name), r));
            }
            let c = &*ctx;
            if !c.extradata.is_null() && c.extradata_size > 0 {
                self.extradata =
                    std::slice::from_raw_parts(c.extradata, c.extradata_size as usize).to_vec();
            }
        }
        Ok(())
    }

    /// Encoder global header (extradata), as FFmpeg produced it.
    pub fn extradata(&self) -> &[u8] {
        &self.extradata
    }

    /// Takes a surface from the pool and returns `(ID3D11Texture2D*, array index)` to render into.
    /// The pointer stays valid until [`submit`](Self::submit) or the next `acquire`.
    pub fn acquire(&mut self) -> Result<(*mut c_void, u32), String> {
        // SAFETY: frames_ref is a valid initialised frames context; the frame is ours.
        unsafe {
            if !self.frame.is_null() {
                ffi::av_frame_free(&mut self.frame);
            }
            let f = ffi::av_frame_alloc();
            if f.is_null() {
                return Err("av_frame_alloc failed".into());
            }
            self.frame = f;
            let r = ffi::av_hwframe_get_buffer(self.frames_ref, f, 0);
            if r < 0 {
                return Err(av_err("av_hwframe_get_buffer", r));
            }
            Ok(((*f).data[0] as *mut c_void, (*f).data[1] as usize as u32))
        }
    }

    /// Sends the surface from [`acquire`](Self::acquire) to the encoder.
    pub fn submit(&mut self, force_key: bool) -> Result<(), String> {
        if self.frame.is_null() {
            return Err("submit without acquire".into());
        }
        // SAFETY: `frame` is a hardware frame from our pool; ctx is an opened encoder.
        unsafe {
            let f = &mut *self.frame;
            f.pts = self.pts;
            self.pts += 1;
            if force_key {
                f.pict_type = ffi::AVPictureType::AV_PICTURE_TYPE_I;
                f.flags |= ffi::AV_FRAME_FLAG_KEY as i32;
            }
            let r = ffi::avcodec_send_frame(self.ctx, self.frame);
            ffi::av_frame_free(&mut self.frame);
            if r < 0 {
                return Err(av_err("avcodec_send_frame", r));
            }
        }
        Ok(())
    }

    /// Fetches one packet if available.
    pub fn receive(&mut self) -> Result<Option<Packet>, String> {
        // SAFETY: ctx/pkt valid; data/size describe a readable buffer until av_packet_unref.
        unsafe {
            let r = ffi::avcodec_receive_packet(self.ctx, self.pkt);
            if r == EAGAIN || r == -(i32::from_le_bytes(*b"EOF ")) {
                return Ok(None);
            }
            if r < 0 {
                return Err(av_err("avcodec_receive_packet", r));
            }
            let p = &*self.pkt;
            let out = Packet {
                data: std::slice::from_raw_parts(p.data, p.size as usize).to_vec(),
                keyframe: p.flags & ffi::AV_PKT_FLAG_KEY as i32 != 0,
            };
            ffi::av_packet_unref(self.pkt);
            Ok(Some(out))
        }
    }
}

/// Releases a COM reference through its vtable (`IUnknown::Release` is slot 2).
///
/// # Safety
/// `p` must be a valid COM interface pointer we own a reference to.
unsafe fn release_com(p: *mut c_void) {
    if p.is_null() {
        return;
    }
    // SAFETY: every COM object starts with a pointer to a vtable whose third entry is Release.
    unsafe {
        let vtbl = *(p as *const *const unsafe extern "system" fn(*mut c_void) -> u32);
        let release = *vtbl.add(2);
        release(p);
    }
}

impl Drop for ZeroCopyEncoder {
    fn drop(&mut self) {
        // SAFETY: each pointer is null or exclusively owned; the free functions reset them.
        unsafe {
            ffi::av_frame_free(&mut self.frame);
            ffi::av_packet_free(&mut self.pkt);
            ffi::avcodec_free_context(&mut self.ctx);
            ffi::av_buffer_unref(&mut self.frames_ref);
            ffi::av_buffer_unref(&mut self.device_ref);
        }
    }
}
