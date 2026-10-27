//! Rendering: Direct2D on the GPU, presented through DirectComposition, with
//! a software fallback that pushes a DIB with `UpdateLayeredWindowIndirect`.
//!
//! The island window is a fixed-size, per-pixel-alpha layered window that is
//! big enough for the largest island (the widest, tallest expanded page plus
//! spring overshoot). The island is drawn centred in it, so nothing moves or
//! resizes while it morphs: its centre stays on one pixel and its edges glide
//! at sub-pixel precision.
//!
//! GPU renderer (any Direct3D 11 device, WARP included): frames go to a
//! premultiplied-alpha flip-model swap chain that DWM composes on the vblank,
//! with no CPU copies. DWM shows them in a window of their own that takes
//! no input (`window::create_visual_window`); the layered island window's
//! own bitmap then only carries an invisible hit-test mask (alpha 1/255
//! where the island takes the mouse, pen and touch, 0 elsewhere), so
//! transparent pixels stay click-through and tap-through as before.
//!
//! Software renderer (no Direct3D or DirectComposition, e.g. under Wine, or
//! after repeated device loss): the frame itself is the window's bitmap, and
//! each frame only copies the rectangle the island covers.
//!
//! All drawing coordinates are DIPs; the render target's DPI is set to the
//! monitor DPI so Direct2D does the scaling. Frames are only produced while
//! something animates, so idle CPU stays at zero.

use crate::icons::{self, Icon};
use crate::glass::{GlassParams, GlassPass};
use poni_core::geom::{Color, Point, Rect};
use poni_core::look::{self, Look};
use poni_core::svgpath::Seg;
use std::collections::HashMap;
use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use windows::core::{w, Interface, Result, GUID, HRESULT, PCWSTR};
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Foundation::{BOOL, COLORREF, E_NOTIMPL, HMODULE, HWND, POINT, RECT, SIZE};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::DirectComposition::{IDCompositionDevice, IDCompositionTarget, IDCompositionVisual};
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};
use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, UpdateLayeredWindowIndirect, SW_HIDE, SW_SHOWNOACTIVATE, ULW_ALPHA, UPDATELAYEREDWINDOWINFO};

// ---------------------------------------------------------------- images

/// Decoded image: premultiplied BGRA, top-down rows, `width * 4` stride.
pub struct ImageData {
    pub id: u64,
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl std::fmt::Debug for ImageData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ImageData#{}({}x{})", self.id, self.width, self.height)
    }
}

pub type Image = Arc<ImageData>;

static NEXT_IMAGE_ID: AtomicU64 = AtomicU64::new(1);

impl ImageData {
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Image {
        debug_assert_eq!(pixels.len(), (width * height * 4) as usize);
        Arc::new(ImageData { id: NEXT_IMAGE_ID.fetch_add(1, Ordering::Relaxed), width, height, pixels })
    }

    /// A saturated, reasonably bright colour from the image (for accents),
    /// falling back to the plain average for greyscale art.
    pub fn accent_color(&self) -> Color {
        let (mut best, mut best_score) = (None, 0.0f32);
        let (mut sr, mut sg, mut sb, mut n) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        let px = (self.width * self.height) as usize;
        let step = (px / 2048).max(1);
        let mut i = 0;
        while i < px {
            let o = i * 4;
            let a = self.pixels[o + 3] as f32 / 255.0;
            if a > 0.5 {
                let b = self.pixels[o] as f32 / 255.0 / a;
                let g = self.pixels[o + 1] as f32 / 255.0 / a;
                let r = self.pixels[o + 2] as f32 / 255.0 / a;
                sr += r;
                sg += g;
                sb += b;
                n += 1.0;
                let max = r.max(g).max(b);
                let min = r.min(g).min(b);
                let sat = if max > 0.0 { (max - min) / max } else { 0.0 };
                // Prefer vivid AND bright pixels (the old score picked dark seas).
                let score = sat * max * max;
                if score > best_score {
                    best_score = score;
                    best = Some(Color::rgba(r.min(1.0), g.min(1.0), b.min(1.0), 1.0));
                }
            }
            i += step;
        }
        match best {
            Some(c) if best_score > 0.2 => c.for_island_accent(),
            _ if n > 0.0 => Color::rgba(sr / n, sg / n, sb / n, 1.0).for_island_accent(),
            _ => Color::hex(0x0A84FF),
        }
    }
}

// ---------------------------------------------------------------- text

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Weight {
    Regular,
    Medium,
    Semibold,
    Bold,
}

impl Weight {
    fn dwrite(self) -> DWRITE_FONT_WEIGHT {
        match self {
            Weight::Regular => DWRITE_FONT_WEIGHT_NORMAL,
            Weight::Medium => DWRITE_FONT_WEIGHT_MEDIUM,
            Weight::Semibold => DWRITE_FONT_WEIGHT_SEMI_BOLD,
            Weight::Bold => DWRITE_FONT_WEIGHT_BOLD,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// The type scale. Use these instead of literal sizes; nothing below CAPTION.
pub mod ts {
    pub const CAPTION: f32 = 11.0;
    pub const FOOTNOTE: f32 = 12.0;
    pub const BODY: f32 = 13.0;
    pub const HEADLINE: f32 = 15.0;
    pub const VALUE: f32 = 18.0;
    pub const DISPLAY: f32 = 30.0;
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextStyle {
    pub size: f32,
    pub weight: Weight,
    pub color: Color,
    pub align: Align,
    /// Fixed-width digits (timers, clocks) so numbers don't jiggle.
    pub tabular: bool,
    /// Wrap onto several lines instead of a single ellipsized line.
    pub wrap: bool,
    /// Vertically centre inside the rect (default) or top-align.
    pub vcenter: bool,
    /// Uniform line height as a multiple of the size (0 = font default).
    pub line: f32,
}

impl TextStyle {
    pub fn new(size: f32) -> Self {
        Self { size, weight: Weight::Regular, color: poni_core::geom::palette::TEXT, align: Align::Left, tabular: false, wrap: false, vcenter: true, line: 0.0 }
    }
    pub fn caption() -> Self {
        Self::new(ts::CAPTION)
    }
    pub fn footnote() -> Self {
        Self::new(ts::FOOTNOTE)
    }
    pub fn body() -> Self {
        Self::new(ts::BODY)
    }
    pub fn headline() -> Self {
        Self::new(ts::HEADLINE).weight(Weight::Semibold)
    }
    pub fn value() -> Self {
        Self::new(ts::VALUE).weight(Weight::Semibold)
    }
    pub fn display() -> Self {
        Self::new(ts::DISPLAY).weight(Weight::Semibold)
    }
    /// Uniform line spacing (e.g. 1.2) so wrapped lines read as one unit.
    pub fn line_spacing(mut self, mult: f32) -> Self {
        self.line = mult;
        self
    }
    pub fn weight(mut self, w: Weight) -> Self {
        self.weight = w;
        self
    }
    pub fn color(mut self, c: Color) -> Self {
        self.color = c;
        self
    }
    pub fn align(mut self, a: Align) -> Self {
        self.align = a;
        self
    }
    pub fn center(self) -> Self {
        self.align(Align::Center)
    }
    pub fn right(self) -> Self {
        self.align(Align::Right)
    }
    pub fn tabular(mut self) -> Self {
        self.tabular = true;
        self
    }
    pub fn wrap(mut self) -> Self {
        self.wrap = true;
        self
    }
    pub fn top(mut self) -> Self {
        self.vcenter = false;
        self
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn d2d_color(c: Color) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: c.r, g: c.g, b: c.b, a: c.a.clamp(0.0, 1.0) }
}

fn rectf(r: Rect) -> D2D_RECT_F {
    D2D_RECT_F { left: r.x, top: r.y, right: r.x + r.w, bottom: r.y + r.h }
}

fn pt(x: f32, y: f32) -> D2D_POINT_2F {
    D2D_POINT_2F { x, y }
}

// ---------------------------------------------------------------- targets

/// Direct2D (software) into a top-down 32-bit premultiplied DIB that is
/// pushed to the layered window with `UpdateLayeredWindowIndirect`.
struct SoftTarget {
    rt: ID2D1DCRenderTarget,
    mem_dc: HDC,
    dib: HBITMAP,
    old_bmp: HGDIOBJ,
    bits: *mut u8,
    cap_w: i32,
    cap_h: i32,
    /// Position and size last given to the window: (x, y, w, h).
    shown: Option<(i32, i32, i32, i32)>,
}

impl Drop for SoftTarget {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.mem_dc, self.old_bmp);
            let _ = DeleteObject(self.dib);
            let _ = DeleteDC(self.mem_dc);
        }
    }
}

impl SoftTarget {
    fn new(d2d: &ID2D1Factory) -> Result<Self> {
        unsafe {
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                pixelFormat: PREMUL_BGRA,
                dpiX: 0.0,
                dpiY: 0.0,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let rt = d2d.CreateDCRenderTarget(&props)?;
            rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            rt.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            let mem_dc = CreateCompatibleDC(None);
            let mut t = SoftTarget { rt, mem_dc, dib: HBITMAP::default(), old_bmp: HGDIOBJ::default(), bits: std::ptr::null_mut(), cap_w: 0, cap_h: 0, shown: None };
            t.ensure_capacity(512, 128)?;
            Ok(t)
        }
    }

    /// Make sure the DIB can hold a `w`×`h` pixel frame.
    fn ensure_capacity(&mut self, w: i32, h: i32) -> Result<()> {
        if w <= self.cap_w && h <= self.cap_h && !self.bits.is_null() {
            return Ok(());
        }
        let nw = ((w.max(self.cap_w) + 63) / 64) * 64;
        let nh = ((h.max(self.cap_h) + 63) / 64) * 64;
        unsafe {
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: nw,
                    biHeight: -nh, // top-down
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut c_void = std::ptr::null_mut();
            let dib = CreateDIBSection(self.mem_dc, &bmi, DIB_RGB_COLORS, &mut bits, None, 0)?;
            let prev = SelectObject(self.mem_dc, dib);
            if self.dib.is_invalid() {
                self.old_bmp = prev;
            } else {
                let _ = DeleteObject(self.dib);
            }
            self.dib = dib;
            self.bits = bits as *mut u8;
            self.cap_w = nw;
            self.cap_h = nh;
            // A fresh DIB is zeroed (transparent); the window must get all of it.
            self.shown = None;
        }
        Ok(())
    }

    unsafe fn begin(&mut self, w: i32, h: i32, dpi: f32) -> Result<()> {
        let rc = RECT { left: 0, top: 0, right: w, bottom: h };
        self.rt.BindDC(self.mem_dc, &rc)?;
        self.rt.SetDpi(dpi, dpi);
        self.rt.BeginDraw();
        Ok(())
    }

    /// Push the DIB to the window at (`x`, `y`), size `w`×`h`. With `dirty`,
    /// only that rectangle is copied (unless the window moved or resized).
    unsafe fn push(&mut self, hwnd: HWND, x: i32, y: i32, w: i32, h: i32, dirty: Option<RECT>) -> Result<()> {
        let same = self.shown == Some((x, y, w, h));
        let dirty = dirty.filter(|_| same).map(|d| RECT { left: d.left.clamp(0, w), top: d.top.clamp(0, h), right: d.right.clamp(0, w), bottom: d.bottom.clamp(0, h) });
        if dirty.is_some_and(|d| d.right <= d.left || d.bottom <= d.top) {
            return Ok(());
        }
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
        let (dst, size, src) = (POINT { x, y }, SIZE { cx: w, cy: h }, POINT { x: 0, y: 0 });
        let info = UPDATELAYEREDWINDOWINFO {
            cbSize: std::mem::size_of::<UPDATELAYEREDWINDOWINFO>() as u32,
            hdcDst: HDC::default(),
            pptDst: &dst,
            psize: &size,
            hdcSrc: self.mem_dc,
            pptSrc: &src,
            crKey: COLORREF(0),
            pblend: &blend,
            dwFlags: ULW_ALPHA,
            prcDirty: dirty.as_ref().map_or(std::ptr::null(), |d| d as *const RECT),
        };
        if UpdateLayeredWindowIndirect(hwnd, &info).as_bool() {
            self.shown = Some((x, y, w, h));
            Ok(())
        } else {
            self.shown = None;
            Err(windows::core::Error::from_win32())
        }
    }

