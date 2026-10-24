//! The Glass theme's renderer (GPU only): liquid glass for the island body.
//!
//! The screen copy (services::backdrop) is uploaded to a texture and frosted
//! twice with Direct2D's Gaussian blur (a light and a heavy frost, mixed by
//! the glass's thickness). Before each frame the shader in res/glass.hlsl
//! draws the glass, its rim light and its shadow into a texture the size of
//! the window; the frame then draws that texture where Classic paints its
//! black body, and the pages on top as usual.
//!
//! The shader is compiled at run time with Windows' own d3dcompiler_47.dll
//! from System32 (the way gfx.rs loads dcomp.dll). Anything that fails here
//! leaves the island in smoked glass (gfx::Canvas::glass), never blank.

use poni_core::geom::{Color, Rect};
use std::ffi::c_void;
use std::mem::ManuallyDrop;
use windows::core::{s, w, Interface, Result, HRESULT, PCSTR};
use windows::Win32::Foundation::{E_FAIL, E_NOTIMPL, RECT};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::IDXGISurface;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};

const SHADER: &str = include_str!("../res/glass.hlsl");
/// Light and heavy frost, in DIPs of blur (standard deviation).
const FROST_LO: f32 = 3.0;
const FROST_HI: f32 = 9.0;

/// Everything the glass needs for one frame. Lengths are DIPs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassParams {
    /// The island body and its outline (as `gfx::notch_segments`).
    pub body: Rect,
    pub radius: f32,
    pub shoulder: f32,
    /// Rim band width and how far its edge looks inward (poni_core::glass).
    pub bevel: f32,
    pub mag: f32,
    /// 0 = light frost (thin glass) .. 1 = heavy frost (thick glass).
    pub frost_mix: f32,
    /// 0 = dark glass .. 1 = light glass.
    pub light_mix: f32,
    /// Tint toward black (dark glass) and toward white (light glass).
    pub tint_dark: f32,
    pub tint_light: f32,
    /// Smoked glass (no screen copy): the looks' grounds.
    pub ground_dark: Color,
    pub ground_light: Color,
    /// Rim light and inner glow strength, and where the light comes from.
    pub rim: f32,
    pub glow: f32,
    pub light_dir: (f32, f32),
    pub shadow_alpha: f32,
    pub shadow_reach: f32,
    pub shadow_drop: f32,
    pub saturation: f32,
    pub dispersion: f32,
    /// Lens strength (1; the self-test turns it off to see the bend).
    pub lens: f32,
    /// Sample the screen copy (false = smoked glass even with a copy).
    pub use_copy: bool,
    /// Draw the probe pixels the screen copy is checked for.
    pub probe: bool,
    /// Where the screen copy's top-left is, in this window's pixels (it was
    /// taken before the window last moved or grew).
    pub copy_origin: (f32, f32),
    /// The probe's first pixel in the window's top row (the copy is checked
    /// at the same place).
    pub probe_x: i32,
}

/// The probe: four top-row pixels in the colour of the first, nudged by these
/// steps (r, g, b, in 8-bit units). Invisible, and too exact to happen by
/// chance in what's under the island.
pub const PROBE_STEPS: [[i16; 3]; 4] = [[0, 0, 0], [2, -1, 1], [-1, 2, -1], [1, 1, -2]];

/// Where the probe starts in the window's top row (pixels).
pub fn probe_x(body: Rect, scale: f32) -> i32 {
    ((body.x + body.w / 2.0) * scale).floor() as i32 - 2
}

