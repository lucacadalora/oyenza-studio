//! Shelf tiles: a picture's own thumbnail (ImageIO, on a worker thread), else
//! the file's or folder's Finder icon (AppKit, on the main thread). Pictures
//! fill their tile; icons sit inside it with some air, as on Windows.

use super::cf::{self, CFTypeRef};
use crate::events::{AppEvent, EventSink};
use crate::gfx::{Bitmap, Image, ImageData};
use objc2::rc::Retained;
use objc2_app_kit::NSWorkspace;
use objc2_core_foundation::{CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType, CGFloat, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGContext, CGImage};
use objc2_foundation::{NSString, NSURL};
use std::ptr::NonNull;

/// Icons fill this share of their tile.
const ICON_FRACTION: f32 = 0.72;
/// Pictures ImageIO can thumbnail (by extension; anything else gets its icon).
const PICTURES: [&str; 11] = ["png", "jpg", "jpeg", "gif", "heic", "heif", "tif", "tiff", "bmp", "webp", "avif"];

#[link(name = "ImageIO", kind = "framework")]
#[allow(non_upper_case_globals)]
extern "C" {
    fn CGImageSourceCreateWithURL(url: CFTypeRef, options: CFTypeRef) -> CFTypeRef;
    fn CGImageSourceCreateThumbnailAtIndex(src: CFTypeRef, index: usize, options: CFTypeRef) -> CFTypeRef;
    static kCGImageSourceCreateThumbnailFromImageAlways: CFTypeRef;
    static kCGImageSourceThumbnailMaxPixelSize: CFTypeRef;
    static kCGImageSourceCreateThumbnailWithTransform: CFTypeRef;
}

fn is_picture(path: &str) -> bool {
    let ext = std::path::Path::new(path).extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase());
    ext.is_some_and(|e| PICTURES.contains(&e.as_str()))
}

/// Make the tile for `path` (`px` square) and send it as `AppEvent::ShelfThumb`.
pub fn load_async(path: String, px: u32, sink: EventSink) {
    if is_picture(&path) {
        std::thread::spawn(move || match picture(&path, px) {
            Some(img) => sink.send(AppEvent::ShelfThumb { path, image: Some(img) }),
            None => main_icon(path, px, sink),
        });
    } else {
        main_icon(path, px, sink);
    }
}

fn main_icon(path: String, px: u32, sink: EventSink) {
    dispatch2::DispatchQueue::main().exec_async(move || {
        let image = icon(&path, px);
        sink.send(AppEvent::ShelfThumb { path, image });
    });
}

/// The tile right now, on the calling thread (main thread for icons).
pub fn make(path: &str, px: u32) -> Option<Image> {
    if is_picture(path) {
        if let Some(img) = picture(path, px) {
            return Some(img);
        }
    }
    icon(path, px)
}

/// A picture's thumbnail, cropped to fill the tile.
fn picture(path: &str, px: u32) -> Option<Image> {
    let url = NSURL::fileURLWithPath(&NSString::from_str(path));
    unsafe {
        let src = CGImageSourceCreateWithURL(Retained::as_ptr(&url) as CFTypeRef, std::ptr::null());
        if src.is_null() {
            return None;
        }
        let key = |k: CFTypeRef| &*(k as *const CFString);
        let keys: [&CFString; 3] =
            [key(kCGImageSourceCreateThumbnailFromImageAlways), key(kCGImageSourceThumbnailMaxPixelSize), key(kCGImageSourceCreateThumbnailWithTransform)];
        // Cropping to a square needs the short side at `px`, so ask for more.
        let max = CFNumber::new_i32((px * 3).min(1024) as i32);
        let yes = CFBoolean::new(true);
        let values: [&CFType; 3] = [yes.as_ref(), max.as_ref(), yes.as_ref()];
        let opts = CFDictionary::<CFString, CFType>::from_slices(&keys, &values);
        let thumb = CGImageSourceCreateThumbnailAtIndex(src, 0, opts.as_opaque() as *const _ as CFTypeRef);
        cf::CFRelease(src);
        let cg: CFRetained<CGImage> = CFRetained::from_raw(NonNull::new(thumb as *mut CGImage)?);
        let (w, h) = (CGImage::width(Some(&cg)) as f32, CGImage::height(Some(&cg)) as f32);
        if w < 1.0 || h < 1.0 {
            return None;
        }
        let s = (px as f32 / w).max(px as f32 / h);
        let (dw, dh) = (w * s, h * s);
        render(px, &cg, (px as f32 - dw) / 2.0, (px as f32 - dh) / 2.0, dw, dh)
    }
}

/// The Finder icon, fitted inside the tile.
fn icon(path: &str, px: u32) -> Option<Image> {
    let img = NSWorkspace::sharedWorkspace().iconForFile(&NSString::from_str(path));
    let side = ((px as f32 * ICON_FRACTION).round()).max(16.0);
    let mut rect = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: side as CGFloat, height: side as CGFloat } };
    let cg = unsafe { img.CGImageForProposedRect_context_hints(&mut rect, None, None) }?;
    let off = (px as f32 - side) / 2.0;
    render(px, &cg, off, off, side, side)
}

/// Draw `cg` into a transparent `px` square at (x, y, w, h), top-left origin.
fn render(px: u32, cg: &CGImage, x: f32, y: f32, w: f32, h: f32) -> Option<Image> {
    let bmp = Bitmap::new(px as usize, px as usize, 1.0)?;
    let ctx: &CGContext = bmp.ctx();
    CGContext::save_g_state(Some(ctx));
    CGContext::set_interpolation_quality(Some(ctx), objc2_core_graphics::CGInterpolationQuality::High);
    // Images draw bottom-up; flip them back in this y-down space.
    CGContext::translate_ctm(Some(ctx), x as CGFloat, (y + h) as CGFloat);
    CGContext::scale_ctm(Some(ctx), 1.0, -1.0);
    let r = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: w as CGFloat, height: h as CGFloat } };
    CGContext::draw_image(Some(ctx), r, Some(cg));
    CGContext::restore_g_state(Some(ctx));
    Some(ImageData::new(px, px, bmp.pixels()))
}