    /// Give `hwnd`, another layered window, a fully transparent `w`×`h`
    /// bitmap at (`x`, `y`): it places and sizes that window and shows
    /// nothing of its own. Clears the DIB on the way; what the island's
    /// window shows stays, and its next push redraws what it copies.
    unsafe fn push_clear(&mut self, hwnd: HWND, x: i32, y: i32, w: i32, h: i32) -> Result<()> {
        self.ensure_capacity(w, h)?;
        for row in 0..h {
            std::ptr::write_bytes(self.bits.add((row * self.cap_w * 4) as usize), 0, (w * 4) as usize);
        }
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
        let (dst, size, src) = (POINT { x, y }, SIZE { cx: w, cy: h }, POINT { x: 0, y: 0 });
        let info = UPDATELAYEREDWINDOWINFO {
            cbSize: std::mem::size_of::<UPDATELAYEREDWINDOWINFO>() as u32,
            hdcDst: HDC::default(),
            pptDst: &dst,
            psize: &size,
            hdcSrc: self.mem_dc,
            pptSrc: &src,
            crKey: COLORREF(0),
            pblend: &blend,
            dwFlags: ULW_ALPHA,
            prcDirty: std::ptr::null(),
        };
        if UpdateLayeredWindowIndirect(hwnd, &info).as_bool() {
            Ok(())
        } else {
            Err(windows::core::Error::from_win32())
        }
    }

    /// Copy of the pixels in `r` (premultiplied BGRA, top-down).
    fn pixels(&self, r: RECT) -> Vec<u8> {
        let (x0, y0) = (r.left.clamp(0, self.cap_w), r.top.clamp(0, self.cap_h));
        let (x1, y1) = (r.right.clamp(x0, self.cap_w), r.bottom.clamp(y0, self.cap_h));
        let mut out = Vec::with_capacity(((x1 - x0) * (y1 - y0) * 4) as usize);
        unsafe {
            for y in y0..y1 {
                let row = self.bits.add(((y * self.cap_w + x0) * 4) as usize);
                out.extend_from_slice(std::slice::from_raw_parts(row, ((x1 - x0) * 4) as usize));
            }
        }
        out
    }
}

/// Direct2D on a Direct3D 11 device, drawing into a flip-model swap chain
/// that DirectComposition shows on the island window.
struct GpuTarget {
    // Declaration order is drop order: the composition target goes first, so
    // the window falls back to its own bitmap before the device goes away.
    _target: IDCompositionTarget,
    _visual: IDCompositionVisual,
    _dcomp: IDCompositionDevice,
    target_bmp: Option<ID2D1Bitmap1>,
    dc: ID2D1DeviceContext,
    swap: IDXGISwapChain1,
    staging: Option<(ID3D11Texture2D, i32, i32)>,
    ctx: ID3D11DeviceContext,
    d3d: ID3D11Device,
    w: i32,
    h: i32,
    warp: bool,
    /// The adapter is Microsoft's software rasterizer (WARP, or the Basic
    /// Render Driver a machine without a GPU driver offers as "hardware").
    soft_adapter: bool,
    /// The adapter's name, for the log.
    adapter: String,
}

/// Microsoft's PCI vendor id and the Basic Render Driver's device id: a
/// "hardware" adapter that is really WARP (VMs and CI runners without a GPU).
const MICROSOFT_VENDOR: u32 = 0x1414;
const BASIC_RENDER_DEVICE: u32 = 0x8C;

/// A Direct3D 11 device that Direct2D can draw with: the GPU, else WARP.
unsafe fn create_d3d_device() -> Result<(ID3D11Device, ID3D11DeviceContext, bool)> {
    let levels = [
        D3D_FEATURE_LEVEL_11_1,
        D3D_FEATURE_LEVEL_11_0,
        D3D_FEATURE_LEVEL_10_1,
        D3D_FEATURE_LEVEL_10_0,
        D3D_FEATURE_LEVEL_9_3,
        D3D_FEATURE_LEVEL_9_2,
        D3D_FEATURE_LEVEL_9_1,
    ];
    let mut last = None;
    for (driver, warp) in [(D3D_DRIVER_TYPE_HARDWARE, false), (D3D_DRIVER_TYPE_WARP, true)] {
        // Runtimes that don't know 11.1 reject the whole list: retry without it.
        for lv in [&levels[..], &levels[1..]] {
            let (mut dev, mut ctx) = (None, None);
            match D3D11CreateDevice(None, driver, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, Some(lv), D3D11_SDK_VERSION, Some(&mut dev), None, Some(&mut ctx)) {
                Ok(()) => {
                    if let (Some(d), Some(c)) = (dev, ctx) {
                        return Ok((d, c, warp));
                    }
                }
                Err(e) => last = Some(e),
            }
        }
    }
    Err(last.unwrap_or_else(|| E_NOTIMPL.into()))
}

/// `DCompositionCreateDevice`, looked up at run time from System32 (it is
/// missing or a stub on some systems, and must never load from the app folder).
unsafe fn dcomp_create_device(dxgi: &IDXGIDevice) -> Result<IDCompositionDevice> {
    type Create = unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT;
    let lib = LoadLibraryExW(w!("dcomp.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32)?;
    let Some(f) = GetProcAddress(lib, windows::core::s!("DCompositionCreateDevice")) else {
        return Err(E_NOTIMPL.into());
    };
    let create: Create = std::mem::transmute(f);
    let mut out = std::ptr::null_mut();
    create(dxgi.as_raw(), &IDCompositionDevice::IID, &mut out).ok()?;
    if out.is_null() {
        return Err(E_NOTIMPL.into());
    }
    Ok(IDCompositionDevice::from_raw(out))
}

impl GpuTarget {
    unsafe fn new(d2d: &ID2D1Factory1, hwnd: HWND, w: i32, h: i32) -> Result<Self> {
        let (d3d, ctx, warp) = create_d3d_device()?;
        let dxgi: IDXGIDevice = d3d.cast()?;
        let dc = d2d.CreateDevice(&dxgi)?.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
        let dxgi_adapter = dxgi.GetAdapter()?;
        let (soft_adapter, adapter) = match dxgi_adapter.GetDesc() {
            Ok(d) => {
                let name = String::from_utf16_lossy(&d.Description).trim_end_matches('\0').to_string();
                (warp || (d.VendorId == MICROSOFT_VENDOR && d.DeviceId == BASIC_RENDER_DEVICE), name)
            }
            Err(_) => (warp, String::new()),
        };
        let factory: IDXGIFactory2 = dxgi_adapter.GetParent()?;
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: w as u32,
            Height: h as u32,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            Stereo: BOOL(0),
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            // Three: drawing the next frame never waits for DWM to let go of one.
            BufferCount: 3,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
            Flags: 0,
        };
        let swap = factory.CreateSwapChainForComposition(&d3d, &desc, None)?;
        let dcomp = dcomp_create_device(&dxgi)?;
        let target = dcomp.CreateTargetForHwnd(hwnd, true)?;
        let visual = dcomp.CreateVisual()?;
        visual.SetContent(&swap)?;
        target.SetRoot(&visual)?;
        dcomp.Commit()?;
        Ok(Self { _target: target, _visual: visual, _dcomp: dcomp, target_bmp: None, dc, swap, staging: None, ctx, d3d, w, h, warp, soft_adapter, adapter })
    }

    unsafe fn resize(&mut self, w: i32, h: i32) -> Result<()> {
        self.dc.SetTarget(None::<&ID2D1Image>);
        self.target_bmp = None;
        self.staging = None;
        self.swap.ResizeBuffers(3, w as u32, h as u32, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SWAP_CHAIN_FLAG(0))?;
        self.w = w;
        self.h = h;
        Ok(())
    }

    /// Point the device context at the swap chain's back buffer.
    unsafe fn begin(&mut self, dpi: f32) -> Result<()> {
        let surface: IDXGISurface = self.swap.GetBuffer(0)?;
        let props = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: PREMUL_BGRA,
            dpiX: dpi,
            dpiY: dpi,
            bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
            colorContext: ManuallyDrop::new(None),
        };
        let bmp = self.dc.CreateBitmapFromDxgiSurface(&surface, Some(&props))?;
        self.dc.SetTarget(&bmp);
        self.dc.SetDpi(dpi, dpi);
        self.dc.BeginDraw();
        self.target_bmp = Some(bmp);
        Ok(())
    }

    unsafe fn end(&mut self) {
        self.dc.SetTarget(None::<&ID2D1Image>);
        self.target_bmp = None;
    }

    /// Read `r` back from the frame just drawn (self-test only).
    unsafe fn read(&mut self, r: RECT) -> Result<Vec<u8>> {
        let back: ID3D11Texture2D = self.swap.GetBuffer(0)?;
        if !self.staging.as_ref().is_some_and(|s| s.1 == self.w && s.2 == self.h) {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: self.w as u32,
                Height: self.h as u32,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: 0,
            };
            let mut tex = None;
            self.d3d.CreateTexture2D(&desc, None, Some(&mut tex))?;
            self.staging = tex.map(|t| (t, self.w, self.h));
        }
        let Some((staging, _, _)) = self.staging.as_ref() else { return Err(E_NOTIMPL.into()) };
        self.ctx.CopyResource(staging, &back);
        let mut m = D3D11_MAPPED_SUBRESOURCE::default();
        self.ctx.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut m))?;
        let (x0, y0) = (r.left.clamp(0, self.w), r.top.clamp(0, self.h));
        let (x1, y1) = (r.right.clamp(x0, self.w), r.bottom.clamp(y0, self.h));
        let mut out = Vec::with_capacity(((x1 - x0) * (y1 - y0) * 4) as usize);
        for y in y0..y1 {
            let row = (m.pData as *const u8).add(y as usize * m.RowPitch as usize + x0 as usize * 4);
            out.extend_from_slice(std::slice::from_raw_parts(row, ((x1 - x0) * 4) as usize));
        }
        self.ctx.Unmap(staging, 0);
        Ok(out)
    }
}

// ---------------------------------------------------------------- Gfx

/// Which renderer draws the frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Renderer {
    /// Direct2D on a Direct3D 11 device (`warp` = Microsoft's software
    /// rasterizer), presented through DirectComposition.
    Gpu { warp: bool },
    /// Direct2D in software, pushed with `UpdateLayeredWindowIndirect`.
    Software,
}

/// Where the island takes the mouse while the GPU renderer draws (the
/// layered window's own bitmap: alpha 1/255 inside, 0 elsewhere). DIPs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HitMask {
    /// A plain rectangle (the open island and its forgiving margin).
    pub rect: Option<Rect>,
    /// The island silhouette: body, bottom corner radius, shoulder.
    pub notch: Option<(Rect, f32, f32)>,
    /// The floating island's outline (the Pill theme): body, corner radius.
    pub pill: Option<(Rect, f32)>,
}