/// Whether a screen copy (BGRA, `w` wide) shows the probe at `x`: Windows put
/// the island into its own copy.
pub fn probe_in(px: &[u8], w: i32, x: i32) -> bool {
    if x < 0 || x + 4 > w {
        return false;
    }
    let rgb = |i: usize| px.get((x as usize + i) * 4..(x as usize + i) * 4 + 3).map(|p| [p[2] as i16, p[1] as i16, p[0] as i16]);
    let Some(base) = rgb(0) else { return false };
    // Near black or white the nudges clip; the probe can't be read there.
    if base.iter().any(|&v| !(3..=252).contains(&v)) {
        return false;
    }
    (1..4).all(|i| rgb(i).is_some_and(|c| (0..3).all(|k| c[k] - base[k] == PROBE_STEPS[i][k])))
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Consts {
    body: [f32; 4],
    shape: [f32; 4],
    lens: [f32; 4],
    tint_dark: [f32; 4],
    tint_light: [f32; 4],
    light: [f32; 4],
    shadow: [f32; 4],
    copy: [f32; 4],
    probe: [f32; 4],
    origin: [f32; 4],
}

type D3DCompileFn = unsafe extern "system" fn(
    *const c_void,
    usize,
    PCSTR,
    *const c_void,
    *const c_void,
    PCSTR,
    PCSTR,
    u32,
    u32,
    *mut *mut c_void,
    *mut *mut c_void,
) -> HRESULT;

/// Compile `entry` of the glass shader for `target` with System32's compiler.
unsafe fn compile(entry: PCSTR, target: PCSTR) -> Result<Vec<u8>> {
    let lib = LoadLibraryExW(w!("d3dcompiler_47.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32)?;
    let Some(f) = GetProcAddress(lib, s!("D3DCompile")) else {
        return Err(E_NOTIMPL.into());
    };
    let f: D3DCompileFn = std::mem::transmute(f);
    // D3DCOMPILE_ENABLE_STRICTNESS | D3DCOMPILE_OPTIMIZATION_LEVEL3
    let flags = (1 << 11) | (1 << 15);
    let (mut code, mut errors) = (std::ptr::null_mut(), std::ptr::null_mut());
    let hr = f(SHADER.as_ptr() as *const c_void, SHADER.len(), s!("glass.hlsl"), std::ptr::null(), std::ptr::null(), entry, target, flags, 0, &mut code, &mut errors);
    if !errors.is_null() {
        let e = ID3DBlob::from_raw(errors);
        let msg = std::slice::from_raw_parts(e.GetBufferPointer() as *const u8, e.GetBufferSize());
        let msg = String::from_utf8_lossy(msg);
        if hr.is_err() {
            crate::pwarn!("glass: shader didn't compile: {}", msg.trim());
        }
    }
    hr.ok()?;
    if code.is_null() {
        return Err(E_FAIL.into());
    }
    let blob = ID3DBlob::from_raw(code);
    Ok(std::slice::from_raw_parts(blob.GetBufferPointer() as *const u8, blob.GetBufferSize()).to_vec())
}

/// A texture the shader reads, and Direct2D's view of it.
struct Tex {
    tex: ID3D11Texture2D,
    srv: ID3D11ShaderResourceView,
    bmp: ID2D1Bitmap1,
}

unsafe fn texture(d3d: &ID3D11Device, dc: &ID2D1DeviceContext, w: i32, h: i32, target: bool, dpi: f32) -> Result<Tex> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: w as u32,
        Height: h as u32,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | if target { D3D11_BIND_RENDER_TARGET.0 } else { 0 }) as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let mut tex = None;
    d3d.CreateTexture2D(&desc, None, Some(&mut tex))?;
    let tex = tex.ok_or(windows::core::Error::from(E_FAIL))?;
    let mut srv = None;
    d3d.CreateShaderResourceView(&tex, None, Some(&mut srv))?;
    let srv = srv.ok_or(windows::core::Error::from(E_FAIL))?;
    let surface: IDXGISurface = tex.cast()?;
    let props = D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
        dpiX: dpi,
        dpiY: dpi,
        bitmapOptions: if target { D2D1_BITMAP_OPTIONS_TARGET } else { D2D1_BITMAP_OPTIONS_NONE },
        colorContext: ManuallyDrop::new(None),
    };
    let bmp = dc.CreateBitmapFromDxgiSurface(&surface, Some(&props))?;
    Ok(Tex { tex, srv, bmp })
}

/// The screen copy and its two frosts.
struct Copy {
    sharp: Tex,
    lo: Tex,
    hi: Tex,
    w: i32,
    h: i32,
    /// Pixels per DIP the frosts were blurred for.
    scale: f32,
}

