//! GPU colour conversion for an HDR desktop (scRGB FP16 from Desktop Duplication).
//!
//! * [`ScrgbToP010`]: scRGB -> BT.2020 PQ Y'CbCr 10-bit (HDR10) written into a `P010` texture.
//! * [`ScrgbToSdr`]: scRGB -> BGRA8 sRGB at the user's SDR white level, for clients that cannot
//!   show HDR; the regular SDR pipeline continues from there.
//!
//! Both are full-screen-triangle pixel shaders (feature level 11_0, no compute, no sampler)
//! compiled once per session with `D3DCompile` (`d3dcompiler_47.dll` ships with Windows 10).
//! The formulas are the ones of [`crate::hdr_math`], which the WARP test compares against.
//!
//! P010 layout: the luma pass renders into an `R16_UNORM` view of plane 0, the chroma pass into
//! an `R16G16_UNORM` view of plane 1 (half size). P010 keeps its 10 bits in the high bits of
//! each 16-bit word, so a shader writes `code10 * 64 / 65535` (an exact `code10 << 6`).
//! Chroma is the 2x2 box average of the four luma pixels' R'G'B' (centred siting, half a pixel
//! right of the left siting BT.2020 content nominally uses; invisible in practice).

use super::util::err_str;
use std::collections::HashMap;
use std::ffi::c_void;
use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::{D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, ID3DBlob};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_CONSTANT_BUFFER, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE,
    D3D11_BUFFER_DESC, D3D11_CULL_NONE, D3D11_FILL_SOLID, D3D11_RASTERIZER_DESC,
    D3D11_RENDER_TARGET_VIEW_DESC, D3D11_RENDER_TARGET_VIEW_DESC_0,
    D3D11_RTV_DIMENSION_TEXTURE2DARRAY, D3D11_SUBRESOURCE_DATA, D3D11_TEX2D_ARRAY_RTV,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_VIEWPORT, ID3D11Buffer, ID3D11ClassLinkage,
    ID3D11Device, ID3D11DeviceContext, ID3D11PixelShader, ID3D11RasterizerState,
    ID3D11RenderTargetView, ID3D11ShaderResourceView, ID3D11Texture2D, ID3D11VertexShader,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_P010, DXGI_FORMAT_R16_UNORM,
    DXGI_FORMAT_R16G16_UNORM, DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_SAMPLE_DESC,
};
use windows::core::{Interface, PCSTR, s};

/// HLSL of every pass. `Load` only: no sampler, no filtering, one source pixel per fetch.
const HLSL: &str = r#"
Texture2D<float4> src : register(t0);

cbuffer Params : register(b0) {
    float sdr_scale;   // 80 / SDR white level in cd/m2
    float3 pad;
};

struct VsOut { float4 pos : SV_Position; };