impl HitMask {
    fn bounds(&self) -> Option<Rect> {
        let a = self.rect;
        let b = self.notch.map(|(body, _, s)| Rect::new(body.x - s, body.y, body.w + 2.0 * s, body.h));
        let c = self.pill.map(|(body, _)| body);
        [a, b, c].into_iter().flatten().reduce(union_rect)
    }

    /// Whether `p` (DIPs) is inside the mask: the same shapes the mask is
    /// filled with, so the app's own tests agree with where the window takes
    /// the mouse.
    pub fn contains(&self, p: Point) -> bool {
        self.rect.is_some_and(|r| r.contains(p))
            || self.pill.is_some_and(|(body, radius)| in_rounded_rect(body, radius, p))
            || self.notch.is_some_and(|(body, radius, shoulder)| in_notch(body, radius, shoulder, p))
    }
}

/// `p` inside `body` with every corner rounded by `radius` (clamped as
/// `pill::shape::segments` clamps it, so a short body is a capsule).
fn in_rounded_rect(body: Rect, radius: f32, p: Point) -> bool {
    if !body.contains(p) {
        return false;
    }
    let r = radius.min(body.h / 2.0).min(body.w / 2.0).max(0.0);
    // max/min, not clamp: for a capsule (r = h/2) at fractional positions,
    // as while it moves, `y + r` can round one step past `bottom - r`, and
    // clamp panics when its bounds cross.
    let cx = p.x.max(body.x + r).min(body.right() - r);
    let cy = p.y.max(body.y + r).min(body.bottom() - r);
    (p.x - cx).hypot(p.y - cy) <= r
}

/// `p` inside [`notch_segments`]' outline: from the top edge down, the body
/// with rounded bottom corners, and the concave shoulders flaring into the
/// edge beside it.
fn in_notch(body: Rect, radius: f32, shoulder: f32, p: Point) -> bool {
    if p.y < 0.0 || p.y >= body.bottom() {
        return false;
    }
    if p.x >= body.x && p.x < body.right() {
        let r = radius.min(body.h).min(body.w / 2.0).max(0.0);
        // max/min, not clamp (see in_rounded_rect).
        let cx = p.x.max(body.x + r).min(body.right() - r);
        let cy = p.y.min(body.bottom() - r);
        return (p.x - cx).hypot(p.y - cy) <= r;
    }
    // A shoulder: the corner between the edge and the body's side, outside
    // the arc from `shoulder` out along the edge to `shoulder` down the side
    // (an ellipse when the body is shorter than that).
    let (sw, sh) = (shoulder.max(0.0), shoulder.max(0.0).min(body.h));
    let out = if p.x < body.x { body.x - p.x } else { p.x - body.right() };
    if sw <= 0.0 || sh <= 0.0 || out > sw || p.y >= sh {
        return false;
    }
    let (u, v) = ((sw - out) / sw, (sh - p.y) / sh);
    u * u + v * v >= 1.0
}

/// Smallest rectangle holding both.
pub fn union_rect(a: Rect, b: Rect) -> Rect {
    let (x0, y0) = (a.x.min(b.x), a.y.min(b.y));
    Rect::new(x0, y0, a.right().max(b.right()) - x0, a.bottom().max(b.bottom()) - y0)
}

/// `r` (DIPs) as whole pixels, grown outwards by `pad` px and clamped to `w`×`h`.
pub fn px_rect(r: Rect, scale: f32, pad: i32, w: i32, h: i32) -> RECT {
    RECT {
        left: (((r.x * scale).floor() as i32) - pad).clamp(0, w),
        top: (((r.y * scale).floor() as i32) - pad).clamp(0, h),
        right: (((r.right() * scale).ceil() as i32) + pad).clamp(0, w),
        bottom: (((r.bottom() * scale).ceil() as i32) + pad).clamp(0, h),
    }
}

/// Smallest pixel rectangle holding both.
pub fn px_union(a: RECT, b: RECT) -> RECT {
    if a.right <= a.left || a.bottom <= a.top {
        return b;
    }
    if b.right <= b.left || b.bottom <= b.top {
        return a;
    }
    RECT { left: a.left.min(b.left), top: a.top.min(b.top), right: a.right.max(b.right), bottom: a.bottom.max(b.bottom) }
}

/// Scale by `s` about `p`.
pub fn scale_about(p: Point, s: f32) -> Matrix3x2 {
    Matrix3x2 { M11: s, M12: 0.0, M21: 0.0, M22: s, M31: p.x * (1.0 - s), M32: p.y * (1.0 - s) }
}

/// Text layouts are cached by everything that shapes them (not colour).
#[derive(Clone, PartialEq, Eq, Hash)]
struct LayoutKey {
    text: String,
    size: u32,
    weight: Weight,
    wrap: bool,
    align: Align,
    vcenter: bool,
    tabular: bool,
    line: u32,
    w: u32,
    h: u32,
}

const PREMUL_BGRA: D2D1_PIXEL_FORMAT = D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED };
const CLEAR: D2D1_COLOR_F = D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 };

pub struct Gfx {
    pub d2d: ID2D1Factory,
    pub dwrite: IDWriteFactory,
    /// Where drawing goes right now: the frame, or an offscreen layer.
    rt: ID2D1RenderTarget,
    /// The frame's render target; brushes and bitmaps are created on it.
    main: ID2D1RenderTarget,
    gpu: Option<GpuTarget>,
    soft: SoftTarget,
    /// Brush for the hit mask, on the software target.
    mask_brush: ID2D1SolidColorBrush,
    hwnd: HWND,
    /// The window the GPU renderer's frames show in (made with the first GPU
    /// target), and where it was last put: (x, y, w, h) in screen pixels.
    visual: Option<HWND>,
    visual_at: Option<(i32, i32, i32, i32)>,
    /// The app shows the island; the visual window goes with it.
    shown: bool,
    /// Surface (= window) size in pixels.
    surf_w: i32,
    surf_h: i32,
    pub dpi: f32,
    brush: ID2D1SolidColorBrush,
    round_stroke: ID2D1StrokeStyle,
    /// The system UI font family (Segoe UI Variable / Segoe UI).
    system_family: Vec<u16>,
    /// The family text is written in now: the look's font, else the system's.
    family: Vec<u16>,
    /// The fonts shipped in the exe, once loaded (`None` until a look wants
    /// them, or when this Windows can't load fonts from memory).
    embedded: Option<IDWriteFontCollection>,
    embedded_tried: bool,
    /// How content is painted (the theme).
    look: &'static Look,
    formats: HashMap<(u32, Weight, bool), IDWriteTextFormat>,
    /// Text layouts with the frame they were last used in.
    layouts: HashMap<LayoutKey, (IDWriteTextLayout, u64)>,
    bitmaps: HashMap<u64, ID2D1Bitmap>,
    icon_geoms: HashMap<Icon, Vec<(ID2D1PathGeometry, bool)>>,
    /// Bitmaps used during the current frame (others are evicted at `end`).
    used_bitmaps: Vec<u64>,
    /// Whether each pushed shape mask really pushed a layer (to balance pops).
    mask_stack: Vec<bool>,
    /// Offscreen layers for [`Canvas::layered`], reused frame to frame.
    layers: Vec<ID2D1BitmapRenderTarget>,
    layers_used: usize,
    /// Gradient brushes for [`Canvas::rrect_shadow`], by stops and colour
    /// (made on `main`, so they start over with it).
    shadows: HashMap<u64, (ID2D1RadialGradientBrush, ID2D1LinearGradientBrush)>,
    frame_no: u64,
    /// Pixels the software present copies this frame.
    dirty: RECT,
    /// The next software present must copy the whole surface.
    soft_full: bool,
    /// Rectangle to read back after this frame (self-test), and its pixels.
    capture: Option<RECT>,
    captured: Vec<u8>,
    /// GPU renderer allowed (not forced off, not given up after failures).
    gpu_allowed: bool,
    /// GPU failures in a row (a long healthy run forgives earlier ones).
    gpu_failures: u32,
    gpu_frames: u32,
    /// The renderer switched (GPU lost or started) since the app last asked.
    renderer_changed: bool,
    /// The Glass theme's shader and textures (GPU only, made on first use).
    glass: Option<GlassPass>,
    /// The shader can't run here (compile failed, old GPU): smoked glass.
    glass_broken: bool,
    glass_failures: u32,
    glass_ok: u32,
    /// `prepare_glass` drew this frame's glass.
    glass_drawn: bool,
    /// ...and the last time it was called (the self-test reads this).
    glass_drew_last: bool,
}

fn pick_font_family(dwrite: &IDWriteFactory) -> Vec<u16> {
    // Windows 11 ships "Segoe UI Variable"; Windows 10 has "Segoe UI".
    unsafe {
        let mut coll: Option<IDWriteFontCollection> = None;
        if dwrite.GetSystemFontCollection(&mut coll, false).is_ok() {
            if let Some(coll) = coll {
                for name in ["Segoe UI Variable Text", "Segoe UI Variable", "Segoe UI"] {
                    let w: Vec<u16> = wide(name).into_iter().chain(Some(0)).collect();
                    let mut index = 0u32;
                    let mut exists = BOOL(0);
                    if coll.FindFamilyName(PCWSTR(w.as_ptr()), &mut index, &mut exists).is_ok() && exists.as_bool() {
                        return w;
                    }
                }
            }
        }
    }
    wide("Segoe UI").into_iter().chain(Some(0)).collect()
}

/// Fonts shipped inside the exe: Outfit (SIL Open Font License 1.1, see
/// `res/fonts/OFL.txt`), the Oyenza Studio website's typeface, in the four weights
/// the island uses.
const EMBEDDED_FONTS: [&[u8]; 4] = [
    include_bytes!("../res/fonts/Outfit-Regular.ttf"),
    include_bytes!("../res/fonts/Outfit-Medium.ttf"),
    include_bytes!("../res/fonts/Outfit-SemiBold.ttf"),
    include_bytes!("../res/fonts/Outfit-Bold.ttf"),
];

/// A font collection of the embedded fonts (needs Windows 10 1703+). Letters
/// they lack (Japanese, Korean, emoji, ...) still come from the system fonts
/// through DirectWrite's fallback.
fn load_embedded_fonts(dwrite: &IDWriteFactory) -> Result<IDWriteFontCollection> {
    unsafe {
        let f5: IDWriteFactory5 = dwrite.cast()?;
        let loader = f5.CreateInMemoryFontFileLoader()?;
        f5.RegisterFontFileLoader(&loader)?;
        let builder = f5.CreateFontSetBuilder()?;
        for data in EMBEDDED_FONTS {
            // No owner object: DirectWrite keeps its own copy of the bytes.
            let file = loader.CreateInMemoryFontFileReference(&f5, data.as_ptr() as *const c_void, data.len() as u32, None::<&windows::core::IUnknown>)?;
            builder.AddFontFile(&file)?;
        }
        let set = builder.CreateFontSet()?;
        f5.CreateFontCollectionFromFontSet(&set)?.cast()
    }
}

/// Whether `coll` holds a family called `name`.
fn has_family(coll: &IDWriteFontCollection, name: &[u16]) -> bool {
    let mut index = 0u32;
    let mut exists = BOOL(0);
    unsafe { coll.FindFamilyName(PCWSTR(name.as_ptr()), &mut index, &mut exists).is_ok() && exists.as_bool() }
}

/// `PONI_RENDERER=software` forces the software renderer (troubleshooting).
fn gpu_wanted() -> bool {
    !std::env::var("PONI_RENDERER").is_ok_and(|v| v.trim().eq_ignore_ascii_case("software"))
}