/// Blur `c`'s copy into its light and heavy frost.
unsafe fn blur_into(blur: &ID2D1Effect, dc: &ID2D1DeviceContext, c: &Copy, scale: f32) -> Result<()> {
    blur.SetInput(0, &c.sharp.bmp, true);
    let output = blur.GetOutput()?;
    let mut r = Ok(());
    for (target, sigma) in [(&c.lo.bmp, FROST_LO), (&c.hi.bmp, FROST_HI)] {
        r = blur.SetValue(D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32, D2D1_PROPERTY_TYPE_FLOAT, &(sigma * scale).to_le_bytes());
        if r.is_err() {
            break;
        }
        dc.SetTarget(target);
        dc.SetDpi(96.0, 96.0);
        dc.BeginDraw();
        dc.DrawImage(&output, None, None, D2D1_INTERPOLATION_MODE_LINEAR, D2D1_COMPOSITE_MODE_SOURCE_COPY);
        r = dc.EndDraw(None, None);
        dc.SetTarget(None::<&ID2D1Image>);
        if r.is_err() {
            break;
        }
    }
    blur.SetInput(0, None::<&ID2D1Image>, true);
    r
}

/// The texture the glass is drawn into.
struct Out {
    rtv: ID3D11RenderTargetView,
    tex: Tex,
    w: i32,
    h: i32,
    dpi: f32,
}

pub struct GlassPass {
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    cb: ID3D11Buffer,
    sampler: ID3D11SamplerState,
    blur: ID2D1Effect,
    copy: Option<Copy>,
    out: Option<Out>,
}

impl GlassPass {
    pub unsafe fn new(d3d: &ID3D11Device, dc: &ID2D1DeviceContext) -> Result<Self> {
        if d3d.GetFeatureLevel().0 < D3D_FEATURE_LEVEL_10_0.0 {
            return Err(E_NOTIMPL.into());
        }
        let vs_code = compile(s!("vs_main"), s!("vs_4_0"))?;
        let ps_code = compile(s!("ps_main"), s!("ps_4_0"))?;
        let mut vs = None;
        d3d.CreateVertexShader(&vs_code, None, Some(&mut vs))?;
        let mut ps = None;
        d3d.CreatePixelShader(&ps_code, None, Some(&mut ps))?;
        let desc = D3D11_BUFFER_DESC {
            ByteWidth: std::mem::size_of::<Consts>() as u32,
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
            StructureByteStride: 0,
        };
        let mut cb = None;
        d3d.CreateBuffer(&desc, None, Some(&mut cb))?;
        let sdesc = D3D11_SAMPLER_DESC {
            Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
            AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
            MipLODBias: 0.0,
            MaxAnisotropy: 1,
            ComparisonFunc: D3D11_COMPARISON_NEVER,
            BorderColor: [0.0; 4],
            MinLOD: 0.0,
            MaxLOD: f32::MAX,
        };
        let mut sampler = None;
        d3d.CreateSamplerState(&sdesc, Some(&mut sampler))?;
        let blur = dc.CreateEffect(&CLSID_D2D1GaussianBlur)?;
        blur.SetValue(D2D1_GAUSSIANBLUR_PROP_BORDER_MODE.0 as u32, D2D1_PROPERTY_TYPE_ENUM, &(D2D1_BORDER_MODE_HARD.0 as u32).to_le_bytes())?;
        let none = || windows::core::Error::from(E_FAIL);
        Ok(Self {
            vs: vs.ok_or_else(none)?,
            ps: ps.ok_or_else(none)?,
            cb: cb.ok_or_else(none)?,
            sampler: sampler.ok_or_else(none)?,
            blur,
            copy: None,
            out: None,
        })
    }

    pub fn has_copy(&self) -> bool {
        self.copy.is_some()
    }

    pub fn clear_copy(&mut self) {
        self.copy = None;
    }

    /// Upload a screen copy (BGRA, `w`×`h`, the window's own rectangle) and
    /// frost it for `scale` pixels per DIP.
    pub unsafe fn set_copy(&mut self, d3d: &ID3D11Device, ctx: &ID3D11DeviceContext, dc: &ID2D1DeviceContext, px: &[u8], w: i32, h: i32, scale: f32) -> Result<()> {
        if w <= 0 || h <= 0 || px.len() < (w * h * 4) as usize {
            return Err(E_FAIL.into());
        }
        if !self.copy.as_ref().is_some_and(|c| c.w == w && c.h == h) {
            self.copy = None;
            let sharp = texture(d3d, dc, w, h, false, 96.0)?;
            let lo = texture(d3d, dc, w, h, true, 96.0)?;
            let hi = texture(d3d, dc, w, h, true, 96.0)?;
            self.copy = Some(Copy { sharp, lo, hi, w, h, scale: 0.0 });
        }
        let Some(c) = self.copy.as_mut() else { return Err(E_FAIL.into()) };
        ctx.UpdateSubresource(&c.sharp.tex, 0, None, px.as_ptr() as *const c_void, (w * 4) as u32, 0);
        c.scale = 0.0;
        self.frost(dc, scale)
    }

