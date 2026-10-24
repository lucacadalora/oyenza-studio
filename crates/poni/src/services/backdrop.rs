//! The Glass theme's view of the screen: a copy of the patch under the
//! island's window, for the glass to bend, frost and tint.
//!
//! Runs only while the Glass theme is on. A thread of its own owns a Direct3D
//! device on the island monitor's adapter and duplicates that output (DXGI
//! Desktop Duplication). The island window is left out of screen capture
//! (`window::set_capture_excluded`), so the copy shows what's under it, never
//! the island itself. Windows hands over a frame only when the desktop
//! changed; the thread copies the window's rectangle and, when those pixels
//! really differ from the last copy, keeps them and wakes the UI thread.
//!
//! The copy stays in memory and is overwritten each time: never saved, never
//! logged, never sent. Anything unexpected (another GPU drives the display,
//! a rotated or HDR output, Remote Desktop) stops the copy and the island
//! draws smoked glass instead; the log says why.

use crate::events::{AppEvent, EventSink};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use windows::core::{Interface, Result};
use windows::Win32::Foundation::{HMODULE, RECT};
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::HMONITOR;

/// One copy of the window's rectangle: BGRA, top-down, `w * 4` bytes a row,
/// taken with the window's top-left at (`left`, `top`) screen pixels.
pub struct Frame {
    pub w: i32,
    pub h: i32,
    pub px: Vec<u8>,
    pub left: i32,
    pub top: i32,
    pub generation: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Starting,
    Running,
    /// Gave up for good (the island draws smoked glass); why, for the log.
    Failed(String),
}

struct Shared {
    /// The island window's rectangle in screen pixels.
    rect: Mutex<RECT>,
    /// Bumped when the rectangle moves: the thread starts a fresh duplication,
    /// which hands over the whole screen at once.
    rect_gen: AtomicU64,
    latest: Mutex<Option<Arc<Frame>>>,
    generation: AtomicU64,
    status: Mutex<Status>,
    stop: AtomicBool,
}

pub struct Backdrop {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    taken: u64,
}

impl Backdrop {
    /// Start copying `rect` (screen pixels) on `monitor`.
    pub fn start(sink: EventSink, monitor: isize, rect: RECT) -> Self {
        let shared = Arc::new(Shared {
            rect: Mutex::new(rect),
            rect_gen: AtomicU64::new(0),
            latest: Mutex::new(None),
            generation: AtomicU64::new(0),
            status: Mutex::new(Status::Starting),
            stop: AtomicBool::new(false),
        });
        let s = shared.clone();
        let thread = std::thread::Builder::new()
            .name("poni-backdrop".into())
            .spawn(move || {
                let status = match run(&s, &sink, monitor) {
                    Ok(()) => return,
                    Err(why) => why,
                };
                crate::pinfo!("glass: screen copy stopped ({status}); smoked glass instead");
                *s.status.lock().unwrap_or_else(|p| p.into_inner()) = Status::Failed(status);
                sink.send(AppEvent::Backdrop);
            })
            .ok();
        if thread.is_none() {
            *shared.status.lock().unwrap_or_else(|p| p.into_inner()) = Status::Failed("no thread".into());
        }
        Self { shared, thread, taken: 0 }
    }