fn setup_target(rt: &ID2D1RenderTarget) {
    unsafe {
        rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
        rt.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
    }
}

impl Gfx {
    pub fn new(dpi: f32, hwnd: HWND) -> Result<Self> {
        unsafe {
            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let soft = SoftTarget::new(&d2d)?;
            let mask_brush = soft.rt.CreateSolidColorBrush(&d2d_color(Color::black(1.0 / 255.0)), None)?;
            let main: ID2D1RenderTarget = soft.rt.cast()?;
            setup_target(&main);
            let brush = main.CreateSolidColorBrush(&d2d_color(Color::white(1.0)), None)?;
            let round_stroke = d2d.CreateStrokeStyle(
                &D2D1_STROKE_STYLE_PROPERTIES {
                    startCap: D2D1_CAP_STYLE_ROUND,
                    endCap: D2D1_CAP_STYLE_ROUND,
                    dashCap: D2D1_CAP_STYLE_ROUND,
                    lineJoin: D2D1_LINE_JOIN_ROUND,
                    miterLimit: 10.0,
                    dashStyle: D2D1_DASH_STYLE_SOLID,
                    dashOffset: 0.0,
                },
                None,
            )?;
            let family = pick_font_family(&dwrite);
            let gpu_allowed = gpu_wanted();
            if !gpu_allowed {
                crate::pinfo!("renderer: software (PONI_RENDERER=software)");
            }
            Ok(Gfx {
                d2d,
                dwrite,
                rt: main.clone(),
                main,
                gpu: None,
                soft,
                mask_brush,
                hwnd,
                visual: None,
                visual_at: None,
                shown: false,
                surf_w: 0,
                surf_h: 0,
                dpi,
                brush,
                round_stroke,
                system_family: family.clone(),
                family,
                embedded: None,
                embedded_tried: false,
                look: &look::CLASSIC,
                formats: HashMap::new(),
                layouts: HashMap::new(),
                bitmaps: HashMap::new(),
                icon_geoms: HashMap::new(),
                used_bitmaps: Vec::new(),
                mask_stack: Vec::new(),
                layers: Vec::new(),
                layers_used: 0,
                shadows: HashMap::new(),
                frame_no: 0,
                dirty: RECT::default(),
                soft_full: true,
                capture: None,
                captured: Vec::new(),
                gpu_allowed,
                gpu_failures: 0,
                gpu_frames: 0,
                renderer_changed: false,
                glass: None,
                glass_broken: false,
                glass_failures: 0,
                glass_ok: 0,
                glass_drawn: false,
                glass_drew_last: false,
            })
        }
    }

    pub fn scale(&self) -> f32 {
        self.dpi / 96.0
    }

    pub fn set_dpi(&mut self, dpi: f32) {
        if dpi != self.dpi {
            self.dpi = dpi;
            self.layers.clear();
        }
    }

    /// Paint with `look` from the next frame on. A new font throws away the
    /// cached text formats and layouts.
    pub fn set_look(&mut self, look: &'static Look) {
        if std::ptr::eq(self.look, look) {
            return;
        }
        self.look = look;
        let family = match look.font {
            Some(name) => {
                if !self.embedded_tried {
                    self.embedded_tried = true;
                    match load_embedded_fonts(&self.dwrite) {
                        Ok(c) => self.embedded = Some(c),
                        Err(e) => crate::pwarn!("fonts: embedded fonts unavailable ({e}); using the system font"),
                    }
                }
                let w: Vec<u16> = wide(name).into_iter().chain(Some(0)).collect();
                match &self.embedded {
                    Some(c) if has_family(c, &w) => w,
                    _ => self.system_family.clone(),
                }
            }
            None => self.system_family.clone(),
        };
        if family != self.family {
            self.family = family;
            self.formats.clear();
            self.layouts.clear();
        }
    }

    pub fn renderer(&self) -> Renderer {
        match &self.gpu {
            Some(g) => Renderer::Gpu { warp: g.warp },
            None => Renderer::Software,
        }
    }

    /// Drawing on the GPU path, but with Microsoft's software rasterizer
    /// (WARP or the Basic Render Driver): fine for correctness, not for timing.
    pub fn software_gpu(&self) -> bool {
        self.gpu.as_ref().is_some_and(|g| g.soft_adapter)
    }

    /// The GPU renderer draws: the window's bitmap must carry the hit mask.
    pub fn needs_hit_mask(&self) -> bool {
        self.gpu.is_some()
    }

    /// Whether the renderer switched since the last call. The window's own
    /// bitmap then holds the other renderer's content (a hit mask, or a
    /// software frame): the app must redraw and push everything again.
    pub fn take_renderer_changed(&mut self) -> bool {
        std::mem::take(&mut self.renderer_changed)
    }

    /// Size the surface (and so the window) to `w`×`h` pixels.
    pub fn set_surface(&mut self, w: i32, h: i32) -> Result<()> {
        let (w, h) = (w.max(1), h.max(1));
        if (w, h) == (self.surf_w, self.surf_h) {
            return Ok(());
        }
        self.surf_w = w;
        self.surf_h = h;
        self.layers.clear();
        self.soft_full = true;
        self.soft.ensure_capacity(w, h)?;
        if let Some(g) = self.gpu.as_mut() {
            if let Err(e) = unsafe { g.resize(w, h) } {
                self.gpu_lost("resize", e);
            }
        }
        Ok(())
    }

    /// Switch drawing to `main` (a new renderer): device resources start over.
    fn use_main(&mut self, main: ID2D1RenderTarget) -> Result<()> {
        setup_target(&main);
        self.brush = unsafe { main.CreateSolidColorBrush(&d2d_color(Color::white(1.0)), None)? };
        self.rt = main.clone();
        self.main = main;
        self.bitmaps.clear();
        self.layers.clear();
        self.shadows.clear();
        Ok(())
    }

    /// Start (or restart) the GPU renderer when it's allowed and missing.
    fn ensure_gpu(&mut self) {
        if self.gpu.is_some() || !self.gpu_allowed || self.surf_w <= 0 {
            return;
        }
        let visual = match self.visual {
            Some(v) => v,
            None => match crate::window::create_visual_window(self.hwnd) {
                Ok(v) => *self.visual.insert(v),
                Err(e) => return self.gpu_failed(e),
            },
        };
        let made = self.d2d.cast::<ID2D1Factory1>().and_then(|f| unsafe { GpuTarget::new(&f, visual, self.surf_w, self.surf_h) });
        match made {
            Ok(g) => {
                let main = g.dc.cast::<ID2D1RenderTarget>();
                match main.and_then(|m| self.use_main(m)) {
                    Ok(()) => {
                        crate::pinfo!("renderer: GPU ({}), DirectComposition, {}x{} px", if g.warp { "WARP" } else { "hardware" }, self.surf_w, self.surf_h);
                        crate::pinfo!("renderer: adapter {}{}", g.adapter, if g.soft_adapter { " (software rasterizer)" } else { "" });
                        self.gpu = Some(g);
                        self.visual_at = None;
                        self.renderer_changed = true;
                    }
                    Err(e) => self.gpu_failed(e),
                }
            }
            Err(e) => self.gpu_failed(e),
        }
    }

    fn gpu_failed(&mut self, e: windows::core::Error) {
        self.gpu_failures += 1;
        // DirectComposition missing (Wine, very old systems) won't come back.
        if e.code() == E_NOTIMPL || self.gpu_failures >= 3 {
            self.gpu_allowed = false;
            crate::pinfo!("renderer: software (GPU unavailable: {e})");
        } else {
            crate::pwarn!("GPU renderer failed to start ({e}); retrying");
        }
    }

    /// The device was lost (driver update, GPU reset) or a call failed:
    /// draw in software until the GPU renderer can be rebuilt.
    fn gpu_lost(&mut self, what: &str, e: windows::core::Error) {
        crate::pwarn!("GPU renderer lost at {what} ({e})");
        self.glass = None;
        self.gpu = None;
        // The software renderer draws in the island's own window.
        if let Some(v) = self.visual {
            unsafe {
                let _ = ShowWindow(v, SW_HIDE);
            }
        }
        self.visual_at = None;
        self.gpu_frames = 0;
        self.renderer_changed = true;
        self.gpu_failures += 1;
        if self.gpu_failures >= 3 {
            self.gpu_allowed = false;
            crate::pwarn!("renderer: staying on software after {} GPU failures", self.gpu_failures);
        }
        self.soft_full = true;
        if let Ok(m) = self.soft.rt.cast::<ID2D1RenderTarget>() {
            let _ = self.use_main(m);
        }
    }

    /// The glass shader, made on first use. False on the software renderer
    /// or where it can't run (the island then draws smoked glass).
    fn ensure_glass(&mut self) -> bool {
        if self.glass.is_some() {
            return true;
        }
        if self.glass_broken {
            return false;
        }
        let Some(g) = self.gpu.as_ref() else { return false };
        match unsafe { GlassPass::new(&g.d3d, &g.dc) } {
            Ok(p) => {
                crate::pinfo!("glass: shader ready");
                self.glass = Some(p);
                true
            }
            Err(e) => {
                crate::pwarn!("glass: no glass shader here ({e}); smoked glass");
                self.glass_broken = true;
                false
            }
        }
    }

    /// Glass: hand over a screen copy (BGRA, the window's rectangle). False
    /// when it can't be used here (software renderer, no shader).
    pub fn glass_copy(&mut self, px: &[u8], w: i32, h: i32) -> bool {
        if !self.ensure_glass() {
            return false;
        }
        let scale = self.scale();
        let (Some(g), Some(p)) = (self.gpu.as_ref(), self.glass.as_mut()) else { return false };
        match unsafe { p.set_copy(&g.d3d, &g.ctx, &g.dc, px, w, h, scale) } {
            Ok(()) => true,
            Err(e) => {
                crate::pwarn!("glass: uploading the screen copy failed ({e})");
                false
            }
        }
    }

    /// The last frame's glass came from the shader (not smoked glass).
    pub fn glass_drew(&self) -> bool {
        self.glass_drew_last
    }