    /// Blur the copy into the light and heavy frosts (again when the DPI
    /// changed since).
    unsafe fn frost(&mut self, dc: &ID2D1DeviceContext, scale: f32) -> Result<()> {
        let r = match self.copy.as_ref() {
            Some(c) if c.scale != scale => blur_into(&self.blur, dc, c, scale),
            _ => return Ok(()),
        };
        match r {
            Ok(()) => {
                if let Some(c) = self.copy.as_mut() {
                    c.scale = scale;
                }
                Ok(())
            }
            Err(e) => {
                self.copy = None;
                Err(e)
            }
        }
    }

    /// Draw the glass for `p` into the output texture, inside `area` (window
    /// pixels). The frame then draws [`GlassPass::output`] over the same area.
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn render(
        &mut self,
        d3d: &ID3D11Device,
        ctx: &ID3D11DeviceContext,
        dc: &ID2D1DeviceContext,
        surf_w: i32,
        surf_h: i32,
        dpi: f32,
        p: &GlassParams,
        area: RECT,
    ) -> Result<()> {
        let scale = dpi / 96.0;
        if self.copy.as_ref().is_some_and(|c| c.scale != scale) {
            self.frost(dc, scale)?;
        }
        if !self.out.as_ref().is_some_and(|o| o.w == surf_w && o.h == surf_h && o.dpi == dpi) {
            self.out = None;
            let tex = texture(d3d, dc, surf_w, surf_h, true, dpi)?;
            let mut rtv = None;
            d3d.CreateRenderTargetView(&tex.tex, None, Some(&mut rtv))?;
            let rtv = rtv.ok_or(windows::core::Error::from(E_FAIL))?;
            self.out = Some(Out { rtv, tex, w: surf_w, h: surf_h, dpi });
        }
        let Some(out) = self.out.as_ref() else { return Err(E_FAIL.into()) };
        let use_copy = p.use_copy && self.copy.is_some();
        let (cw, ch) = self.copy.as_ref().map_or((1, 1), |c| (c.w, c.h));
        let b = p.body;
        let r = p.radius.min(b.h).min(b.w / 2.0).max(0.0);
        let k = Consts {
            body: [b.x * scale, b.y * scale, (b.x + b.w) * scale, (b.y + b.h) * scale],
            shape: [r * scale, p.shoulder.max(0.0) * scale, scale, if use_copy { 1.0 } else { 0.0 }],
            lens: [(p.bevel * scale).max(1.0), p.mag * scale, poni_core::glass::RIM_POWER, p.frost_mix.clamp(0.0, 1.0)],
            tint_dark: [p.ground_dark.r, p.ground_dark.g, p.ground_dark.b, p.tint_dark],
            tint_light: [p.ground_light.r, p.ground_light.g, p.ground_light.b, p.tint_light],
            light: [p.light_dir.0, p.light_dir.1, p.rim, p.glow],
            shadow: [p.shadow_alpha, (p.shadow_reach * scale).max(1.0), p.shadow_drop * scale, p.light_mix.clamp(0.0, 1.0)],
            copy: [cw as f32, ch as f32, p.saturation, p.dispersion],
            probe: [p.probe_x as f32, if p.probe { 1.0 } else { 0.0 }, p.lens, if use_copy { 1.0 } else { poni_core::glass::SMOKED_OPACITY }],
            origin: [p.copy_origin.0, p.copy_origin.1, 0.0, 0.0],
        };
        ctx.UpdateSubresource(&self.cb, 0, None, &k as *const Consts as *const c_void, 0, 0);
        let (x0, y0) = (area.left.clamp(0, surf_w), area.top.clamp(0, surf_h));
        let (x1, y1) = (area.right.clamp(x0, surf_w), area.bottom.clamp(y0, surf_h));
        if x1 <= x0 || y1 <= y0 {
            return Ok(());
        }
        let vp = D3D11_VIEWPORT { TopLeftX: x0 as f32, TopLeftY: y0 as f32, Width: (x1 - x0) as f32, Height: (y1 - y0) as f32, MinDepth: 0.0, MaxDepth: 1.0 };
        ctx.OMSetRenderTargets(Some(&[Some(out.rtv.clone())]), None::<&ID3D11DepthStencilView>);
        ctx.RSSetViewports(Some(&[vp]));
        ctx.RSSetState(None::<&ID3D11RasterizerState>);
        ctx.OMSetBlendState(None::<&ID3D11BlendState>, None, 0xffff_ffff);
        ctx.IASetInputLayout(None::<&ID3D11InputLayout>);
        ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        ctx.VSSetShader(&self.vs, None);
        ctx.PSSetShader(&self.ps, None);
        let views = match self.copy.as_ref() {
            Some(c) if use_copy => [Some(c.sharp.srv.clone()), Some(c.lo.srv.clone()), Some(c.hi.srv.clone())],
            _ => [None, None, None],
        };
        ctx.PSSetShaderResources(0, Some(&views));
        ctx.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
        ctx.PSSetConstantBuffers(0, Some(&[Some(self.cb.clone())]));
        ctx.Draw(3, 0);
        ctx.PSSetShaderResources(0, Some(&[None, None, None]));
        ctx.OMSetRenderTargets(None, None::<&ID3D11DepthStencilView>);
        let _ = d3d;
        Ok(())
    }