    /// The island window moved or changed size.
    pub fn set_rect(&self, rect: RECT) {
        let mut r = self.shared.rect.lock().unwrap_or_else(|p| p.into_inner());
        if *r != rect {
            *r = rect;
            self.shared.rect_gen.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn status(&self) -> Status {
        self.shared.status.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// The newest copy, once each.
    pub fn take_new(&mut self) -> Option<Arc<Frame>> {
        if self.shared.generation.load(Ordering::Acquire) == self.taken {
            return None;
        }
        let f = self.shared.latest.lock().unwrap_or_else(|p| p.into_inner()).clone()?;
        self.taken = f.generation;
        Some(f)
    }
}

impl Drop for Backdrop {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            // The thread wakes at least every 250 ms (frame wait timeout).
            let _ = t.join();
        }
    }
}

/// The output showing `monitor`, and its adapter.
unsafe fn find_output(monitor: isize) -> std::result::Result<(IDXGIAdapter1, IDXGIOutput, DXGI_OUTPUT_DESC), String> {
    let factory: IDXGIFactory1 = CreateDXGIFactory1().map_err(|e| format!("no DXGI factory: {e}"))?;
    let mut a = 0;
    while let Ok(adapter) = factory.EnumAdapters1(a) {
        let mut o = 0;
        while let Ok(output) = adapter.EnumOutputs(o) {
            if let Ok(desc) = output.GetDesc() {
                if desc.Monitor == HMONITOR(monitor as *mut core::ffi::c_void) {
                    return Ok((adapter, output, desc));
                }
            }
            o += 1;
        }
        a += 1;
    }
    Err("the island's monitor isn't on any adapter's outputs".into())
}

unsafe fn device_on(adapter: &IDXGIAdapter1) -> Result<(ID3D11Device, ID3D11DeviceContext)> {
    let levels = [D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_9_3];
    let (mut dev, mut ctx) = (None, None);
    D3D11CreateDevice(adapter, D3D_DRIVER_TYPE_UNKNOWN, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, Some(&levels), D3D11_SDK_VERSION, Some(&mut dev), None, Some(&mut ctx))?;
    match (dev, ctx) {
        (Some(d), Some(c)) => Ok((d, c)),
        _ => Err(windows::Win32::Foundation::E_FAIL.into()),
    }
}

/// Duplicate `output`, asking for 8-bit BGRA (what the glass samples).
unsafe fn duplicate(output: &IDXGIOutput, dev: &ID3D11Device) -> std::result::Result<IDXGIOutputDuplication, String> {
    let dup = match output.cast::<IDXGIOutput5>() {
        Ok(o5) => o5.DuplicateOutput1(dev, 0, &[DXGI_FORMAT_B8G8R8A8_UNORM]),
        Err(_) => output.cast::<IDXGIOutput1>().map_err(|e| format!("no IDXGIOutput1: {e}"))?.DuplicateOutput(dev),
    };
    dup.map_err(|e| {
        if e.code() == DXGI_ERROR_UNSUPPORTED {
            format!("Windows can't copy this display from the adapter that drives it ({e})")
        } else if e.code() == DXGI_ERROR_NOT_CURRENTLY_AVAILABLE {
            format!("too many screen copies running ({e})")
        } else {
            format!("screen copy refused ({e})")
        }
    })
}

/// `r` minus `origin`, clipped to `w`×`h`.
fn local(r: RECT, origin: (i32, i32), w: i32, h: i32) -> RECT {
    RECT {
        left: (r.left - origin.0).clamp(0, w),
        top: (r.top - origin.1).clamp(0, h),
        right: (r.right - origin.0).clamp(0, w),
        bottom: (r.bottom - origin.1).clamp(0, h),
    }
}

fn overlaps(a: &RECT, b: &RECT) -> bool {
    a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom
}

/// Whether any dirty or moved rectangle of the current frame touches `area`.
unsafe fn frame_touches(dup: &IDXGIOutputDuplication, meta: u32, area: &RECT, buf: &mut Vec<u8>) -> bool {
    if meta == 0 {
        return true;
    }
    buf.resize(meta as usize, 0);
    let mut used = 0u32;
    if dup.GetFrameMoveRects(meta, buf.as_mut_ptr() as *mut DXGI_OUTDUPL_MOVE_RECT, &mut used).is_err() {
        return true;
    }
    let moves = std::slice::from_raw_parts(buf.as_ptr() as *const DXGI_OUTDUPL_MOVE_RECT, used as usize / std::mem::size_of::<DXGI_OUTDUPL_MOVE_RECT>());
    if moves.iter().any(|m| overlaps(&m.DestinationRect, area)) {
        return true;
    }
    let mut used = 0u32;
    if dup.GetFrameDirtyRects(meta, buf.as_mut_ptr() as *mut RECT, &mut used).is_err() {
        return true;
    }
    let dirty = std::slice::from_raw_parts(buf.as_ptr() as *const RECT, used as usize / std::mem::size_of::<RECT>());
    dirty.iter().any(|d| overlaps(d, area))
}

/// Copy loop. Ok(()) when asked to stop; Err(why) when it can't go on.
fn run(s: &Shared, sink: &EventSink, monitor: isize) -> std::result::Result<(), String> {
    unsafe {
        let (adapter, output, odesc) = find_output(monitor)?;
        if !matches!(odesc.Rotation, DXGI_MODE_ROTATION_IDENTITY | DXGI_MODE_ROTATION_UNSPECIFIED) {
            return Err("the display is rotated".into());
        }
        let (dev, ctx) = device_on(&adapter).map_err(|e| format!("no Direct3D device on the display's adapter: {e}"))?;
        let origin = (odesc.DesktopCoordinates.left, odesc.DesktopCoordinates.top);
        let name = String::from_utf16_lossy(&odesc.DeviceName).trim_end_matches('\0').to_string();
        let mut staging: Option<(ID3D11Texture2D, i32, i32)> = None;
        let mut meta_buf = Vec::new();
        let mut last: Option<Arc<Frame>> = None;
        let mut announced = false;
        let mut lost_in_a_row = 0;
        'dup: loop {
            if s.stop.load(Ordering::Relaxed) {
                return Ok(());
            }
            let rect_gen = s.rect_gen.load(Ordering::Relaxed);
            let dup = match duplicate(&output, &dev) {
                Ok(d) => d,
                Err(why) => {
                    // Briefly refused while the secure desktop (UAC, lock
                    // screen) is up or a mode change runs: wait, then retry.
                    lost_in_a_row += 1;
                    if lost_in_a_row > 40 {
                        return Err(why);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(250));
                    continue;
                }
            };
            let ddesc = dup.GetDesc();
            let (ow, oh) = (ddesc.ModeDesc.Width as i32, ddesc.ModeDesc.Height as i32);
            let mut fresh = true; // a new duplication starts with the whole screen
            loop {
                if s.stop.load(Ordering::Relaxed) {
                    return Ok(());
                }
                if s.rect_gen.load(Ordering::Relaxed) != rect_gen {
                    continue 'dup;
                }
                let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
                let mut res: Option<IDXGIResource> = None;
                match dup.AcquireNextFrame(250, &mut info, &mut res) {
                    Ok(()) => {}
                    Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => continue,
                    Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST || e.code() == DXGI_ERROR_INVALID_CALL => continue 'dup,
                    Err(e) => return Err(format!("frame wait failed: {e}")),
                }
                lost_in_a_row = 0;
                let rect = *s.rect.lock().unwrap_or_else(|p| p.into_inner());
                let area = local(rect, origin, ow, oh);
                let (w, h) = (rect.right - rect.left, rect.bottom - rect.top);
                let wanted = w > 0 && h > 0 && area.right > area.left && area.bottom > area.top
                    && info.LastPresentTime != 0
                    && (fresh || frame_touches(&dup, info.TotalMetadataBufferSize, &area, &mut meta_buf));
                let mut copied = None;
                if wanted {
                    if let Some(tex) = res.as_ref().and_then(|r| r.cast::<ID3D11Texture2D>().ok()) {
                        let mut tdesc = D3D11_TEXTURE2D_DESC::default();
                        tex.GetDesc(&mut tdesc);
                        if tdesc.Format != DXGI_FORMAT_B8G8R8A8_UNORM {
                            let _ = dup.ReleaseFrame();
                            return Err(format!("the display sends {:?} pixels (HDR?), not 8-bit", tdesc.Format));
                        }
                        match copy_area(&dev, &ctx, &tex, &mut staging, area, rect, origin, w, h) {
                            Ok(px) => copied = Some(px),
                            Err(e) => crate::pwarn!("glass: copying the screen failed ({e})"),
                        }
                    }
                }
                let _ = dup.ReleaseFrame();
                if let Some(px) = copied {
                    fresh = false;
                    // Dirty rectangles also cover redraws that changed nothing.
                    let same = last.as_ref().is_some_and(|l| l.w == w && l.h == h && l.px == px);
                    if !same {
                        let generation = s.generation.load(Ordering::Relaxed) + 1;
                        let f = Arc::new(Frame { w, h, px, left: rect.left, top: rect.top, generation });
                        *s.latest.lock().unwrap_or_else(|p| p.into_inner()) = Some(f.clone());
                        s.generation.store(generation, Ordering::Release);
                        last = Some(f);
                        if !announced {
                            announced = true;
                            *s.status.lock().unwrap_or_else(|p| p.into_inner()) = Status::Running;
                            crate::pinfo!("glass: screen copy on ({name}, {ow}x{oh}, window {w}x{h} px)");
                        }
                        sink.send(AppEvent::Backdrop);
                    }
                }
            }
        }
    }
}