    /// The look content is painted in now.
    pub fn look(&self) -> &'static Look {
        self.look
    }

    pub fn glass_clear_copy(&mut self) {
        if let Some(p) = self.glass.as_mut() {
            p.clear_copy();
        }
    }

    /// A screen copy is on the GPU (lost with the device; the app uploads its
    /// last one again when this turns false).
    pub fn glass_has_copy(&self) -> bool {
        self.glass.as_ref().is_some_and(|p| p.has_copy())
    }

    /// Draw this frame's glass (before [`Gfx::begin`]) inside `area`
    /// (surface pixels). False: the canvas draws smoked glass itself.
    pub fn prepare_glass(&mut self, params: &GlassParams, area: RECT) -> bool {
        self.glass_drawn = false;
        self.glass_drew_last = false;
        // (No GPU restart from here: `begin` does that, once per frame.)
        if !self.ensure_glass() {
            return false;
        }
        let (w, h, dpi) = (self.surf_w, self.surf_h, self.dpi);
        let (Some(g), Some(p)) = (self.gpu.as_ref(), self.glass.as_mut()) else { return false };
        match unsafe { p.render(&g.d3d, &g.ctx, &g.dc, w, h, dpi, params, area) } {
            Ok(()) => {
                self.glass_drawn = true;
                self.glass_drew_last = true;
                // A long healthy run forgives earlier failures (driver resets).
                self.glass_ok = self.glass_ok.saturating_add(1);
                if self.glass_ok > 600 {
                    self.glass_failures = 0;
                }
                true
            }
            Err(e) => {
                self.glass_ok = 0;
                self.glass_failures += 1;
                crate::pwarn!("glass: drawing failed ({e})");
                self.glass = None;
                if self.glass_failures >= 3 {
                    self.glass_broken = true;
                    crate::pwarn!("glass: smoked glass from now on");
                }
                false
            }
        }
    }

    /// Read back `r` (surface pixels) after the next frame (self-test).
    pub fn request_capture(&mut self, r: RECT) {
        self.capture = Some(r);
        self.captured.clear();
    }

    /// Premultiplied BGRA pixels of the requested rectangle.
    pub fn take_capture(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.captured)
    }

    /// Start a frame; `dirty` is the part of the surface (pixels) that changes.
    /// Returns a canvas in DIPs.
    pub fn begin(&mut self, dirty: RECT) -> Result<Canvas<'_>> {
        self.ensure_gpu();
        self.used_bitmaps.clear();
        self.layers_used = 0;
        self.frame_no += 1;
        let full = RECT { left: 0, top: 0, right: self.surf_w, bottom: self.surf_h };
        self.dirty = if self.soft_full { full } else { px_union(RECT::default(), dirty) };
        if let Some(g) = self.gpu.as_mut() {
            if let Err(e) = unsafe { g.begin(self.dpi) } {
                self.gpu_lost("begin", e);
                self.dirty = full;
            }
        }
        unsafe {
            if self.gpu.is_none() {
                self.soft.begin(self.surf_w, self.surf_h, self.dpi)?;
            }
            self.main.SetTransform(&Matrix3x2::identity());
            if self.gpu.is_some() {
                self.main.Clear(Some(&CLEAR));
            } else {
                // Only the dirty part: the rest of the DIB is already clear.
                let s = self.scale();
                let d = self.dirty;
                let r = D2D_RECT_F { left: d.left as f32 / s, top: d.top as f32 / s, right: d.right as f32 / s, bottom: d.bottom as f32 / s };
                self.main.PushAxisAlignedClip(&r, D2D1_ANTIALIAS_MODE_ALIASED);
                self.main.Clear(Some(&CLEAR));
                self.main.PopAxisAlignedClip();
            }
        }
        self.rt = self.main.clone();
        Ok(Canvas { g: self, alpha: 1.0, snap_text: true })
    }

    /// Finish the frame. Evicts bitmaps and text layouts that weren't used
    /// recently once the caches grow large.
    pub fn end(&mut self) -> Result<()> {
        self.glass_drawn = false;
        // Balance any shape masks a caller forgot to pop.
        while let Some(pushed) = self.mask_stack.pop() {
            if pushed {
                unsafe { self.main.PopLayer() };
            }
        }
        self.rt = self.main.clone();
        let r = unsafe { self.main.EndDraw(None, None) };
        let capture = self.capture.take();
        match self.gpu.as_mut() {
            Some(g) => {
                if let (Ok(()), Some(c)) = (&r, capture) {
                    match unsafe { g.read(c) } {
                        Ok(px) => self.captured = px,
                        Err(e) => crate::pwarn!("frame read-back failed: {e}"),
                    }
                }
                unsafe { g.end() };
                if let Err(e) = &r {
                    self.gpu_lost("draw", e.clone());
                }
            }
            None => {
                if let Some(c) = capture {
                    self.captured = self.soft.pixels(c);
                }
            }
        }
        if self.bitmaps.len() > 48 {
            let used = std::mem::take(&mut self.used_bitmaps);
            self.bitmaps.retain(|k, _| used.contains(k));
        }
        if self.frame_no % 120 == 0 || self.layouts.len() > 1500 {
            let keep_after = self.frame_no.saturating_sub(if self.layouts.len() > 1500 { 2 } else { 240 });
            self.layouts.retain(|_, (_, used)| *used >= keep_after);
        }
        r
    }

    /// Show the frame. The window sits at (`x`, `y`) screen pixels.
    pub fn present(&mut self, x: i32, y: i32) -> Result<()> {
        if self.gpu.is_some() {
            self.place_visual(x, y);
        }
        if let Some(g) = self.gpu.as_ref() {
            if let Err(e) = unsafe { g.swap.Present(1, DXGI_PRESENT(0)) }.ok() {
                self.gpu_lost("present", e.clone());
                return Err(e);
            }
            self.gpu_frames = self.gpu_frames.saturating_add(1);
            if self.gpu_frames > 600 {
                self.gpu_failures = 0;
            }
            return Ok(());
        }
        let dirty = std::mem::take(&mut self.dirty);
        let r = unsafe { self.soft.push(self.hwnd, x, y, self.surf_w, self.surf_h, (!self.soft_full).then_some(dirty)) };
        if r.is_ok() {
            self.soft_full = false;
        }
        r
    }

    /// GPU renderer: put the visual window where the island's window is,
    /// (`x`, `y`) at the surface's size, before a frame shows in it.
    fn place_visual(&mut self, x: i32, y: i32) {
        let Some(v) = self.visual else { return };
        let at = (x, y, self.surf_w, self.surf_h);
        if self.visual_at == Some(at) {
            return;
        }
        match unsafe { self.soft.push_clear(v, x, y, self.surf_w, self.surf_h) } {
            Ok(()) => {
                self.visual_at = Some(at);
                if self.shown {
                    unsafe {
                        let _ = ShowWindow(v, SW_SHOWNOACTIVATE);
                    }
                }
            }
            Err(e) => crate::pwarn!("visual window: placing it failed ({e})"),
        }
    }

    /// The app shows or hides the island: the visual window follows while
    /// the GPU renderer draws in it.
    pub fn set_shown(&mut self, on: bool) {
        self.shown = on;
        if let Some(v) = self.visual.filter(|_| self.gpu.is_some() && self.visual_at.is_some()) {
            unsafe {
                let _ = ShowWindow(v, if on { SW_SHOWNOACTIVATE } else { SW_HIDE });
            }
        }
    }

    /// GPU renderer: update the window's own bitmap, an invisible mask that
    /// decides where the island takes the mouse. `prev` is the last mask
    /// pushed (only the area of either is redrawn).
    pub fn push_hit_mask(&mut self, x: i32, y: i32, mask: &HitMask, prev: Option<&HitMask>) -> Result<()> {
        let s = self.scale();
        let (w, h) = (self.surf_w, self.surf_h);
        let full = RECT { left: 0, top: 0, right: w, bottom: h };
        let area = |m: &HitMask| m.bounds().map(|b| px_rect(b, s, 2, w, h)).unwrap_or_default();
        let dirty = match prev {
            Some(p) if !self.soft_full => px_union(area(p), area(mask)),
            _ => full,
        };
        unsafe {
            self.soft.begin(w, h, self.dpi)?;
            let rt = &self.soft.rt;
            rt.SetTransform(&Matrix3x2::identity());
            let r = D2D_RECT_F { left: dirty.left as f32 / s, top: dirty.top as f32 / s, right: dirty.right as f32 / s, bottom: dirty.bottom as f32 / s };
            rt.PushAxisAlignedClip(&r, D2D1_ANTIALIAS_MODE_ALIASED);
            rt.Clear(Some(&CLEAR));
            if let Some(r) = mask.rect {
                rt.FillRectangle(&rectf(r), &self.mask_brush);
            }
            if let Some((body, radius, shoulder)) = mask.notch {
                if let Ok(geo) = self.path_geometry(&notch_segments(body, radius, shoulder), true) {
                    rt.FillGeometry(&geo, &self.mask_brush, None);
                }
            }
            if let Some((body, radius)) = mask.pill {
                if let Ok(geo) = self.path_geometry(&crate::pill::shape::segments(body, radius), true) {
                    rt.FillGeometry(&geo, &self.mask_brush, None);
                }
            }
            rt.PopAxisAlignedClip();
            rt.EndDraw(None, None)?;
            let full_push = self.soft_full || prev.is_none();
            self.soft.push(self.hwnd, x, y, w, h, (!full_push).then_some(dirty))?;
        }
        self.soft_full = false;
        Ok(())
    }

    /// An offscreen layer the size of the surface (see [`Canvas::layered`]).
    fn layer(&mut self) -> Result<ID2D1BitmapRenderTarget> {
        let i = self.layers_used;
        self.layers_used += 1;
        if let Some(l) = self.layers.get(i) {
            return Ok(l.clone());
        }
        let s = self.scale();
        let px = D2D_SIZE_U { width: self.surf_w.max(1) as u32, height: self.surf_h.max(1) as u32 };
        let dip = D2D_SIZE_F { width: px.width as f32 / s, height: px.height as f32 / s };
        let l = unsafe { self.main.CreateCompatibleRenderTarget(Some(&dip), Some(&px), Some(&PREMUL_BGRA), D2D1_COMPATIBLE_RENDER_TARGET_OPTIONS_NONE)? };
        setup_target(&l.cast()?);
        self.layers.push(l.clone());
        Ok(l)
    }

    fn format(&mut self, size: f32, weight: Weight, wrap: bool) -> Result<IDWriteTextFormat> {
        let key = ((size * 100.0) as u32, weight, wrap);
        if let Some(f) = self.formats.get(&key) {
            return Ok(f.clone());
        }
        let coll = if self.family != self.system_family { self.embedded.clone() } else { None };
        let f = unsafe {
            let f = self.dwrite.CreateTextFormat(
                PCWSTR(self.family.as_ptr()),
                coll.as_ref(),
                weight.dwrite(),
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size,
                w!("en-us"),
            )?;
            f.SetWordWrapping(if wrap { DWRITE_WORD_WRAPPING_WRAP } else { DWRITE_WORD_WRAPPING_NO_WRAP })?;
            if !wrap {
                let sign = self.dwrite.CreateEllipsisTrimmingSign(&f)?;
                let trimming = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
                f.SetTrimming(&trimming, &sign)?;
            }
            f
        };
        self.formats.insert(key, f.clone());
        Ok(f)
    }

    /// A text layout, reused across frames: at 144 fps re-shaping every
    /// string every frame would be most of the frame's CPU time.
    fn layout(&mut self, s: &str, st: &TextStyle, w: f32, h: f32) -> Result<IDWriteTextLayout> {
        let (w, h) = (w.max(1.0), h.max(1.0));
        let key = LayoutKey {
            text: s.to_string(),
            size: st.size.to_bits(),
            weight: st.weight,
            wrap: st.wrap,
            align: st.align,
            vcenter: st.vcenter,
            tabular: st.tabular,
            line: st.line.to_bits(),
            w: w.to_bits(),
            h: h.to_bits(),
        };
        let frame = self.frame_no;
        if let Some((l, used)) = self.layouts.get_mut(&key) {
            *used = frame;
            return Ok(l.clone());
        }
        let fmt = self.format(st.size, st.weight, st.wrap)?;
        let text = wide(s);
        let layout = unsafe {
            let layout = self.dwrite.CreateTextLayout(&text, &fmt, w, h)?;
            layout.SetTextAlignment(match st.align {
                Align::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
                Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
                Align::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
            })?;
            layout.SetParagraphAlignment(if st.vcenter { DWRITE_PARAGRAPH_ALIGNMENT_CENTER } else { DWRITE_PARAGRAPH_ALIGNMENT_NEAR })?;
            if st.line > 0.0 {
                layout.SetLineSpacing(DWRITE_LINE_SPACING_METHOD_UNIFORM, st.line * st.size, 0.8 * st.line * st.size)?;
            }
            if st.tabular && !text.is_empty() {
                let typo = self.dwrite.CreateTypography()?;
                typo.AddFontFeature(DWRITE_FONT_FEATURE { nameTag: DWRITE_FONT_FEATURE_TAG_TABULAR_FIGURES, parameter: 1 })?;
                layout.SetTypography(&typo, DWRITE_TEXT_RANGE { startPosition: 0, length: text.len() as u32 })?;
            }
            layout
        };
        self.layouts.insert(key, (layout.clone(), frame));
        Ok(layout)
    }

    fn bitmap_for(&mut self, img: &Image) -> Result<ID2D1Bitmap> {
        self.used_bitmaps.push(img.id);
        if let Some(b) = self.bitmaps.get(&img.id) {
            return Ok(b.clone());
        }
        let bmp = unsafe {
            self.main.CreateBitmap(
                D2D_SIZE_U { width: img.width, height: img.height },
                Some(img.pixels.as_ptr() as *const c_void),
                img.width * 4,
                &D2D1_BITMAP_PROPERTIES {
                    pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                    dpiX: 96.0,
                    dpiY: 96.0,
                },
            )?
        };
        self.bitmaps.insert(img.id, bmp.clone());
        Ok(bmp)
    }

    /// Build Direct2D geometry from parsed SVG segments.
    pub fn path_geometry(&self, segs: &[Seg], close_fill: bool) -> Result<ID2D1PathGeometry> {
        unsafe {
            let geo = self.d2d.CreatePathGeometry()?;
            let sink = geo.Open()?;
            sink.SetFillMode(D2D1_FILL_MODE_WINDING);
            let mut open = false;
            let mut cur = (0.0f32, 0.0f32);
            let begin = if close_fill { D2D1_FIGURE_BEGIN_FILLED } else { D2D1_FIGURE_BEGIN_HOLLOW };
            for s in segs {
                match *s {
                    Seg::MoveTo(x, y) => {
                        if open {
                            sink.EndFigure(D2D1_FIGURE_END_OPEN);
                        }
                        sink.BeginFigure(pt(x, y), begin);
                        open = true;
                        cur = (x, y);
                    }
                    Seg::LineTo(x, y) => {
                        if !open {
                            sink.BeginFigure(pt(cur.0, cur.1), begin);
                            open = true;
                        }
                        sink.AddLine(pt(x, y));
                        cur = (x, y);
                    }
                    Seg::CubicTo(x1, y1, x2, y2, x, y) => {
                        if !open {
                            sink.BeginFigure(pt(cur.0, cur.1), begin);
                            open = true;
                        }
                        sink.AddBezier(&D2D1_BEZIER_SEGMENT { point1: pt(x1, y1), point2: pt(x2, y2), point3: pt(x, y) });
                        cur = (x, y);
                    }
                    Seg::QuadTo(x1, y1, x, y) => {
                        if !open {
                            sink.BeginFigure(pt(cur.0, cur.1), begin);
                            open = true;
                        }
                        sink.AddQuadraticBezier(&D2D1_QUADRATIC_BEZIER_SEGMENT { point1: pt(x1, y1), point2: pt(x, y) });
                        cur = (x, y);
                    }
                    Seg::ArcTo(rx, ry, rot, large, sweep, x, y) => {
                        if !open {
                            sink.BeginFigure(pt(cur.0, cur.1), begin);
                            open = true;
                        }
                        // Flatten to Béziers ourselves: native arc support varies
                        // between Direct2D implementations (Wine draws chords).
                        for c in poni_core::svgpath::arc_to_cubics(cur.0, cur.1, rx, ry, rot, large, sweep, x, y) {
                            sink.AddBezier(&D2D1_BEZIER_SEGMENT { point1: pt(c[0], c[1]), point2: pt(c[2], c[3]), point3: pt(c[4], c[5]) });
                        }
                        cur = (x, y);
                    }
                    Seg::Close => {
                        if open {
                            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                            open = false;
                        }
                    }
                }
            }
            if open {
                sink.EndFigure(D2D1_FIGURE_END_OPEN);
            }
            sink.Close()?;
            Ok(geo)
        }
    }

    fn icon_geometry(&mut self, icon: Icon) -> Vec<(ID2D1PathGeometry, bool)> {
        if let Some(g) = self.icon_geoms.get(&icon) {
            return g.clone();
        }
        let mut out = Vec::new();
        for part in icons::parts(icon) {
            match poni_core::svgpath::parse(part.d) {
                Ok(segs) => match self.path_geometry(&segs, part.fill) {
                    Ok(geo) => out.push((geo, part.fill)),
                    Err(e) => crate::pwarn!("icon {icon:?} geometry failed: {e}"),
                },
                Err(e) => crate::pwarn!("icon {icon:?} path parse failed: {}", e.0),
            }
        }
        self.icon_geoms.insert(icon, out.clone());
        out
    }

    /// The gradient brushes [`Canvas::rrect_shadow`] fills with for `stops`
    /// in `c` (its opacity is set per fill), made on first use.
    fn shadow_brushes(&mut self, stops: &[(f32, f32)], c: Color) -> Option<(ID2D1RadialGradientBrush, ID2D1LinearGradientBrush)> {
        // FNV-1a over the stops and the colour.
        let key = stops
            .iter()
            .flat_map(|&(p, a)| [p, a])
            .chain([c.r, c.g, c.b])
            .fold(0xcbf2_9ce4_8422_2325u64, |h, v| (h ^ v.to_bits() as u64).wrapping_mul(0x0100_0000_01b3));
        if let Some(b) = self.shadows.get(&key) {
            return Some(b.clone());
        }
        let grad: Vec<D2D1_GRADIENT_STOP> = stops.iter().map(|&(position, a)| D2D1_GRADIENT_STOP { position, color: d2d_color(c.with_alpha(a)) }).collect();
        let made = unsafe {
            self.main.CreateGradientStopCollection(&grad, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP).and_then(|coll| {
                let radial = D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES { center: pt(0.0, 0.0), gradientOriginOffset: pt(0.0, 0.0), radiusX: 1.0, radiusY: 1.0 };
                let linear = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES { startPoint: pt(0.0, 0.0), endPoint: pt(0.0, 1.0) };
                Ok((self.main.CreateRadialGradientBrush(&radial, None, &coll)?, self.main.CreateLinearGradientBrush(&linear, None, &coll)?))
            })
        };
        let b = made.ok()?;
        // A morph walks through radii (whole half DIPs); a long session
        // never keeps more than a few pages of them.
        if self.shadows.len() >= 96 {
            self.shadows.clear();
        }
        self.shadows.insert(key, b.clone());
        Some(b)
    }
}