    /// What [`GlassPass::render`] drew, for Direct2D (at the frame's DPI).
    pub fn output(&self) -> Option<&ID2D1Bitmap1> {
        self.out.as_ref().map(|o| &o.tex.bmp)
    }
}

// ---------------------------------------------------------------- the theme at run time

use crate::events::EventSink;
use crate::gfx::Gfx;
use crate::services::backdrop::{Backdrop, Frame, Status};
use crate::window;
use poni_core::glass::{shadow_alpha, thickness, tint_for_contrast, Flip};
use poni_core::look::{GLASS_DARK, GLASS_LIGHT};
use std::sync::Arc;
use windows::Win32::Foundation::HWND;

/// Windows 10 version 2004: the first build that can leave a window out of
/// screen capture (older ones black it out, and the copy would show a hole).
const EXCLUDE_FROM_CAPTURE_BUILD: u32 = 19041;
/// How long the glass takes to turn from dark to light or back (seconds).
const FLIP_TIME: f32 = 0.2;

/// The Glass theme while it runs: the screen copy, what's under the island,
/// light or dark, and this frame's glass.
pub struct Glass {
    copy: Option<Backdrop>,
    /// The monitor the copy is of.
    monitor: isize,
    rect: RECT,
    /// The island is left out of screen capture.
    excluded: bool,
    /// The newest copy, kept to look at what's under the island and to upload
    /// again after a GPU reset.
    frame: Option<Arc<Frame>>,
    under: Option<poni_core::glass::Backdrop>,
    under_px: RECT,
    flip: Flip,
    flip_seeded: bool,
    /// Windows apps are in light mode (the open island follows it).
    pub app_light: bool,
    light_mix: f32,
    /// Why the copy stopped for good (smoked glass from then on).
    stopped: Option<String>,
    probe_hits: u32,
    /// Where this window draws its probe (set each frame).
    probe_px: i32,
    /// Jump straight to light or dark on the next frame (a new self-test scene).
    snap: bool,
    /// The self-test drew the backdrop itself: no screen copy, no exclusion.
    injected: bool,
    /// Self-test switches: lens strength (1) and whether to use the copy.
    pub lens: f32,
    pub use_copy: bool,
}

impl Glass {
    pub fn new(app_light: bool) -> Self {
        Self {
            copy: None,
            monitor: 0,
            rect: RECT::default(),
            excluded: false,
            frame: None,
            under: None,
            under_px: RECT::default(),
            flip: Flip::new(false),
            flip_seeded: false,
            app_light,
            light_mix: if app_light { 1.0 } else { 0.0 },
            stopped: None,
            probe_hits: 0,
            probe_px: 0,
            snap: false,
            injected: false,
            lens: 1.0,
            use_copy: true,
        }
    }