// One triangle covering the viewport; no vertex buffer.
VsOut vs_main(uint id : SV_VertexID) {
    float2 uv = float2((id << 1) & 2, id & 2);
    VsOut o;
    o.pos = float4(uv * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
    return o;
}

static const float3x3 M709_TO_2020 = float3x3(
    0.627404, 0.329283, 0.043313,
    0.069097, 0.919540, 0.011362,
    0.016391, 0.088013, 0.895595);

float3 pq_encode(float3 nits) {
    float3 y = saturate(nits / 10000.0);
    float3 p = pow(y, 0.1593017578125);
    return pow((0.8359375 + 18.8515625 * p) / (1.0 + 18.6875 * p), 78.84375);
}

// scRGB (linear, BT.709, 1.0 = 80 nits) -> PQ-encoded BT.2020 R'G'B'.
float3 scrgb_to_pq2020(float3 rgb) {
    float3 lin = mul(M709_TO_2020, rgb);
    return pq_encode(max(lin * 80.0, 0.0));
}

float3 fetch_e(int2 p) {
    return scrgb_to_pq2020(src.Load(int3(p, 0)).rgb);
}

static const float KR = 0.2627;
static const float KB = 0.0593;
static const float KG = 0.6780;

// 10-bit code -> value of a 16-bit UNORM view of a P010 plane (code << 6).
float p010(float code) {
    return floor(code + 0.5) * 64.0 / 65535.0;
}

float4 ps_y(VsOut i) : SV_Target {
    float3 e = fetch_e(int2(i.pos.xy));
    float y = KR * e.r + KG * e.g + KB * e.b;
    return float4(p010(64.0 + 876.0 * y), 0.0, 0.0, 1.0);
}

float4 ps_uv(VsOut i) : SV_Target {
    int2 p = int2(i.pos.xy) * 2;
    float3 e = 0.25 * (fetch_e(p) + fetch_e(p + int2(1, 0)) + fetch_e(p + int2(0, 1)) + fetch_e(p + int2(1, 1)));
    float y = KR * e.r + KG * e.g + KB * e.b;
    float cb = (e.b - y) / (2.0 * (1.0 - KB));
    float cr = (e.r - y) / (2.0 * (1.0 - KR));
    return float4(p010(512.0 + 896.0 * cb), p010(512.0 + 896.0 * cr), 0.0, 1.0);
}

float srgb_oetf(float v) {
    v = saturate(v);
    return v <= 0.0031308 ? v * 12.92 : 1.055 * pow(v, 1.0 / 2.4) - 0.055;
}

float4 ps_sdr(VsOut i) : SV_Target {
    float3 c = max(src.Load(int3(int2(i.pos.xy), 0)).rgb * sdr_scale, 0.0);
    float m = max(c.r, max(c.g, c.b));
    if (m > 1.0) {
        c = c / m;
    }
    return float4(srgb_oetf(c.r), srgb_oetf(c.g), srgb_oetf(c.b), 1.0);
}
"#;

fn compile(entry: PCSTR, target: PCSTR) -> Result<Vec<u8>, String> {
    let mut code: Option<ID3DBlob> = None;
    let mut errors: Option<ID3DBlob> = None;
    // SAFETY: the source slice and the out-pointers are valid for the call.
    let r = unsafe {
        D3DCompile(
            HLSL.as_ptr().cast::<c_void>(),
            HLSL.len(),
            s!("ezd_hdr.hlsl"),
            None,
            None::<&windows::Win32::Graphics::Direct3D::ID3DInclude>,
            entry,
            target,
            0,
            0,
            &mut code,
            Some(&mut errors),
        )
    };
    let blob_bytes = |b: &ID3DBlob| {
        // SAFETY: a blob owns `GetBufferSize` bytes at `GetBufferPointer` for its lifetime.
        unsafe { std::slice::from_raw_parts(b.GetBufferPointer().cast::<u8>(), b.GetBufferSize()) }
            .to_vec()
    };
    match r {
        Ok(()) => code
            .as_ref()
            .map(blob_bytes)
            .ok_or_else(|| "D3DCompile returned no code".to_string()),
        Err(e) => {
            let msg = errors
                .as_ref()
                .map(|b| String::from_utf8_lossy(&blob_bytes(b)).trim().to_string())
                .unwrap_or_default();
            Err(format!("D3DCompile: {} {msg}", err_str(&e)))
        }
    }
}

fn vertex_shader(device: &ID3D11Device) -> Result<ID3D11VertexShader, String> {
    let bytes = compile(s!("vs_main"), s!("vs_5_0"))?;
    let mut vs = None;
    // SAFETY: `bytes` is compiled vs_5_0 bytecode.
    unsafe { device.CreateVertexShader(&bytes, None::<&ID3D11ClassLinkage>, Some(&mut vs)) }
        .map_err(|e| format!("CreateVertexShader: {}", err_str(&e)))?;
    vs.ok_or_else(|| "no vertex shader".to_string())
}

fn pixel_shader(device: &ID3D11Device, entry: PCSTR) -> Result<ID3D11PixelShader, String> {
    let bytes = compile(entry, s!("ps_5_0"))?;
    let mut ps = None;
    // SAFETY: `bytes` is compiled ps_5_0 bytecode.
    unsafe { device.CreatePixelShader(&bytes, None::<&ID3D11ClassLinkage>, Some(&mut ps)) }
        .map_err(|e| format!("CreatePixelShader: {}", err_str(&e)))?;
    ps.ok_or_else(|| "no pixel shader".to_string())
}

/// A zero-initialised texture; `bind` is a combination of `D3D11_BIND_*`.
pub fn create_texture(
    device: &ID3D11Device,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
    bind: u32,
) -> Result<ID3D11Texture2D, String> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: bind,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let mut tex = None;
    // SAFETY: `desc` is fully initialised; no initial data (planar formats cannot take any).
    unsafe { device.CreateTexture2D(&desc, None, Some(&mut tex)) }
        .map_err(|e| format!("CreateTexture2D({format:?}): {}", err_str(&e)))?;
    tex.ok_or_else(|| "CreateTexture2D returned no texture".to_string())
}