// ---------------------------------------------------------------- Canvas

/// Drawing API used by pages and activities. Coordinates are DIPs.
pub struct Canvas<'a> {
    g: &'a mut Gfx,
    /// Global opacity multiplier applied to everything drawn.
    alpha: f32,
    /// Text lands on whole pixels vertically (crisp at rest). Off while text
    /// moves, so it glides instead of ticking a pixel at a time.
    snap_text: bool,
}

impl<'a> Canvas<'a> {
    /// This frame draws on the GPU (the window's bitmap is then the hit mask).
    pub fn on_gpu(&self) -> bool {
        self.g.gpu.is_some()
    }

    pub fn alpha(&self) -> f32 {
        self.alpha
    }

    pub fn set_alpha(&mut self, a: f32) {
        self.alpha = a.clamp(0.0, 1.0);
    }

    /// Run `f` with opacity multiplied by `a`.
    pub fn with_alpha<R>(&mut self, a: f32, f: impl FnOnce(&mut Canvas<'a>) -> R) -> R {
        let prev = self.alpha;
        self.alpha = (prev * a).clamp(0.0, 1.0);
        let r = f(self);
        self.alpha = prev;
        r
    }

    /// Snap text to whole pixels vertically (`true`, the default: crisp) or
    /// not (`false`: for text that is moving, which Direct2D would otherwise
    /// move a whole pixel at a time). Returns the previous setting.
    pub fn set_text_snap(&mut self, on: bool) -> bool {
        std::mem::replace(&mut self.snap_text, on)
    }

    fn set_brush(&self, c: Color) {
        let c = self.g.look.paint(c);
        unsafe { self.g.brush.SetColor(&d2d_color(c.mul_alpha(self.alpha))) };
    }

    fn set_text_brush(&self, c: Color) {
        let c = self.g.look.paint_text(c);
        unsafe { self.g.brush.SetColor(&d2d_color(c.mul_alpha(self.alpha))) };
    }

    /// The theme's look (how the canvas paints the colours it is given).
    pub fn look(&self) -> &'static Look {
        self.g.look
    }

    /// Run `f` painting colours exactly as given, outside the theme (Oyen's
    /// own face).
    pub fn unthemed<R>(&mut self, f: impl FnOnce(&mut Canvas<'a>) -> R) -> R {
        let prev = self.g.look;
        self.g.look = &look::CLASSIC;
        let r = f(self);
        self.g.look = prev;
        r
    }

    pub fn scale(&self) -> f32 {
        self.g.scale()
    }

    pub fn fill_rect(&mut self, r: Rect, c: Color) {
        self.set_brush(c);
        unsafe { self.g.rt.FillRectangle(&rectf(r), &self.g.brush) };
    }

    pub fn fill_rrect(&mut self, r: Rect, radius: f32, c: Color) {
        self.set_brush(c);
        let rr = D2D1_ROUNDED_RECT { rect: rectf(r), radiusX: radius, radiusY: radius };
        unsafe { self.g.rt.FillRoundedRectangle(&rr, &self.g.brush) };
    }

    pub fn stroke_rrect(&mut self, r: Rect, radius: f32, c: Color, width: f32) {
        self.set_brush(c);
        let rr = D2D1_ROUNDED_RECT { rect: rectf(r.inset(width / 2.0, width / 2.0)), radiusX: radius, radiusY: radius };
        unsafe { self.g.rt.DrawRoundedRectangle(&rr, &self.g.brush, width, &self.g.round_stroke) };
    }

    pub fn fill_circle(&mut self, c: Point, radius: f32, color: Color) {
        self.set_brush(color);
        let e = D2D1_ELLIPSE { point: pt(c.x, c.y), radiusX: radius, radiusY: radius };
        unsafe { self.g.rt.FillEllipse(&e, &self.g.brush) };
    }

    pub fn stroke_circle(&mut self, c: Point, radius: f32, color: Color, width: f32) {
        self.set_brush(color);
        let e = D2D1_ELLIPSE { point: pt(c.x, c.y), radiusX: radius, radiusY: radius };
        unsafe { self.g.rt.DrawEllipse(&e, &self.g.brush, width, &self.g.round_stroke) };
    }

    pub fn line(&mut self, a: Point, b: Point, c: Color, width: f32) {
        self.set_brush(c);
        unsafe { self.g.rt.DrawLine(pt(a.x, a.y), pt(b.x, b.y), &self.g.brush, width, &self.g.round_stroke) };
    }

    /// Arc clockwise from 12 o'clock: starts at `start` (0..1 of a turn) and
    /// spans `sweep` (0..1). Round caps.
    pub fn arc(&mut self, c: Point, radius: f32, start: f32, sweep: f32, color: Color, width: f32) {
        let sweep = sweep.clamp(0.0, 1.0);
        if sweep <= 0.0005 {
            return;
        }
        if sweep >= 0.9995 {
            self.stroke_circle(c, radius, color, width);
            return;
        }
        let ang = |f: f32| (f * std::f32::consts::TAU) - std::f32::consts::FRAC_PI_2;
        let (a0, a1) = (ang(start), ang(start + sweep));
        let p0 = (c.x + radius * a0.cos(), c.y + radius * a0.sin());
        let p1 = (c.x + radius * a1.cos(), c.y + radius * a1.sin());
        let segs = [Seg::MoveTo(p0.0, p0.1), Seg::ArcTo(radius, radius, 0.0, sweep > 0.5, true, p1.0, p1.1)];
        if let Ok(geo) = self.g.path_geometry(&segs, false) {
            self.set_brush(color);
            unsafe { self.g.rt.DrawGeometry(&geo, &self.g.brush, width, &self.g.round_stroke) };
        }
    }

    /// Progress ring: full track circle plus a progress arc on top.
    pub fn ring(&mut self, c: Point, radius: f32, width: f32, progress: f32, track: Color, fill: Color) {
        self.stroke_circle(c, radius, track, width);
        self.arc(c, radius, 0.0, progress, fill, width);
    }

    /// Fill and/or stroke arbitrary path segments.
    pub fn path(&mut self, segs: &[Seg], fill: Option<Color>, stroke: Option<(Color, f32)>) {
        if let Ok(geo) = self.g.path_geometry(segs, fill.is_some()) {
            unsafe {
                if let Some(f) = fill {
                    self.set_brush(f);
                    self.g.rt.FillGeometry(&geo, &self.g.brush, None);
                }
                if let Some((c, w)) = stroke {
                    self.set_brush(c);
                    self.g.rt.DrawGeometry(&geo, &self.g.brush, w, &self.g.round_stroke);
                }
            }
        }
    }

    /// Fill a path with `c` fading from `alpha0` at height `y0` to `alpha1`
    /// at `y1` (a vertical gradient, as under Stats' charts).
    pub fn fill_path_fade(&mut self, segs: &[Seg], c: Color, y0: f32, y1: f32, alpha0: f32, alpha1: f32) {
        let Ok(geo) = self.g.path_geometry(segs, true) else { return };
        let c = self.g.look.paint(c);
        let stop = |position: f32, a: f32| D2D1_GRADIENT_STOP { position, color: d2d_color(c.with_alpha(c.a * a)) };
        let stops = [stop(0.0, alpha0), stop(1.0, alpha1)];
        unsafe {
            let Ok(coll) = self.g.rt.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP) else { return };
            let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES { startPoint: pt(0.0, y0), endPoint: pt(0.0, y1) };
            let Ok(brush) = self.g.rt.CreateLinearGradientBrush(&props, None, &coll) else { return };
            brush.SetOpacity(self.alpha);
            self.g.rt.FillGeometry(&geo, &brush, None);
        }
    }