    /// Start or stop the screen copy: `on` while the theme is Glass and the
    /// GPU renderer draws (smoked glass needs no copy). `rect` is the window
    /// in screen pixels.
    pub fn sync(&mut self, on: bool, hwnd: HWND, sink: &EventSink, monitor: isize, rect: RECT, gfx: &mut Gfx) {
        if self.injected {
            self.rect = rect;
        }
        if !on || self.injected {
            if self.copy.take().is_some() {
                crate::pinfo!("glass: screen copy off");
            }
            // Off (theme changed, island hidden, PC locked): the next start
            // gets a clean slate, even after giving up.
            self.stopped = None;
            self.probe_hits = 0;
            if self.excluded {
                window::set_capture_excluded(hwnd, false);
                self.excluded = false;
            }
            if !self.injected && self.frame.take().is_some() {
                gfx.glass_clear_copy();
                self.under = None;
            }
            return;
        }
        if self.copy.is_some() && (monitor != self.monitor || rect != self.rect) {
            if monitor != self.monitor {
                // Another display: duplicate that one instead.
                self.copy = None;
                self.frame = None;
                self.under = None;
                gfx.glass_clear_copy();
            } else if let Some(c) = &self.copy {
                // The window moved or grew: keep showing the last copy where it
                // was taken until the new one arrives.
                c.set_rect(rect);
                self.under = None;
            }
            self.rect = rect;
        }
        if self.copy.is_some() || self.stopped.is_some() {
            return;
        }
        match window::windows_build() {
            Some(b) if b >= EXCLUDE_FROM_CAPTURE_BUILD => {}
            b => {
                self.stop_for(hwnd, gfx, format!("Windows build {} can't leave a window out of capture", b.map_or("unknown".into(), |b| b.to_string())));
                return;
            }
        }
        if !self.excluded {
            if !window::set_capture_excluded(hwnd, true) {
                self.stop_for(hwnd, gfx, "Windows didn't leave the island out of capture".into());
                return;
            }
            self.excluded = true;
        }
        self.monitor = monitor;
        self.rect = rect;
        self.copy = Some(Backdrop::start(sink.clone(), monitor, rect));
    }

    fn stop_for(&mut self, hwnd: HWND, gfx: &mut Gfx, why: String) {
        crate::pinfo!("glass: smoked glass ({why})");
        self.stopped = Some(why);
        self.copy = None;
        if self.excluded {
            window::set_capture_excluded(hwnd, false);
            self.excluded = false;
        }
        self.frame = None;
        self.under = None;
        gfx.glass_clear_copy();
    }

    /// A message from the copy thread: take the newest copy. True when the
    /// island should redraw.
    pub fn on_backdrop(&mut self, hwnd: HWND, gfx: &mut Gfx) -> bool {
        let probe_x = self.probe_px;
        let Some(c) = self.copy.as_mut() else { return false };
        if let Status::Failed(why) = c.status() {
            self.stop_for(hwnd, gfx, why);
            return true;
        }
        let Some(f) = c.take_new() else { return false };
        if probe_in(&f.px, f.w, probe_x) {
            // Seen twice in a row: Windows does put the island into the copy.
            self.probe_hits += 1;
            if self.probe_hits >= 2 {
                self.stop_for(hwnd, gfx, "Windows put the island into its own screen copy".into());
                return true;
            }
            return false;
        }
        self.probe_hits = 0;
        gfx.glass_copy(&f.px, f.w, f.h);
        self.frame = Some(f);
        self.under = None;
        true
    }

    /// Self-test: use `px` (BGRA, the window's size) as what's under the window.
    pub fn inject(&mut self, px: Vec<u8>, w: i32, h: i32, gfx: &mut Gfx) {
        self.injected = true;
        self.copy = None;
        gfx.glass_copy(&px, w, h);
        self.frame = Some(Arc::new(Frame { w, h, px, left: self.rect.left, top: self.rect.top, generation: 0 }));
        self.under = None;
        self.flip_seeded = false;
    }

    /// Start over on the next frame's backdrop (self-test, between scenes).
    pub fn reseed(&mut self) {
        self.flip_seeded = false;
        self.snap = true;
    }