/// A `P010` texture a shader can render into (the encoder surface for the 10-bit path).
pub fn create_p010_texture(
    device: &ID3D11Device,
    width: u32,
    height: u32,
) -> Result<ID3D11Texture2D, String> {
    create_texture(
        device,
        width,
        height,
        DXGI_FORMAT_P010,
        (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
    )
}

/// A black FP16 scRGB texture, the format of an HDR desktop capture.
pub fn create_fp16_texture(
    device: &ID3D11Device,
    width: u32,
    height: u32,
) -> Result<ID3D11Texture2D, String> {
    create_texture(
        device,
        width,
        height,
        DXGI_FORMAT_R16G16B16A16_FLOAT,
        (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
    )
}

fn plane_rtv(
    device: &ID3D11Device,
    tex: &ID3D11Texture2D,
    format: DXGI_FORMAT,
    slice: u32,
) -> Result<ID3D11RenderTargetView, String> {
    let desc = D3D11_RENDER_TARGET_VIEW_DESC {
        Format: format,
        ViewDimension: D3D11_RTV_DIMENSION_TEXTURE2DARRAY,
        Anonymous: D3D11_RENDER_TARGET_VIEW_DESC_0 {
            Texture2DArray: D3D11_TEX2D_ARRAY_RTV {
                MipSlice: 0,
                FirstArraySlice: slice,
                ArraySize: 1,
            },
        },
    };
    let mut rtv = None;
    // SAFETY: valid resource and descriptor.
    unsafe { device.CreateRenderTargetView(tex, Some(&desc), Some(&mut rtv)) }
        .map_err(|e| format!("CreateRenderTargetView({format:?}): {}", err_str(&e)))?;
    rtv.ok_or_else(|| "no render target view".to_string())
}

/// State shared by the passes: the triangle vertex shader, raster state and a source view cache.
struct Pass {
    device: ID3D11Device,
    ctx: ID3D11DeviceContext,
    vs: ID3D11VertexShader,
    raster: ID3D11RasterizerState,
    /// Identity of the source texture the cached view belongs to.
    src_view: Option<(*mut c_void, ID3D11ShaderResourceView)>,
}

// SAFETY: used from the thread owning the encoder; the device is multithread protected. The raw
// pointer is only an identity key.
unsafe impl Send for Pass {}

impl Pass {
    fn new(device: &ID3D11Device) -> Result<Pass, String> {
        // SAFETY: plain COM call.
        let ctx = unsafe { device.GetImmediateContext() }
            .map_err(|e| format!("GetImmediateContext: {}", err_str(&e)))?;
        let desc = D3D11_RASTERIZER_DESC {
            FillMode: D3D11_FILL_SOLID,
            CullMode: D3D11_CULL_NONE,
            DepthClipEnable: true.into(),
            ..Default::default()
        };
        let mut raster = None;
        // SAFETY: valid descriptor.
        unsafe { device.CreateRasterizerState(&desc, Some(&mut raster)) }
            .map_err(|e| format!("CreateRasterizerState: {}", err_str(&e)))?;
        Ok(Pass {
            device: device.clone(),
            ctx,
            vs: vertex_shader(device)?,
            raster: raster.ok_or("no rasterizer state")?,
            src_view: None,
        })
    }

    fn srv(&mut self, src: &ID3D11Texture2D) -> Result<ID3D11ShaderResourceView, String> {
        let key = src.as_raw();
        if let Some((k, v)) = &self.src_view {
            if *k == key {
                return Ok(v.clone());
            }
        }
        let mut view = None;
        // SAFETY: valid resource; a null descriptor views the whole texture in its own format.
        unsafe {
            self.device
                .CreateShaderResourceView(src, None, Some(&mut view))
        }
        .map_err(|e| format!("CreateShaderResourceView: {}", err_str(&e)))?;
        let view = view.ok_or("no shader resource view")?;
        self.src_view = Some((key, view.clone()));
        Ok(view)
    }

    /// Draws the triangle with `ps` into `rtv` (a `width x height` area), reading `srv` and `cb`.
    fn draw(
        &self,
        ps: &ID3D11PixelShader,
        srv: &ID3D11ShaderResourceView,
        cb: Option<&ID3D11Buffer>,
        rtv: &ID3D11RenderTargetView,
        width: u32,
        height: u32,
    ) {
        let vp = D3D11_VIEWPORT {
            TopLeftX: 0.0,
            TopLeftY: 0.0,
            Width: width as f32,
            Height: height as f32,
            MinDepth: 0.0,
            MaxDepth: 1.0,
        };
        // SAFETY: all objects belong to `self.device`; bindings are cleared before returning so
        // neither texture stays bound while the encoder or the capture thread uses it.
        unsafe {
            let c = &self.ctx;
            c.IASetInputLayout(None::<&windows::Win32::Graphics::Direct3D11::ID3D11InputLayout>);
            c.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            c.VSSetShader(&self.vs, None);
            c.PSSetShader(ps, None);
            c.PSSetShaderResources(0, Some(&[Some(srv.clone())]));
            if let Some(cb) = cb {
                c.PSSetConstantBuffers(0, Some(&[Some(cb.clone())]));
            }
            c.RSSetState(&self.raster);
            c.RSSetViewports(Some(&[vp]));
            c.OMSetRenderTargets(
                Some(&[Some(rtv.clone())]),
                None::<&windows::Win32::Graphics::Direct3D11::ID3D11DepthStencilView>,
            );
            c.Draw(3, 0);
            c.OMSetRenderTargets(
                None,
                None::<&windows::Win32::Graphics::Direct3D11::ID3D11DepthStencilView>,
            );
            c.PSSetShaderResources(0, Some(&[None]));
        }
    }
}

/// Destination plane views kept by [`ScrgbToP010`].
const MAX_CACHED_VIEWS: usize = 32;

/// scRGB FP16 -> P010 (HDR10 PQ, BT.2020 NCL, limited range).
pub struct ScrgbToP010 {
    pass: Pass,
    ps_y: ID3D11PixelShader,
    ps_uv: ID3D11PixelShader,
    width: u32,
    height: u32,
    /// Plane views of destination textures, by `(texture, array slice)`.
    views: HashMap<(usize, u32), (ID3D11RenderTargetView, ID3D11RenderTargetView)>,
}

impl ScrgbToP010 {
    /// Compiles the shaders for `width x height` frames (both even).
    pub fn new(device: &ID3D11Device, width: u32, height: u32) -> Result<ScrgbToP010, String> {
        if width % 2 != 0 || height % 2 != 0 {
            return Err(format!("P010 needs even dimensions, got {width}x{height}"));
        }
        Ok(ScrgbToP010 {
            pass: Pass::new(device)?,
            ps_y: pixel_shader(device, s!("ps_y"))?,
            ps_uv: pixel_shader(device, s!("ps_uv"))?,
            width,
            height,
            views: HashMap::new(),
        })
    }

    /// Renders `src` (FP16 scRGB, `width x height`) into the planes `y` (`R16_UNORM`, full size)
    /// and `uv` (`R16G16_UNORM`, half size).
    pub fn convert_views(
        &mut self,
        src: &ID3D11Texture2D,
        y: &ID3D11RenderTargetView,
        uv: &ID3D11RenderTargetView,
    ) -> Result<(), String> {
        let srv = self.pass.srv(src)?;
        self.pass
            .draw(&self.ps_y, &srv, None, y, self.width, self.height);
        self.pass
            .draw(&self.ps_uv, &srv, None, uv, self.width / 2, self.height / 2);
        Ok(())
    }

    /// Renders `src` into slice `slice` of the `P010` texture `dst`.
    pub fn convert(
        &mut self,
        src: &ID3D11Texture2D,
        dst: &ID3D11Texture2D,
        slice: u32,
    ) -> Result<(), String> {
        let key = (dst.as_raw() as usize, slice);
        if !self.views.contains_key(&key) {
            // A pool of N array slices needs N entries; a pool of single textures that FFmpeg
            // recycles lazily could otherwise grow this without bound (the views keep their
            // textures alive).
            if self.views.len() >= MAX_CACHED_VIEWS {
                self.views.clear();
            }
            let y = plane_rtv(&self.pass.device, dst, DXGI_FORMAT_R16_UNORM, slice)?;
            let uv = plane_rtv(&self.pass.device, dst, DXGI_FORMAT_R16G16_UNORM, slice)?;
            self.views.insert(key, (y, uv));
        }
        let (y, uv) = self.views.get(&key).cloned().ok_or("view cache")?;
        self.convert_views(src, &y, &uv)
    }
}

/// scRGB FP16 -> BGRA8 sRGB at the SDR white level.
pub struct ScrgbToSdr {
    pass: Pass,
    ps: ID3D11PixelShader,
    cb: ID3D11Buffer,
    white_nits: f32,
    width: u32,
    height: u32,
}

/// Matches the HLSL `Params` constant buffer (16 bytes).
fn params_bytes(white_nits: f32) -> [f32; 4] {
    [
        crate::hdr_math::SCRGB_WHITE_NITS / white_nits.max(1.0),
        0.0,
        0.0,
        0.0,
    ]
}

impl ScrgbToSdr {
    pub fn new(
        device: &ID3D11Device,
        width: u32,
        height: u32,
        white_nits: f32,
    ) -> Result<ScrgbToSdr, String> {
        let init = params_bytes(white_nits);
        let desc = D3D11_BUFFER_DESC {
            ByteWidth: size_of::<[f32; 4]>() as u32,
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
            ..Default::default()
        };
        let data = D3D11_SUBRESOURCE_DATA {
            pSysMem: init.as_ptr().cast(),
            SysMemPitch: 0,
            SysMemSlicePitch: 0,
        };
        let mut cb = None;
        // SAFETY: `init` outlives the call and is `ByteWidth` bytes.
        unsafe { device.CreateBuffer(&desc, Some(&data), Some(&mut cb)) }
            .map_err(|e| format!("CreateBuffer: {}", err_str(&e)))?;
        Ok(ScrgbToSdr {
            pass: Pass::new(device)?,
            ps: pixel_shader(device, s!("ps_sdr"))?,
            cb: cb.ok_or("no constant buffer")?,
            white_nits,
            width,
            height,
        })
    }

    /// Follows a change of the Windows "SDR content brightness" setting.
    pub fn set_white_nits(&mut self, white_nits: f32) {
        if (white_nits - self.white_nits).abs() < 0.5 {
            return;
        }
        self.white_nits = white_nits;
        let v = params_bytes(white_nits);
        // SAFETY: `cb` is a DEFAULT buffer of 16 bytes and `v` is 16 bytes.
        unsafe {
            self.pass
                .ctx
                .UpdateSubresource(&self.cb, 0, None, v.as_ptr().cast(), 0, 0)
        };
    }

    /// Renders `src` (FP16 scRGB) into `dst` (`B8G8R8A8_UNORM` render target, same size).
    pub fn convert(
        &mut self,
        src: &ID3D11Texture2D,
        dst: &ID3D11RenderTargetView,
    ) -> Result<(), String> {
        let srv = self.pass.srv(src)?;
        self.pass
            .draw(&self.ps, &srv, Some(&self.cb), dst, self.width, self.height);
        Ok(())
    }
}

/// A BGRA8 texture usable as render target and shader resource, with its render target view:
/// the output of [`ScrgbToSdr`].
pub fn create_sdr_target(
    device: &ID3D11Device,
    width: u32,
    height: u32,
) -> Result<(ID3D11Texture2D, ID3D11RenderTargetView), String> {
    let tex = create_texture(
        device,
        width,
        height,
        DXGI_FORMAT_B8G8R8A8_UNORM,
        (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
    )?;
    let mut rtv = None;
    // SAFETY: valid resource; a null descriptor views the whole texture.
    unsafe { device.CreateRenderTargetView(&tex, None, Some(&mut rtv)) }
        .map_err(|e| format!("CreateRenderTargetView(BGRA): {}", err_str(&e)))?;
    Ok((tex, rtv.ok_or("no render target view")?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hdr_math::{
        f16_bits_to_f32, f32_to_f16_bits, p010_word, scrgb_to_p010_chroma, scrgb_to_p010_luma,
        scrgb_to_sdr8,
    };
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_11_0};
    use windows::Win32::Graphics::Direct3D11::{
        D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
        D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_USAGE_STAGING, D3D11CreateDevice,
    };

    const W: u32 = 8;
    const H: u32 = 4;

    /// Software rasteriser device: no GPU, no VDD.
    fn warp() -> (ID3D11Device, ID3D11DeviceContext) {
        let (mut device, mut ctx) = (None, None);
        // SAFETY: valid out-pointers and level list.
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_WARP,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut ctx),
            )
        }
        .expect("WARP device");
        (device.unwrap(), ctx.unwrap())
    }

    /// Known scRGB pixels: black, SDR white, 200 / 480 / 1000 / 10000 nit whites, 100 nits of
    /// BT.2020 red, mid grey, out-of-gamut negatives, a bright orange, a saturated blue.
    fn pixels() -> Vec<[f32; 3]> {
        let k = 100.0 / 80.0;
        let mut v = vec![
            [0.0; 3],
            [1.0; 3],
            [2.5; 3],
            [6.0; 3],
            [12.5; 3],
            [125.0; 3],
            [1.660_491 * k, -0.124_550 * k, -0.018_151 * k],
            [0.18; 3],
            [-0.5, -0.2, 0.7],
            [10.0, 5.0, 0.0],
            [0.0, 0.0, 3.0],
            [0.5, 1.5, 0.25],
        ];
        // A deterministic spread for the rest.
        let mut s = 12345u32;
        while v.len() < (W * H) as usize {
            let mut next = || {
                s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (s >> 8) as f32 / (1 << 24) as f32
            };
            v.push([next() * 4.0, next() * 4.0, next() * 4.0]);
        }
        // The GPU sees the FP16 texture, so the reference must too: near zero PQ is steep enough
        // that the FP16 rounding of a saturated colour moves the result by several codes.
        for px in &mut v {
            for c in px.iter_mut() {
                *c = f16_bits_to_f32(f32_to_f16_bits(*c));
            }
        }
        v
    }

    fn upload_fp16(device: &ID3D11Device, px: &[[f32; 3]]) -> ID3D11Texture2D {
        let mut data = Vec::with_capacity(px.len() * 4);
        for p in px {
            for c in p {
                data.push(f32_to_f16_bits(*c));
            }
            data.push(f32_to_f16_bits(1.0));
        }
        let desc = D3D11_TEXTURE2D_DESC {
            Width: W,
            Height: H,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_R16G16B16A16_FLOAT,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let init = D3D11_SUBRESOURCE_DATA {
            pSysMem: data.as_ptr().cast(),
            SysMemPitch: W * 8,
            SysMemSlicePitch: 0,
        };
        let mut tex = None;
        // SAFETY: `data` outlives the call.
        unsafe { device.CreateTexture2D(&desc, Some(&init), Some(&mut tex)) }.unwrap();
        tex.unwrap()
    }

    /// Reads texture `t` (`bpp` bytes per texel, `w x h`) back as bytes, rows tightly packed.
    fn read_back(
        device: &ID3D11Device,
        ctx: &ID3D11DeviceContext,
        t: &ID3D11Texture2D,
        w: u32,
        h: u32,
        bpp: u32,
    ) -> Vec<u8> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: valid out-parameter.
        unsafe { t.GetDesc(&mut desc) };
        desc.Usage = D3D11_USAGE_STAGING;
        desc.BindFlags = 0;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        let mut staging = None;
        // SAFETY: valid descriptor; the mapping is released before returning.
        unsafe {
            device
                .CreateTexture2D(&desc, None, Some(&mut staging))
                .unwrap();
            let staging = staging.unwrap();
            ctx.CopyResource(&staging, t);
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut m))
                .unwrap();
            let mut out = Vec::new();
            for row in 0..h as usize {
                let p = (m.pData as *const u8).add(row * m.RowPitch as usize);
                out.extend_from_slice(std::slice::from_raw_parts(p, (w * bpp) as usize));
            }
            ctx.Unmap(&staging, 0);
            out
        }
    }

    fn words(bytes: &[u8]) -> Vec<u16> {
        bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect()
    }

    fn check_planes(y: &[u16], uv: &[u16], px: &[[f32; 3]], what: &str) {
        for i in 0..(W * H) as usize {
            let want = i32::from(p010_word(scrgb_to_p010_luma(px[i])));
            let d = (i32::from(y[i]) - want).abs() >> 6;
            assert!(
                d <= 2,
                "{what}: luma {i} {:?}: got {} want {want}",
                px[i],
                y[i]
            );
        }
        for cy in 0..(H / 2) as usize {
            for cx in 0..(W / 2) as usize {
                let at = |dx: usize, dy: usize| px[(cy * 2 + dy) * W as usize + cx * 2 + dx];
                let (cb, cr) = scrgb_to_p010_chroma([at(0, 0), at(1, 0), at(0, 1), at(1, 1)]);
                let i = (cy * (W / 2) as usize + cx) * 2;
                for (got, want, n) in [(uv[i], cb, "Cb"), (uv[i + 1], cr, "Cr")] {
                    let d = (i32::from(got) - i32::from(p010_word(want))).abs() >> 6;
                    assert!(
                        d <= 2,
                        "{what}: {n} ({cx},{cy}): got {got} want {}",
                        p010_word(want)
                    );
                }
            }
        }
    }

    #[test]
    fn shaders_compile() {
        let (device, _) = warp();
        ScrgbToP010::new(&device, W, H).expect("P010 shaders");
        ScrgbToSdr::new(&device, W, H, 80.0).expect("SDR shader");
    }

    #[test]
    fn scrgb_to_p010_matches_reference_on_plain_planes() {
        let (device, ctx) = warp();
        let px = pixels();
        let src = upload_fp16(&device, &px);
        let bind = (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32;
        let ytex = create_texture(&device, W, H, DXGI_FORMAT_R16_UNORM, bind).unwrap();
        let uvtex = create_texture(&device, W / 2, H / 2, DXGI_FORMAT_R16G16_UNORM, bind).unwrap();
        let (mut yv, mut uvv) = (None, None);
        // SAFETY: valid resources, whole-texture views.
        unsafe {
            device
                .CreateRenderTargetView(&ytex, None, Some(&mut yv))
                .unwrap();
            device
                .CreateRenderTargetView(&uvtex, None, Some(&mut uvv))
                .unwrap();
        }
        let mut conv = ScrgbToP010::new(&device, W, H).unwrap();
        conv.convert_views(&src, &yv.unwrap(), &uvv.unwrap())
            .unwrap();
        let y = words(&read_back(&device, &ctx, &ytex, W, H, 2));
        let uv = words(&read_back(&device, &ctx, &uvtex, W / 2, H / 2, 4));
        check_planes(&y, &uv, &px, "plain planes");
    }

    /// The real destination: plane views of a P010 texture. Skipped when this WARP cannot
    /// render to P010 (the plane-view test above still covers the shader math).
    #[test]
    fn scrgb_to_p010_matches_reference_on_p010() {
        let (device, ctx) = warp();
        let px = pixels();
        let src = upload_fp16(&device, &px);
        let Ok(dst) = create_p010_texture(&device, W, H) else {
            eprintln!("skipping: WARP cannot create a P010 render target");
            return;
        };
        let mut conv = ScrgbToP010::new(&device, W, H).unwrap();
        if let Err(e) = conv.convert(&src, &dst, 0) {
            eprintln!("skipping: WARP cannot render to P010 planes: {e}");
            return;
        }
        // Planar read-back: luma rows, then interleaved chroma rows, both `W * 2` bytes wide.
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: valid out-parameter.
        unsafe { dst.GetDesc(&mut desc) };
        desc.Usage = D3D11_USAGE_STAGING;
        desc.BindFlags = 0;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        let mut staging = None;
        // SAFETY: valid descriptor; the mapping is released before leaving the block.
        let (y, uv) = unsafe {
            device
                .CreateTexture2D(&desc, None, Some(&mut staging))
                .unwrap();
            let staging = staging.unwrap();
            ctx.CopyResource(&staging, &dst);
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut m))
                .unwrap();
            let pitch = m.RowPitch as usize;
            let base = m.pData as *const u8;
            let mut y = Vec::new();
            for r in 0..H as usize {
                y.extend_from_slice(std::slice::from_raw_parts(
                    base.add(r * pitch),
                    (W * 2) as usize,
                ));
            }
            let mut uv = Vec::new();
            for r in 0..(H / 2) as usize {
                uv.extend_from_slice(std::slice::from_raw_parts(
                    base.add((H as usize + r) * pitch),
                    (W * 2) as usize,
                ));
            }
            ctx.Unmap(&staging, 0);
            (words(&y), words(&uv))
        };
        check_planes(&y, &uv, &px, "P010");
    }

    #[test]
    fn scrgb_to_sdr_matches_reference() {
        for white in [80.0f32, 200.0, 480.0] {
            let (device, ctx) = warp();
            let px = pixels();
            let src = upload_fp16(&device, &px);
            let (dst, rtv) = create_sdr_target(&device, W, H).unwrap();
            // Start at another level to exercise the live update.
            let mut conv = ScrgbToSdr::new(&device, W, H, 80.0).unwrap();
            conv.set_white_nits(white);
            conv.convert(&src, &rtv).unwrap();
            let got = read_back(&device, &ctx, &dst, W, H, 4);
            for (i, p) in px.iter().enumerate() {
                let want = scrgb_to_sdr8(*p, white);
                // BGRA order in memory.
                let g = [got[i * 4 + 2], got[i * 4 + 1], got[i * 4]];
                for c in 0..3 {
                    assert!(
                        (i32::from(g[c]) - i32::from(want[c])).abs() <= 1,
                        "white {white}, pixel {i} {p:?}: got {g:?} want {want:?}"
                    );
                }
            }
        }
    }
}