    /// A soft shadow round the rounded rectangle `r` (corner `radius`): `c`
    /// at the opacity `ramp` gives by distance from the edge (DIPs, negative
    /// inside, in order), from its first stop inside the edge to its last
    /// outside it, where it should reach 0. Deeper inside than the first
    /// stop it may draw or not: cover that, as the floating island's body
    /// covers its shadow. A few gradient fills (corners and sides: four for
    /// a capsule, eight for a card, see `pill::shape::shadow_plan`) that meet
    /// on whole pixels, so draw it without a content transform. The
    /// gradients are kept between frames.
    pub fn rrect_shadow(&mut self, r: Rect, radius: f32, c: Color, ramp: &[(f32, f32)]) {
        let Some(plan) = crate::pill::shape::shadow_plan(r, radius, ramp, self.scale()) else { return };
        let c = self.g.look.paint(c);
        let opacity = (c.a * self.alpha).clamp(0.0, 1.0);
        if opacity <= 0.0 {
            return;
        }
        let Some((radial, linear)) = self.g.shadow_brushes(&plan.stops, c) else { return };
        unsafe {
            radial.SetOpacity(opacity);
            radial.SetRadiusX(plan.reach);
            radial.SetRadiusY(plan.reach);
            for (square, o) in plan.corners.into_iter().flatten() {
                radial.SetCenter(pt(o.x, o.y));
                self.g.rt.FillRectangle(&rectf(square), &radial);
            }
            linear.SetOpacity(opacity);
            for (strip, a, b) in plan.sides.into_iter().flatten() {
                linear.SetStartPoint(pt(a.x, a.y));
                linear.SetEndPoint(pt(b.x, b.y));
                self.g.rt.FillRectangle(&rectf(strip), &linear);
            }
            if let Some((m, a)) = plan.middle {
                self.g.brush.SetColor(&d2d_color(c.with_alpha(a * opacity)));
                self.g.rt.FillRectangle(&rectf(m), &self.g.brush);
            }
        }
    }

    /// The island silhouette: flat top flush with the screen edge, concave
    /// "shoulders" flaring into the top edge and rounded bottom corners.
    /// `body` is the main rectangle; shoulders extend `shoulder` DIPs outside it.
    pub fn notch(&mut self, body: Rect, radius: f32, shoulder: f32, fill: Color, border: Option<Color>) {
        self.path(&notch_segments(body, radius, shoulder), Some(fill), None);
        if let Some(b) = border {
            // Stroke only the sides and bottom: stroking the closing top segment
            // or the shoulders leaves a grey seam against the screen edge.
            let s = shoulder.max(0.0);
            let r = radius.min(body.h).min(body.w / 2.0).max(0.0);
            let (x0, x1, y1) = (body.x, body.right(), body.bottom());
            let top = s.min(body.h);
            let segs = [
                Seg::MoveTo(x0, top),
                Seg::LineTo(x0, (y1 - r).max(top)),
                Seg::ArcTo(r, r, 0.0, false, false, x0 + r, y1),
                Seg::LineTo(x1 - r, y1),
                Seg::ArcTo(r, r, 0.0, false, false, x1, (y1 - r).max(top)),
                Seg::LineTo(x1, top),
            ];
            self.path(&segs, None, Some((b, 1.0)));
        }
    }

    /// The island body in the Glass theme: the liquid glass drawn before the
    /// frame ([`Gfx::prepare_glass`]) over `area`, else smoked glass.
    pub fn glass(&mut self, p: &GlassParams, area: Rect) {
        if self.g.glass_drawn {
            if let Some(bmp) = self.g.glass.as_ref().and_then(|x| x.output()).cloned() {
                let r = rectf(area);
                unsafe { self.g.rt.DrawBitmap(&bmp, Some(&r), self.alpha, D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR, Some(&r)) };
                return;
            }
        }
        self.unthemed(|c| c.smoked_glass(p));
    }

    /// Glass without a view of the screen: the look's ground, see-through so
    /// the screen still shows under it, a soft shadow and the rim light.
    fn smoked_glass(&mut self, p: &GlassParams) {
        for i in 1..=3 {
            let grow = i as f32 * p.shadow_reach / 3.0;
            let b = p.body;
            let shadow = Rect::new(b.x - grow, b.y, b.w + 2.0 * grow, b.h + p.shadow_drop + grow);
            self.path(&notch_segments(shadow, p.radius + grow, p.shoulder), Some(Color::black(p.shadow_alpha / 4.0)), None);
        }
        let ground = Color::lerp(p.ground_dark, p.ground_light, p.light_mix).with_alpha(poni_core::glass::SMOKED_OPACITY);
        self.notch(p.body, p.radius, p.shoulder, ground, Some(Color::white((0.55 * p.rim).clamp(0.0, 1.0))));
    }

    /// Mask subsequent drawing to the island silhouette (anti-aliased) until
    /// [`Canvas::pop_shape_mask`]. Push it before any content scale transform.
    pub fn push_shape_mask(&mut self, body: Rect, radius: f32, shoulder: f32) {
        self.push_outline_mask(&notch_segments(body, radius, shoulder));
    }

    /// Mask subsequent drawing to the closed outline `segs` (anti-aliased)
    /// until [`Canvas::pop_shape_mask`] (the floating island's outline).
    pub fn push_outline_mask(&mut self, segs: &[Seg]) {
        let pushed = match self.g.path_geometry(segs, true).and_then(|g| g.cast::<ID2D1Geometry>()) {
            Ok(geom) => unsafe {
                let params = D2D1_LAYER_PARAMETERS {
                    contentBounds: D2D_RECT_F { left: -1.0e7, top: -1.0e7, right: 1.0e7, bottom: 1.0e7 },
                    geometricMask: ManuallyDrop::new(Some(geom)),
                    maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                    maskTransform: Matrix3x2::identity(),
                    opacity: 1.0,
                    opacityBrush: ManuallyDrop::new(None),
                    layerOptions: D2D1_LAYER_OPTIONS_NONE,
                };
                self.g.rt.PushLayer(&params, None::<&ID2D1Layer>);
                drop(ManuallyDrop::into_inner(params.geometricMask));
                true
            },
            Err(_) => false,
        };
        self.g.mask_stack.push(pushed);
    }

    pub fn pop_shape_mask(&mut self) {
        if let Some(true) = self.g.mask_stack.pop() {
            unsafe { self.g.rt.PopLayer() };
        }
    }

    /// Draw text in `r`. Single-line text is ellipsized; see [`TextStyle`].
    pub fn text(&mut self, s: &str, r: Rect, st: &TextStyle) {
        if s.is_empty() || r.w <= 0.0 || r.h <= 0.0 {
            return;
        }
        if let Ok(layout) = self.g.layout(s, st, r.w, r.h) {
            self.set_text_brush(st.color);
            let opts = if self.snap_text { D2D1_DRAW_TEXT_OPTIONS_NONE } else { D2D1_DRAW_TEXT_OPTIONS_NO_SNAP };
            unsafe { self.g.rt.DrawTextLayout(pt(r.x, r.y), &layout, &self.g.brush, opts) };
        }
    }

    /// Measure text: (width, height) in DIPs, laid out within `max_w`.
    pub fn measure(&mut self, s: &str, st: &TextStyle, max_w: f32) -> (f32, f32) {
        let mut st = *st;
        st.align = Align::Left;
        match self.g.layout(s, &st, max_w.max(1.0), 10_000.0) {
            Ok(layout) => unsafe {
                let mut m = DWRITE_TEXT_METRICS::default();
                if layout.GetMetrics(&mut m).is_ok() {
                    (m.widthIncludingTrailingWhitespace, m.height)
                } else {
                    (0.0, st.size * 1.3)
                }
            },
            Err(_) => (0.0, st.size * 1.3),
        }
    }

    /// Draw an image cover-fitted into `r` with rounded corners.
    pub fn image(&mut self, img: &Image, r: Rect, radius: f32) {
        if img.width == 0 || img.height == 0 || r.w <= 0.0 || r.h <= 0.0 {
            return;
        }
        let Ok(bmp) = self.g.bitmap_for(img) else { return };
        unsafe {
            let props = D2D1_BITMAP_BRUSH_PROPERTIES {
                extendModeX: D2D1_EXTEND_MODE_CLAMP,
                extendModeY: D2D1_EXTEND_MODE_CLAMP,
                interpolationMode: D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
            };
            let Ok(brush) = self.g.rt.CreateBitmapBrush(&bmp, Some(&props), None) else { return };
            // Bitmaps are created at 96 DPI so their DIP size equals pixel size.
            let (iw, ih) = (img.width as f32, img.height as f32);
            let s = (r.w / iw).max(r.h / ih);
            let (dw, dh) = (iw * s, ih * s);
            let ox = r.x + (r.w - dw) / 2.0;
            let oy = r.y + (r.h - dh) / 2.0;
            brush.SetTransform(&Matrix3x2 { M11: s, M12: 0.0, M21: 0.0, M22: s, M31: ox, M32: oy });
            brush.SetOpacity(self.alpha);
            let rr = D2D1_ROUNDED_RECT { rect: rectf(r), radiusX: radius, radiusY: radius };
            self.g.rt.FillRoundedRectangle(&rr, &brush);
        }
    }