    /// This frame's glass for an island `body` (DIPs) with corner `radius`.
    /// `small`: a collapsed pill, which flips with what's under it; the open
    /// island follows the Windows mode. Returns the light look's choice, the
    /// glass, and whether the glass is still changing (keep drawing).
    #[allow(clippy::too_many_arguments)]
    pub fn frame(&mut self, body: Rect, radius: f32, shoulder: f32, small: bool, now: f64, dt: f32, scale: f32, gfx: &mut Gfx) -> (bool, GlassParams, bool) {
        // A GPU reset loses the copy: upload the last one again.
        if let Some(f) = self.frame.as_ref() {
            if !gfx.glass_has_copy() {
                gfx.glass_copy(&f.px, f.w, f.h);
            }
        }
        let has_copy = gfx.glass_has_copy();
        // The copy may predate the window's last move: where it sits now.
        let origin = match self.frame.as_ref() {
            Some(f) if !self.injected => (f.left - self.rect.left, f.top - self.rect.top),
            _ => (0, 0),
        };
        let px = RECT {
            left: (body.x * scale).floor() as i32 - origin.0,
            top: (body.y * scale).floor() as i32 - origin.1,
            right: ((body.x + body.w) * scale).ceil() as i32 - origin.0,
            bottom: ((body.y + body.h) * scale).ceil() as i32 - origin.1,
        };
        if self.under.is_none() || px != self.under_px {
            self.under = self.frame.as_ref().and_then(|f| poni_core::glass::sample_backdrop(&f.px, f.w as usize * 4, (px.left, px.top, px.right, px.bottom), (2.0 * scale).max(2.0) as usize));
            self.under_px = px;
        }
        let under = self.under.filter(|_| has_copy || self.frame.is_some());
        let mut pending = false;
        let light = match (small, under) {
            (true, Some(u)) => {
                if self.flip_seeded {
                    self.flip.update(u.lum, now);
                } else {
                    self.flip.settle(u.lum);
                    self.flip_seeded = true;
                }
                // The hold needs frames to run out.
                pending = self.flip.pending();
                self.flip.light
            }
            _ => self.app_light,
        };
        let target = if light { 1.0 } else { 0.0 };
        let step = if std::mem::take(&mut self.snap) { 1.0 } else { dt / FLIP_TIME };
        self.light_mix = if self.light_mix < target { (self.light_mix + step).min(target) } else { (self.light_mix - step).max(target) };
        let th = thickness(body.h);
        self.probe_px = probe_x(body, scale);
        let ink_dark = [GLASS_LIGHT.ink.r, GLASS_LIGHT.ink.g, GLASS_LIGHT.ink.b];
        let (tint_dark, tint_light, busy) = match under {
            Some(u) => {
                // The material as the shader makes it: frosted, then saturated.
                let l = 0.2126 * u.mean[0] + 0.7152 * u.mean[1] + 0.0722 * u.mean[2];
                let m = u.mean.map(|c| (l + (c - l) * SATURATION).clamp(0.0, 1.0));
                (
                    tint_for_contrast(m, [1.0; 3], [0.0; 3], 0.34, 0.85, 4.5),
                    tint_for_contrast(m, ink_dark, [0.985; 3], 0.42, 0.88, 4.5),
                    u.busy,
                )
            }
            None => (0.34, 0.42, 0.0),
        };
        let mix = self.light_mix;
        let p = GlassParams {
            body,
            radius,
            shoulder,
            bevel: th.bevel,
            mag: th.mag,
            frost_mix: th.t,
            light_mix: mix,
            tint_dark,
            tint_light,
            ground_dark: GLASS_DARK.ground,
            ground_light: GLASS_LIGHT.ground,
            rim: 0.80 + 0.15 * mix,
            glow: 0.09 + 0.01 * mix,
            light_dir: (-0.6, -0.8),
            shadow_alpha: shadow_alpha(busy, th.t),
            shadow_reach: th.shadow_reach,
            shadow_drop: th.shadow_drop,
            saturation: SATURATION,
            dispersion: 0.03,
            lens: self.lens,
            use_copy: self.use_copy && has_copy,
            probe: self.copy.is_some(),
            copy_origin: (origin.0 as f32, origin.1 as f32),
            probe_x: self.probe_px,
        };
        (mix >= 0.5, p, mix != target || pending)
    }

    /// Everything off (leaving the theme, quitting).
    pub fn stop(&mut self, hwnd: HWND, gfx: &mut Gfx) {
        self.injected = false;
        self.copy = None;
        if self.excluded {
            window::set_capture_excluded(hwnd, false);
            self.excluded = false;
        }
        self.frame = None;
        self.under = None;
        gfx.glass_clear_copy();
    }
}

/// How much the glass boosts colour (Apple's glass is vivid, not grey).
const SATURATION: f32 = 1.45;