/// Copy the window's rectangle out of the desktop image into memory. (The
/// window sits on this display; a part off it would keep an older copy.)
#[allow(clippy::too_many_arguments)]
unsafe fn copy_area(
    dev: &ID3D11Device,
    ctx: &ID3D11DeviceContext,
    desktop: &ID3D11Texture2D,
    staging: &mut Option<(ID3D11Texture2D, i32, i32)>,
    area: RECT,
    rect: RECT,
    origin: (i32, i32),
    w: i32,
    h: i32,
) -> Result<Vec<u8>> {
    if !staging.as_ref().is_some_and(|s| s.1 == w && s.2 == h) {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w as u32,
            Height: h as u32,
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
        dev.CreateTexture2D(&desc, None, Some(&mut tex))?;
        *staging = tex.map(|t| (t, w, h));
    }
    let Some((st, _, _)) = staging.as_ref() else { return Err(windows::Win32::Foundation::E_FAIL.into()) };
    // Where the visible part of the window lands inside the window.
    let (dx, dy) = ((area.left + origin.0 - rect.left) as u32, (area.top + origin.1 - rect.top) as u32);
    let src = D3D11_BOX { left: area.left as u32, top: area.top as u32, front: 0, right: area.right as u32, bottom: area.bottom as u32, back: 1 };
    ctx.CopySubresourceRegion(st, 0, dx, dy, 0, desktop, 0, Some(&src));
    let mut m = D3D11_MAPPED_SUBRESOURCE::default();
    ctx.Map(st, 0, D3D11_MAP_READ, 0, Some(&mut m))?;
    let row = w as usize * 4;
    let mut out = vec![0u8; row * h as usize];
    for y in 0..h as usize {
        let src = std::slice::from_raw_parts((m.pData as *const u8).add(y * m.RowPitch as usize), row);
        out[y * row..(y + 1) * row].copy_from_slice(src);
    }
    // The desktop's alpha means nothing; the frosting treats the copy as opaque.
    for a in out.iter_mut().skip(3).step_by(4) {
        *a = 255;
    }
    ctx.Unmap(st, 0);
    Ok(out)
}