    /// Draw a vector icon (24×24 design grid) scaled into `r`.
    pub fn icon(&mut self, icon: Icon, r: Rect, color: Color) {
        let geoms = self.g.icon_geometry(icon);
        if geoms.is_empty() {
            return;
        }
        let s = r.w.min(r.h) / 24.0;
        let ox = r.x + (r.w - 24.0 * s) / 2.0;
        let oy = r.y + (r.h - 24.0 * s) / 2.0;
        // Icons are glyphs: they take a coloured label's shade, not a fill's.
        self.set_text_brush(color);
        unsafe {
            let mut prev = Matrix3x2::identity();
            self.g.rt.GetTransform(&mut prev);
            let m = Matrix3x2 { M11: s, M12: 0.0, M21: 0.0, M22: s, M31: ox, M32: oy };
            self.g.rt.SetTransform(&(m * prev));
            for (geo, fill) in &geoms {
                if *fill {
                    self.g.rt.FillGeometry(geo, &self.g.brush, None);
                } else {
                    self.g.rt.DrawGeometry(geo, &self.g.brush, icons::STROKE, &self.g.round_stroke);
                }
            }
            self.g.rt.SetTransform(&prev);
        }
    }

    /// Clip subsequent drawing to `r` until [`Canvas::unclip`].
    pub fn clip(&mut self, r: Rect) {
        unsafe { self.g.rt.PushAxisAlignedClip(&rectf(r), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE) };
    }

    pub fn unclip(&mut self) {
        unsafe { self.g.rt.PopAxisAlignedClip() };
    }

    /// Translate subsequent drawing by (dx, dy) until [`Canvas::reset_transform`].
    pub fn translate(&mut self, dx: f32, dy: f32) {
        unsafe {
            let mut prev = Matrix3x2::identity();
            self.g.rt.GetTransform(&mut prev);
            self.g.rt.SetTransform(&(Matrix3x2::translation(dx, dy) * prev));
        }
    }

    pub fn reset_transform(&mut self) {
        unsafe { self.g.rt.SetTransform(&Matrix3x2::identity()) };
    }

    /// The current transform (pair with [`Canvas::restore_transform`]
    /// instead of `reset_transform`, which would also drop outer transforms).
    pub fn save_transform(&self) -> Matrix3x2 {
        let mut m = Matrix3x2::identity();
        unsafe { self.g.rt.GetTransform(&mut m) };
        m
    }

    pub fn restore_transform(&mut self, m: Matrix3x2) {
        unsafe { self.g.rt.SetTransform(&m) };
    }

    /// Scale subsequent drawing by `s` about point `p` (on top of the current transform).
    pub fn scale_about(&mut self, p: Point, s: f32) {
        let prev = self.save_transform();
        let m = Matrix3x2 { M11: s, M12: 0.0, M21: 0.0, M22: s, M31: p.x * (1.0 - s), M32: p.y * (1.0 - s) };
        unsafe { self.g.rt.SetTransform(&(m * prev)) };
    }

    /// Draw `f` into an offscreen layer covering `bounds`, then composite the
    /// layer at opacity `alpha`, moved by `xf` (on top of the current
    /// transform). The group fades as one image (overlapping parts don't
    /// show through each other) and its text is rasterised once at rest
    /// size, so it glides while it scales instead of re-hinting every frame.
    /// `f` always runs, even when the layer ends up invisible, so hit regions
    /// and animation state stay continuous.
    pub fn layered<R>(&mut self, bounds: Rect, alpha: f32, xf: Matrix3x2, f: impl FnOnce(&mut Canvas<'a>) -> R) -> R {
        let s = self.scale().max(0.1);
        // Whole pixels: with an identity `xf` the layer lands 1:1 on the frame.
        let (x0, y0) = ((bounds.x * s).floor() / s, (bounds.y * s).floor() / s);
        let (x1, y1) = ((bounds.right() * s).ceil() / s, (bounds.bottom() * s).ceil() / s);
        let b = rectf(Rect::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0)));
        let Ok(layer) = self.g.layer() else {
            // No offscreen target: per-primitive opacity, live transform.
            let prev = self.save_transform();
            unsafe { self.g.rt.SetTransform(&(xf * prev)) };
            let r = self.with_alpha(alpha, f);
            self.restore_transform(prev);
            return r;
        };
        let Ok(layer_rt) = layer.cast::<ID2D1RenderTarget>() else { return self.with_alpha(alpha, f) };
        unsafe {
            layer_rt.BeginDraw();
            layer_rt.SetTransform(&Matrix3x2::identity());
            layer_rt.PushAxisAlignedClip(&b, D2D1_ANTIALIAS_MODE_ALIASED);
            layer_rt.Clear(Some(&CLEAR));
        }
        let outer = std::mem::replace(&mut self.g.rt, layer_rt.clone());
        let outer_alpha = self.alpha;
        let masks = self.g.mask_stack.len();
        self.alpha = 1.0;
        let r = f(self);
        while self.g.mask_stack.len() > masks {
            if self.g.mask_stack.pop() == Some(true) {
                unsafe { layer_rt.PopLayer() };
            }
        }
        self.alpha = outer_alpha;
        self.g.rt = outer;
        unsafe {
            layer_rt.PopAxisAlignedClip();
            let drawn = layer_rt.EndDraw(None, None).is_ok();
            let a = (alpha * outer_alpha).clamp(0.0, 1.0);
            if drawn && a > 0.002 {
                if let Ok(bmp) = layer.GetBitmap() {
                    let prev = self.save_transform();
                    self.g.rt.SetTransform(&(xf * prev));
                    self.g.rt.DrawBitmap(&bmp, Some(&b), a, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, Some(&b));
                    self.g.rt.SetTransform(&prev);
                }
            }
        }
        r
    }
}

/// The island outline as path segments (flat top, concave shoulders, rounded bottom).
pub fn notch_segments(body: Rect, radius: f32, shoulder: f32) -> Vec<Seg> {
    let s = shoulder.max(0.0);
    let r = radius.min(body.h).min(body.w / 2.0).max(0.0);
    let (x0, x1, y1) = (body.x, body.right(), body.bottom());
    let mut segs = vec![Seg::MoveTo(x0 - s, 0.0)];
    if s > 0.0 {
        segs.push(Seg::ArcTo(s, s, 0.0, false, true, x0, s.min(body.h)));
    }
    segs.push(Seg::LineTo(x0, (y1 - r).max(0.0)));
    segs.push(Seg::ArcTo(r, r, 0.0, false, false, x0 + r, y1));
    segs.push(Seg::LineTo(x1 - r, y1));
    segs.push(Seg::ArcTo(r, r, 0.0, false, false, x1, (y1 - r).max(0.0)));
    segs.push(Seg::LineTo(x1, s.min(body.h)));
    if s > 0.0 {
        segs.push(Seg::ArcTo(s, s, 0.0, false, true, x1 + s, 0.0));
    }
    segs.push(Seg::Close);
    segs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(x: f32, y: f32) -> Point {
        Point::new(x, y)
    }

    #[test]
    fn mask_bounds_cover_each_shape() {
        let body = Rect::new(100.0, 0.0, 84.0, 30.0);
        let notch = HitMask { rect: None, notch: Some((body, 14.0, 9.0)), pill: None };
        assert_eq!(notch.bounds(), Some(Rect::new(91.0, 0.0, 102.0, 30.0)), "Classic: the body and both shoulders");

        let floating = Rect::new(100.0, 8.0, 152.0, 32.0);
        let gap = Rect::new(100.0, 0.0, 152.0, 24.0);
        let pill = HitMask { rect: Some(gap), notch: None, pill: Some((floating, 16.0)) };
        assert_eq!(pill.bounds(), Some(Rect::new(100.0, 0.0, 152.0, 40.0)), "Pill: from the top edge to the pill's bottom, no wider");

        let open = Rect::new(80.0, 0.0, 660.0, 245.0);
        assert_eq!(HitMask { rect: Some(open), notch: None, pill: None }.bounds(), Some(open));
        assert_eq!(HitMask { rect: None, notch: None, pill: None }.bounds(), None);
    }

    #[test]
    fn capsule_contains_its_shape_only() {
        let body = Rect::new(100.0, 8.0, 152.0, 32.0);
        let m = HitMask { rect: None, notch: None, pill: Some((body, 16.0)) };
        assert!(m.contains(pt(176.0, 24.0)), "centre");
        assert!(m.contains(pt(100.5, 24.0)), "the left end's middle");
        assert!(m.contains(pt(251.5, 24.0)), "the right end's middle");
        assert!(!m.contains(pt(100.5, 8.5)), "the top-left corner is round");
        assert!(!m.contains(pt(251.5, 39.5)), "the bottom-right corner is round");
        assert!(!m.contains(pt(99.5, 24.0)) && !m.contains(pt(252.0, 24.0)), "beside");
        assert!(!m.contains(pt(176.0, 7.5)) && !m.contains(pt(176.0, 40.0)), "above and below");
        // A radius past half the height still makes a capsule, not a lens.
        let big = HitMask { rect: None, notch: None, pill: Some((body, 40.0)) };
        assert!(big.contains(pt(130.0, 9.0)) && !big.contains(pt(101.0, 9.0)));
    }

    #[test]
    fn a_moving_capsule_never_panics_the_hit_test() {
        // Mid-morph values (from the review's float32 spring simulation)
        // where `y + h/2` rounds past `bottom - h/2`: clamp would panic here.
        let body = Rect::new(100.0, 5.903_092_9, 152.0, 38.169_205);
        let m = HitMask { rect: None, notch: None, pill: Some((body, 20.0)) };
        assert!(m.contains(pt(176.0, 40.0)), "the lower half of a capsule in motion");
        for i in 0..400 {
            let (y, h) = (5.0 + i as f32 * 0.0173, 30.0 + i as f32 * 0.0311);
            let b = Rect::new(100.3, y, 151.7, h);
            let m = HitMask { rect: None, notch: Some((b, h / 2.0, 9.0)), pill: Some((b, h / 2.0)) };
            let _ = m.contains(pt(176.0, y + h * 0.75));
        }
    }

    #[test]
    fn notch_contains_body_and_shoulders() {
        let body = Rect::new(100.0, 0.0, 84.0, 30.0);
        let m = HitMask { rect: None, notch: Some((body, 14.0, 9.0)), pill: None };
        assert!(m.contains(pt(142.0, 0.0)) && m.contains(pt(142.0, 29.5)), "the body from the top edge down");
        assert!(m.contains(pt(100.5, 0.5)), "the body's flat top corner");
        assert!(!m.contains(pt(100.5, 29.5)), "the bottom corners are round");
        assert!(m.contains(pt(99.0, 0.5)) && m.contains(pt(185.0, 0.5)), "a shoulder next to the body at the edge");
        assert!(!m.contains(pt(99.0, 8.5)), "the shoulder thins out down the side");
        assert!(!m.contains(pt(91.5, 0.2)), "and out along the edge");
        assert!(!m.contains(pt(90.0, 0.0)) && !m.contains(pt(194.5, 0.0)), "past the shoulders");
        assert!(!m.contains(pt(142.0, 30.0)), "below");
        // A body shorter than the shoulder (the Hidden style's strip): the shoulders shrink with it.
        let strip = Rect::new(100.0, 0.0, 150.0, 5.0);
        let h = HitMask { rect: None, notch: Some((strip, 2.5, 9.0)), pill: None };
        assert!(h.contains(pt(99.0, 0.5)) && !h.contains(pt(99.0, 5.5)));
    }
}
