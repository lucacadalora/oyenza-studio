//! Rendering on the Mac: Core Graphics for shapes and pictures, Core Text for
//! words, drawn into the island view's layer (or into an offscreen bitmap for
//! `--render` and the menu bar icon).
//!
//! The drawing API ([`Canvas`]) is the Windows renderer's (`../gfx.rs`), call
//! for call, so pages, activities and Oyen draw unchanged. Coordinates are
//! DIPs (points): the context's base transform puts (0, 0) at the top left
//! with y growing down, and Core Graphics does the Retina scaling.
//!
//! Differences that are on purpose:
//! * Colours are sRGB, like the Windows island, whatever the display's own space.
//! * Text uses the system font (SF Pro, with monospaced digits for `tabular`)
//!   unless the look brings its own (Outfit for Oyen, embedded in the app).
//! * [`Canvas::layered`] is a Core Graphics transparency layer: the group still
//!   fades as one picture, and its text is drawn live through the transform
//!   (macOS doesn't hint glyphs, so scaled text glides without re-snapping).

use crate::icons::{self, Icon};
use objc2::rc::Retained;
use objc2_app_kit::{NSFont, NSFontWeightBold, NSFontWeightMedium, NSFontWeightRegular, NSFontWeightSemibold};
use objc2_core_foundation::{
    CFAttributedString, CFBoolean, CFData, CFDictionary, CFNumber, CFRange, CFRetained, CFString, CFType, CGAffineTransform, CGFloat, CGPoint, CGRect,
    CGSize,
};
use objc2_core_graphics::{
    kCGColorSpaceSRGB, CGBitmapContextCreate, CGBitmapContextCreateImage, CGBitmapContextGetData, CGBitmapInfo, CGColorRenderingIntent, CGColorSpace,
    CGContext, CGDataProvider, CGGradient, CGGradientDrawingOptions, CGImage, CGImageAlphaInfo, CGImageByteOrderInfo, CGInterpolationQuality, CGLineCap,
    CGLineJoin, CGMutablePath, CGPath,
};
use objc2_core_text::{
    kCTFontAttributeName, kCTForegroundColorFromContextAttributeName, CTFont, CTFontDescriptor, CTFontManagerCreateFontDescriptorsFromData, CTLine,
    CTLineTruncationType, CTTypesetter,
};
use poni_core::geom::{Color, Point, Rect};
use poni_core::look::{self, Look};
use poni_core::svgpath::Seg;
use std::collections::HashMap;
use std::ptr::{self, NonNull};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------- images

/// Decoded image: premultiplied BGRA, top-down rows, `width * 4` stride
/// (the same bytes the Windows app uses, and what Core Graphics calls
/// premultiplied-first, 32-bit little-endian).
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

// ---------------------------------------------------------------- text styles

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Weight {
    Regular,
    Medium,
    Semibold,
    Bold,
}

impl Weight {
    fn index(self) -> usize {
        match self {
            Weight::Regular => 0,
            Weight::Medium => 1,
            Weight::Semibold => 2,
            Weight::Bold => 3,
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

#[allow(dead_code)]
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

// ---------------------------------------------------------------- transforms

/// A 2D affine transform in Direct2D's layout (row vectors: `a * b` applies
/// `a` first, then `b`), so the island code reads the same on both systems.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Matrix3x2 {
    pub m11: f32,
    pub m12: f32,
    pub m21: f32,
    pub m22: f32,
    pub m31: f32,
    pub m32: f32,
}

#[allow(dead_code)]
impl Matrix3x2 {
    pub const fn identity() -> Self {
        Self { m11: 1.0, m12: 0.0, m21: 0.0, m22: 1.0, m31: 0.0, m32: 0.0 }
    }

    pub const fn translation(x: f32, y: f32) -> Self {
        Self { m11: 1.0, m12: 0.0, m21: 0.0, m22: 1.0, m31: x, m32: y }
    }

    fn to_cg(self) -> CGAffineTransform {
        CGAffineTransform { a: self.m11 as CGFloat, b: self.m12 as CGFloat, c: self.m21 as CGFloat, d: self.m22 as CGFloat, tx: self.m31 as CGFloat, ty: self.m32 as CGFloat }
    }

    fn from_cg(t: CGAffineTransform) -> Self {
        Self { m11: t.a as f32, m12: t.b as f32, m21: t.c as f32, m22: t.d as f32, m31: t.tx as f32, m32: t.ty as f32 }
    }

    fn inverse(self) -> Self {
        let det = self.m11 * self.m22 - self.m12 * self.m21;
        if det.abs() < 1e-12 {
            return Self::identity();
        }
        let i = 1.0 / det;
        let m11 = self.m22 * i;
        let m12 = -self.m12 * i;
        let m21 = -self.m21 * i;
        let m22 = self.m11 * i;
        Self { m11, m12, m21, m22, m31: -(self.m31 * m11 + self.m32 * m21), m32: -(self.m31 * m12 + self.m32 * m22) }
    }
}

impl std::ops::Mul for Matrix3x2 {
    type Output = Matrix3x2;
    fn mul(self, b: Matrix3x2) -> Matrix3x2 {
        let a = self;
        Matrix3x2 {
            m11: a.m11 * b.m11 + a.m12 * b.m21,
            m12: a.m11 * b.m12 + a.m12 * b.m22,
            m21: a.m21 * b.m11 + a.m22 * b.m21,
            m22: a.m21 * b.m12 + a.m22 * b.m22,
            m31: a.m31 * b.m11 + a.m32 * b.m21 + b.m31,
            m32: a.m31 * b.m12 + a.m32 * b.m22 + b.m32,
        }
    }
}

/// Scale by `s` about `p`.
pub fn scale_about(p: Point, s: f32) -> Matrix3x2 {
    Matrix3x2 { m11: s, m12: 0.0, m21: 0.0, m22: s, m31: p.x * (1.0 - s), m32: p.y * (1.0 - s) }
}

pub fn union_rect(a: Rect, b: Rect) -> Rect {
    let (x0, y0) = (a.x.min(b.x), a.y.min(b.y));
    Rect::new(x0, y0, a.right().max(b.right()) - x0, a.bottom().max(b.bottom()) - y0)
}

fn cgrect(r: Rect) -> CGRect {
    CGRect { origin: CGPoint { x: r.x as CGFloat, y: r.y as CGFloat }, size: CGSize { width: r.w.max(0.0) as CGFloat, height: r.h.max(0.0) as CGFloat } }
}

// ---------------------------------------------------------------- fonts and text layout

/// Fonts shipped inside the app: Outfit (SIL Open Font License 1.1, see
/// `res/fonts/OFL.txt`), the Oyenza Studio website's typeface, in the island's
/// four weights. Letters Outfit lacks (Japanese, Korean, emoji, ...) come
/// from the system's fonts through Core Text's fallback.
const EMBEDDED_FONTS: [&[u8]; 4] = [
    include_bytes!("../../res/fonts/Outfit-Regular.ttf"),
    include_bytes!("../../res/fonts/Outfit-Medium.ttf"),
    include_bytes!("../../res/fonts/Outfit-SemiBold.ttf"),
    include_bytes!("../../res/fonts/Outfit-Bold.ttf"),
];

/// `kNumberSpacingType` / `kMonospacedNumbersSelector` (SFNTLayoutTypes.h).
const NUMBER_SPACING: i32 = 6;
const MONOSPACED_NUMBERS: i32 = 0;

#[derive(Clone, PartialEq, Eq, Hash)]
struct FontKey {
    size: u32,
    weight: Weight,
    tabular: bool,
    embedded: bool,
}

/// Text layouts are cached by everything that shapes them (not colour).
#[derive(Clone, PartialEq, Eq, Hash)]
struct LayoutKey {
    text: String,
    font: FontKey,
    wrap: bool,
    line: u32,
    w: u32,
}

struct TextLayout {
    /// Each line and its visible width (trailing spaces excluded).
    lines: Vec<(CFRetained<CTLine>, f32)>,
    ascent: f32,
    leading: f32,
    line_h: f32,
    /// Widest line including trailing spaces (what `measure` reports).
    width: f32,
}

// ---------------------------------------------------------------- Gfx

pub struct Gfx {
    scale: f32,
    look: &'static Look,
    srgb: CFRetained<CGColorSpace>,
    fonts: HashMap<FontKey, CFRetained<CTFont>>,
    embedded: Option<Vec<CFRetained<CTFontDescriptor>>>,
    embedded_tried: bool,
    ellipsis: HashMap<FontKey, CFRetained<CTLine>>,
    layouts: HashMap<LayoutKey, (Rc<TextLayout>, u64)>,
    images: HashMap<u64, (CFRetained<CGImage>, u64)>,
    icon_paths: HashMap<Icon, Vec<(CFRetained<CGMutablePath>, bool)>>,
    frame_no: u64,
}

impl Gfx {
    pub fn new(scale: f32) -> Gfx {
        let srgb = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB })).expect("the sRGB colour space");
        Gfx {
            scale: scale.max(1.0),
            look: &look::CLASSIC,
            srgb,
            fonts: HashMap::new(),
            embedded: None,
            embedded_tried: false,
            ellipsis: HashMap::new(),
            layouts: HashMap::new(),
            images: HashMap::new(),
            icon_paths: HashMap::new(),
            frame_no: 0,
        }
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    pub fn set_scale(&mut self, s: f32) {
        self.scale = s.max(1.0);
    }

    /// How content paints from now on (the theme).
    pub fn set_look(&mut self, look: &'static Look) {
        self.look = look;
    }

    /// Start drawing on `ctx`, whose transform already maps DIPs (top left,
    /// y down) to its pixels.
    pub fn canvas<'a>(&'a mut self, ctx: &'a CGContext) -> Canvas<'a> {
        self.frame_no += 1;
        if self.frame_no % 120 == 0 {
            let keep = self.frame_no.saturating_sub(240);
            self.layouts.retain(|_, v| v.1 >= keep);
            self.images.retain(|_, v| v.1 >= keep);
        }
        CGContext::set_fill_color_space(Some(ctx), Some(&self.srgb));
        CGContext::set_stroke_color_space(Some(ctx), Some(&self.srgb));
        CGContext::set_should_antialias(Some(ctx), true);
        // Font smoothing thickens text drawn on transparent pixels.
        CGContext::set_should_smooth_fonts(Some(ctx), false);
        CGContext::set_interpolation_quality(Some(ctx), CGInterpolationQuality::High);
        CGContext::set_line_cap(Some(ctx), CGLineCap::Round);
        CGContext::set_line_join(Some(ctx), CGLineJoin::Round);
        let base = Matrix3x2::from_cg(CGContext::ctm(Some(ctx)));
        Canvas { g: self, ctx, alpha: 1.0, base, stack: 0 }
    }

    fn embedded(&mut self) -> Option<&Vec<CFRetained<CTFontDescriptor>>> {
        if !self.embedded_tried {
            self.embedded_tried = true;
            let mut out = Vec::new();
            for data in EMBEDDED_FONTS {
                let d = CFData::from_static_bytes(data);
                let arr = unsafe { CTFontManagerCreateFontDescriptorsFromData(&d) };
                let arr: &objc2_core_foundation::CFArray<CTFontDescriptor> = unsafe { arr.cast_unchecked() };
                let found = arr.get(0);
                match found {
                    Some(desc) => out.push(desc),
                    None => {
                        pwarn!("embedded font could not be loaded; using the system font");
                        return None;
                    }
                }
            }
            self.embedded = Some(out);
        }
        self.embedded.as_ref()
    }

    fn font(&mut self, size: f32, weight: Weight, tabular: bool) -> CFRetained<CTFont> {
        let key = FontKey { size: size.to_bits(), weight, tabular, embedded: self.look.font.is_some() };
        if let Some(f) = self.fonts.get(&key) {
            return f.clone();
        }
        let embedded = if key.embedded {
            self.embedded().map(|d| d[weight.index()].clone()).map(|desc| {
                let desc = if tabular {
                    let ty = CFNumber::new_i32(NUMBER_SPACING);
                    let sel = CFNumber::new_i32(MONOSPACED_NUMBERS);
                    unsafe { desc.copy_with_feature(&ty, &sel) }
                } else {
                    desc
                };
                unsafe { CTFont::with_font_descriptor(&desc, size as CGFloat, ptr::null()) }
            })
        } else {
            None
        };
        let f = embedded.unwrap_or_else(|| system_font(size, weight, tabular));
        self.fonts.insert(key, f.clone());
        f
    }

    fn attributed(font: &CTFont, s: &str) -> Option<CFRetained<CFAttributedString>> {
        let keys: [&CFString; 2] = unsafe { [kCTFontAttributeName, kCTForegroundColorFromContextAttributeName] };
        let values: [&CFType; 2] = [font.as_ref(), CFBoolean::new(true).as_ref()];
        let attrs = CFDictionary::<CFString, CFType>::from_slices(&keys, &values);
        let text = CFString::from_str(s);
        unsafe { CFAttributedString::new(None, Some(&text), Some(attrs.as_opaque())) }
    }

    fn ellipsis_for(&mut self, key: &FontKey, font: &CTFont) -> Option<CFRetained<CTLine>> {
        if let Some(l) = self.ellipsis.get(key) {
            return Some(l.clone());
        }
        let attr = Self::attributed(font, "\u{2026}")?;
        let line = unsafe { CTLine::with_attributed_string(&attr) };
        self.ellipsis.insert(key.clone(), line.clone());
        Some(line)
    }

    /// A text layout, reused across frames: at 120 fps shaping every string
    /// every frame would be most of the frame.
    fn layout(&mut self, s: &str, st: &TextStyle, w: f32) -> Rc<TextLayout> {
        let w = w.max(1.0);
        let fkey = FontKey { size: st.size.to_bits(), weight: st.weight, tabular: st.tabular, embedded: self.look.font.is_some() };
        let key = LayoutKey { text: s.to_string(), font: fkey.clone(), wrap: st.wrap, line: st.line.to_bits(), w: w.to_bits() };
        let frame = self.frame_no;
        if let Some((l, used)) = self.layouts.get_mut(&key) {
            *used = frame;
            return l.clone();
        }
        let font = self.font(st.size, st.weight, st.tabular);
        let (ascent, descent, leading) = unsafe { (font.ascent() as f32, font.descent() as f32, font.leading() as f32) };
        let line_h = if st.line > 0.0 { st.line * st.size } else { ascent + descent + leading };
        let mut lines = Vec::new();
        let mut width = 0.0f32;
        if let Some(attr) = Self::attributed(&font, s) {
            if !st.wrap {
                let line = unsafe { CTLine::with_attributed_string(&attr) };
                let full = unsafe { line.typographic_bounds(ptr::null_mut(), ptr::null_mut(), ptr::null_mut()) } as f32;
                let trailing = unsafe { line.trailing_whitespace_width() } as f32;
                if full > w + 0.5 {
                    let token = self.ellipsis_for(&fkey, &font);
                    let cut = unsafe { line.truncated_line(w as f64, CTLineTruncationType::End, token.as_deref()) };
                    let line = cut.unwrap_or(line);
                    let lw = unsafe { line.typographic_bounds(ptr::null_mut(), ptr::null_mut(), ptr::null_mut()) } as f32;
                    width = lw.min(w);
                    lines.push((line, width));
                } else {
                    width = full;
                    lines.push((line, full - trailing));
                }
            } else {
                let ts = unsafe { CTTypesetter::with_attributed_string(&attr) };
                let len = s.encode_utf16().count() as isize;
                let mut start = 0isize;
                while start < len {
                    let count = unsafe { ts.suggest_line_break(start, w as f64) };
                    if count <= 0 {
                        break;
                    }
                    let line = unsafe { ts.line(CFRange { location: start, length: count }) };
                    let full = unsafe { line.typographic_bounds(ptr::null_mut(), ptr::null_mut(), ptr::null_mut()) } as f32;
                    let trailing = unsafe { line.trailing_whitespace_width() } as f32;
                    width = width.max(full);
                    lines.push((line, full - trailing));
                    start += count;
                }
            }
        }
        let layout = Rc::new(TextLayout { lines, ascent, leading, line_h, width });
        self.layouts.insert(key, (layout.clone(), frame));
        layout
    }

    fn cg_image(&mut self, img: &Image) -> Option<CFRetained<CGImage>> {
        let frame = self.frame_no;
        if let Some((i, used)) = self.images.get_mut(&img.id) {
            *used = frame;
            return Some(i.clone());
        }
        let data = CFData::from_bytes(&img.pixels);
        let provider = CGDataProvider::with_cf_data(Some(&data))?;
        let info = CGBitmapInfo(CGImageAlphaInfo::PremultipliedFirst.0 | CGImageByteOrderInfo::Order32Little.0);
        let image = unsafe {
            CGImage::new(
                img.width as usize,
                img.height as usize,
                8,
                32,
                img.width as usize * 4,
                Some(&self.srgb),
                info,
                Some(&provider),
                ptr::null(),
                true,
                CGColorRenderingIntent::RenderingIntentDefault,
            )
        }?;
        self.images.insert(img.id, (image.clone(), frame));
        Some(image)
    }

    fn icon_paths(&mut self, icon: Icon) -> Vec<(CFRetained<CGMutablePath>, bool)> {
        if let Some(p) = self.icon_paths.get(&icon) {
            return p.clone();
        }
        let mut out = Vec::new();
        for part in icons::parts(icon) {
            match poni_core::svgpath::parse(part.d) {
                Ok(segs) => out.push((cg_path(&segs), part.fill)),
                Err(e) => pwarn!("icon {icon:?} path parse failed: {}", e.0),
            }
        }
        self.icon_paths.insert(icon, out.clone());
        out
    }
}

/// The system font (SF Pro) at `size`, with fixed-width digits for `tabular`.
fn system_font(size: f32, weight: Weight, tabular: bool) -> CFRetained<CTFont> {
    let w = unsafe {
        match weight {
            Weight::Regular => NSFontWeightRegular,
            Weight::Medium => NSFontWeightMedium,
            Weight::Semibold => NSFontWeightSemibold,
            Weight::Bold => NSFontWeightBold,
        }
    };
    let f: Retained<NSFont> = if tabular {
        NSFont::monospacedDigitSystemFontOfSize_weight(size as CGFloat, w)
    } else {
        NSFont::systemFontOfSize_weight(size as CGFloat, w)
    };
    // NSFont and CTFont are the same object ("toll-free bridged").
    unsafe { CFRetained::retain(NonNull::new_unchecked(Retained::as_ptr(&f) as *mut CTFont)) }
}

/// Build a Core Graphics path from parsed SVG segments. Arcs are flattened to
/// Béziers the same way the Windows renderer does it.
pub fn cg_path(segs: &[Seg]) -> CFRetained<CGMutablePath> {
    let p = CGMutablePath::new();
    let mut open = false;
    let mut cur = (0.0f32, 0.0f32);
    let f = |v: f32| v as CGFloat;
    unsafe {
        for s in segs {
            match *s {
                Seg::MoveTo(x, y) => {
                    CGMutablePath::move_to_point(Some(&p), ptr::null(), f(x), f(y));
                    open = true;
                    cur = (x, y);
                }
                Seg::LineTo(x, y) => {
                    if !open {
                        CGMutablePath::move_to_point(Some(&p), ptr::null(), f(cur.0), f(cur.1));
                        open = true;
                    }
                    CGMutablePath::add_line_to_point(Some(&p), ptr::null(), f(x), f(y));
                    cur = (x, y);
                }
                Seg::CubicTo(x1, y1, x2, y2, x, y) => {
                    if !open {
                        CGMutablePath::move_to_point(Some(&p), ptr::null(), f(cur.0), f(cur.1));
                        open = true;
                    }
                    CGMutablePath::add_curve_to_point(Some(&p), ptr::null(), f(x1), f(y1), f(x2), f(y2), f(x), f(y));
                    cur = (x, y);
                }
                Seg::QuadTo(x1, y1, x, y) => {
                    if !open {
                        CGMutablePath::move_to_point(Some(&p), ptr::null(), f(cur.0), f(cur.1));
                        open = true;
                    }
                    CGMutablePath::add_quad_curve_to_point(Some(&p), ptr::null(), f(x1), f(y1), f(x), f(y));
                    cur = (x, y);
                }
                Seg::ArcTo(rx, ry, rot, large, sweep, x, y) => {
                    if !open {
                        CGMutablePath::move_to_point(Some(&p), ptr::null(), f(cur.0), f(cur.1));
                        open = true;
                    }
                    for c in poni_core::svgpath::arc_to_cubics(cur.0, cur.1, rx, ry, rot, large, sweep, x, y) {
                        CGMutablePath::add_curve_to_point(Some(&p), ptr::null(), f(c[0]), f(c[1]), f(c[2]), f(c[3]), f(c[4]), f(c[5]));
                    }
                    cur = (x, y);
                }
                Seg::Close => {
                    if open {
                        CGMutablePath::close_subpath(Some(&p));
                        open = false;
                    }
                }
            }
        }
    }
    p
}

/// The island outline as path segments (flat top, concave shoulders, rounded
/// bottom), the same shape the Windows renderer draws.
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

// ---------------------------------------------------------------- Canvas

/// Drawing API used by pages and activities. Coordinates are DIPs.
pub struct Canvas<'a> {
    g: &'a mut Gfx,
    ctx: &'a CGContext,
    /// Global opacity multiplier applied to everything drawn.
    alpha: f32,
    /// The context's transform when the canvas began (DIPs to pixels).
    base: Matrix3x2,
    /// Clips and shape masks pushed and not yet popped (saved graphics states).
    stack: usize,
}

#[allow(dead_code)]
impl<'a> Canvas<'a> {
    /// Same answer as the Windows GPU renderer: hit testing is the app's job,
    /// not the pixels'.
    pub fn on_gpu(&self) -> bool {
        true
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

    /// The Windows canvas snaps text to whole pixels unless told not to
    /// (moving text). Core Text places glyphs where they are asked, so there
    /// is nothing to switch here. Returns the previous setting.
    pub fn set_text_snap(&mut self, _on: bool) -> bool {
        true
    }

    fn cg(&self) -> Option<&CGContext> {
        Some(self.ctx)
    }

    fn components(&self, c: Color) -> [CGFloat; 4] {
        let c = c.mul_alpha(self.alpha);
        [c.r as CGFloat, c.g as CGFloat, c.b as CGFloat, c.a.clamp(0.0, 1.0) as CGFloat]
    }

    fn set_fill(&self, c: Color) {
        let k = self.components(self.g.look.paint(c));
        unsafe { CGContext::set_fill_color(self.cg(), k.as_ptr()) };
    }

    fn set_stroke(&self, c: Color, width: f32) {
        let k = self.components(self.g.look.paint(c));
        unsafe { CGContext::set_stroke_color(self.cg(), k.as_ptr()) };
        CGContext::set_line_width(self.cg(), width as CGFloat);
    }

    /// Text and icons take a coloured label's shade, not a fill's.
    fn set_text_fill(&self, c: Color) {
        let k = self.components(self.g.look.paint_text(c));
        unsafe { CGContext::set_fill_color(self.cg(), k.as_ptr()) };
    }

    fn set_text_stroke(&self, c: Color, width: f32) {
        let k = self.components(self.g.look.paint_text(c));
        unsafe { CGContext::set_stroke_color(self.cg(), k.as_ptr()) };
        CGContext::set_line_width(self.cg(), width as CGFloat);
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
        self.set_fill(c);
        CGContext::fill_rect(self.cg(), cgrect(r));
    }

    fn add_rrect(&self, r: Rect, radius: f32) {
        let rad = radius.min(r.w / 2.0).min(r.h / 2.0).max(0.0) as CGFloat;
        if rad <= 0.0 {
            CGContext::add_rect(self.cg(), cgrect(r));
            return;
        }
        let p = unsafe { CGPath::with_rounded_rect(cgrect(r), rad, rad, ptr::null()) };
        CGContext::add_path(self.cg(), Some(&p));
    }

    pub fn fill_rrect(&mut self, r: Rect, radius: f32, c: Color) {
        if r.w <= 0.0 || r.h <= 0.0 {
            return;
        }
        self.set_fill(c);
        CGContext::begin_path(self.cg());
        self.add_rrect(r, radius);
        CGContext::fill_path(self.cg());
    }

    pub fn stroke_rrect(&mut self, r: Rect, radius: f32, c: Color, width: f32) {
        let r = r.inset(width / 2.0, width / 2.0);
        if r.w <= 0.0 || r.h <= 0.0 {
            return;
        }
        self.set_stroke(c, width);
        CGContext::begin_path(self.cg());
        self.add_rrect(r, radius);
        CGContext::stroke_path(self.cg());
    }

    pub fn fill_circle(&mut self, c: Point, radius: f32, color: Color) {
        self.set_fill(color);
        CGContext::fill_ellipse_in_rect(self.cg(), cgrect(Rect::from_center(c, 2.0 * radius, 2.0 * radius)));
    }

    pub fn stroke_circle(&mut self, c: Point, radius: f32, color: Color, width: f32) {
        self.set_stroke(color, width);
        CGContext::stroke_ellipse_in_rect(self.cg(), cgrect(Rect::from_center(c, 2.0 * radius, 2.0 * radius)));
    }

    pub fn line(&mut self, a: Point, b: Point, c: Color, width: f32) {
        self.set_stroke(c, width);
        CGContext::begin_path(self.cg());
        CGContext::move_to_point(self.cg(), a.x as CGFloat, a.y as CGFloat);
        CGContext::add_line_to_point(self.cg(), b.x as CGFloat, b.y as CGFloat);
        CGContext::stroke_path(self.cg());
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
        self.path(&segs, None, Some((color, width)));
    }

    /// Progress ring: full track circle plus a progress arc on top.
    pub fn ring(&mut self, c: Point, radius: f32, width: f32, progress: f32, track: Color, fill: Color) {
        self.stroke_circle(c, radius, track, width);
        self.arc(c, radius, 0.0, progress, fill, width);
    }

    /// Fill and/or stroke arbitrary path segments.
    pub fn path(&mut self, segs: &[Seg], fill: Option<Color>, stroke: Option<(Color, f32)>) {
        let p = cg_path(segs);
        if let Some(f) = fill {
            self.set_fill(f);
            CGContext::begin_path(self.cg());
            CGContext::add_path(self.cg(), Some(&p));
            CGContext::fill_path(self.cg());
        }
        if let Some((c, w)) = stroke {
            self.set_stroke(c, w);
            CGContext::begin_path(self.cg());
            CGContext::add_path(self.cg(), Some(&p));
            CGContext::stroke_path(self.cg());
        }
    }

    /// Fill a path with `c` fading from `alpha0` at height `y0` to `alpha1`
    /// at `y1` (a vertical gradient, as under Stats' charts); above `y0` and
    /// below `y1` the end opacities carry on, as Direct2D's gradient brush
    /// does on Windows.
    pub fn fill_path_fade(&mut self, segs: &[Seg], c: Color, y0: f32, y1: f32, alpha0: f32, alpha1: f32) {
        let paint = self.g.look.paint(c);
        let (a, b) = (self.components(paint.mul_alpha(alpha0)), self.components(paint.mul_alpha(alpha1)));
        let stops = [a[0], a[1], a[2], a[3], b[0], b[1], b[2], b[3]];
        let at: [CGFloat; 2] = [0.0, 1.0];
        let Some(grad) = (unsafe { CGGradient::with_color_components(Some(&self.g.srgb), stops.as_ptr(), at.as_ptr(), 2) }) else { return };
        CGContext::save_g_state(self.cg());
        let p = cg_path(segs);
        CGContext::begin_path(self.cg());
        CGContext::add_path(self.cg(), Some(&p));
        CGContext::clip(self.cg());
        let (from, to) = (CGPoint { x: 0.0, y: y0 as CGFloat }, CGPoint { x: 0.0, y: y1 as CGFloat });
        let extend = CGGradientDrawingOptions::DrawsBeforeStartLocation | CGGradientDrawingOptions::DrawsAfterEndLocation;
        CGContext::draw_linear_gradient(self.cg(), Some(&grad), from, to, extend);
        CGContext::restore_g_state(self.cg());
    }

    /// A soft shadow round the rounded rectangle `r` (corner `radius`): `c`
    /// at the opacity `ramp` gives by distance from the edge (DIPs, negative
    /// inside, in order), from its first stop inside the edge to its last
    /// outside it, where it should reach 0. Deeper inside than the first
    /// stop it may draw or not: cover that, as the floating island's body
    /// covers its shadow. The same gradient pieces as on Windows (see
    /// `pill::shape::shadow_plan`), each clipped to its rectangle.
    pub fn rrect_shadow(&mut self, r: Rect, radius: f32, c: Color, ramp: &[(f32, f32)]) {
        let Some(plan) = crate::pill::shape::shadow_plan(r, radius, ramp, self.scale()) else { return };
        let paint = self.g.look.paint(c);
        let n = plan.stops.len().min(16);
        let mut comps: [CGFloat; 64] = [0.0; 64];
        let mut at: [CGFloat; 16] = [0.0; 16];
        for (i, &(p, a)) in plan.stops.iter().take(n).enumerate() {
            comps[i * 4..i * 4 + 4].copy_from_slice(&self.components(paint.mul_alpha(a)));
            at[i] = p as CGFloat;
        }
        let Some(grad) = (unsafe { CGGradient::with_color_components(Some(&self.g.srgb), comps.as_ptr(), at.as_ptr(), n) }) else { return };
        let extend = CGGradientDrawingOptions::DrawsBeforeStartLocation | CGGradientDrawingOptions::DrawsAfterEndLocation;
        let cgpt = |p: Point| CGPoint { x: p.x as CGFloat, y: p.y as CGFloat };
        let reach = plan.reach as CGFloat;
        for (square, o) in plan.corners.into_iter().flatten() {
            CGContext::save_g_state(self.cg());
            CGContext::clip_to_rect(self.cg(), cgrect(square));
            CGContext::draw_radial_gradient(self.cg(), Some(&grad), cgpt(o), 0.0, cgpt(o), reach, extend);
            CGContext::restore_g_state(self.cg());
        }
        for (strip, a, b) in plan.sides.into_iter().flatten() {
            CGContext::save_g_state(self.cg());
            CGContext::clip_to_rect(self.cg(), cgrect(strip));
            CGContext::draw_linear_gradient(self.cg(), Some(&grad), cgpt(a), cgpt(b), extend);
            CGContext::restore_g_state(self.cg());
        }
        if let Some((m, a)) = plan.middle {
            let k = self.components(paint.mul_alpha(a));
            unsafe { CGContext::set_fill_color(self.cg(), k.as_ptr()) };
            CGContext::fill_rect(self.cg(), cgrect(m));
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

    /// Restore the graphics state saved by a clip or mask, keeping the
    /// transform set since (clips here never undo a transform, as on Windows).
    fn pop_state(&mut self) {
        if self.stack == 0 {
            return;
        }
        let m = CGContext::ctm(self.cg());
        CGContext::restore_g_state(self.cg());
        self.stack -= 1;
        let now = Matrix3x2::from_cg(CGContext::ctm(self.cg()));
        let want = Matrix3x2::from_cg(m);
        if now != want {
            CGContext::concat_ctm(self.cg(), (want * now.inverse()).to_cg());
        }
    }

    /// Mask subsequent drawing to the island silhouette (anti-aliased) until
    /// [`Canvas::pop_shape_mask`].
    pub fn push_shape_mask(&mut self, body: Rect, radius: f32, shoulder: f32) {
        self.push_outline_mask(&notch_segments(body, radius, shoulder));
    }

    /// Mask subsequent drawing to the closed outline `segs` until
    /// [`Canvas::pop_shape_mask`] (the floating island's outline).
    pub fn push_outline_mask(&mut self, segs: &[Seg]) {
        CGContext::save_g_state(self.cg());
        self.stack += 1;
        let p = cg_path(segs);
        CGContext::begin_path(self.cg());
        CGContext::add_path(self.cg(), Some(&p));
        CGContext::clip(self.cg());
    }

    pub fn pop_shape_mask(&mut self) {
        self.pop_state();
    }

    /// Draw text in `r`. Single-line text is ellipsized; see [`TextStyle`].
    pub fn text(&mut self, s: &str, r: Rect, st: &TextStyle) {
        if s.is_empty() || r.w <= 0.0 || r.h <= 0.0 {
            return;
        }
        let lay = self.g.layout(s, st, r.w);
        if lay.lines.is_empty() {
            return;
        }
        self.set_text_fill(st.color);
        let total = lay.lines.len() as f32 * lay.line_h;
        let mut y = if st.vcenter { r.y + (r.h - total) / 2.0 } else { r.y };
        let base = if st.line > 0.0 { 0.8 * lay.line_h } else { lay.leading / 2.0 + lay.ascent };
        // Glyphs are drawn upright in this y-down space.
        CGContext::set_text_matrix(self.cg(), CGAffineTransform { a: 1.0, b: 0.0, c: 0.0, d: -1.0, tx: 0.0, ty: 0.0 });
        for (line, lw) in &lay.lines {
            let x = match st.align {
                Align::Left => r.x,
                Align::Center => r.x + (r.w - lw) / 2.0,
                Align::Right => r.right() - lw,
            };
            CGContext::set_text_position(self.cg(), x as CGFloat, (y + base) as CGFloat);
            unsafe { line.draw(self.ctx) };
            y += lay.line_h;
        }
    }

    /// Measure text: (width, height) in DIPs, laid out within `max_w`.
    pub fn measure(&mut self, s: &str, st: &TextStyle, max_w: f32) -> (f32, f32) {
        let mut st = *st;
        st.align = Align::Left;
        let lay = self.g.layout(s, &st, max_w.max(1.0));
        let lines = lay.lines.len().max(1) as f32;
        (lay.width.min(max_w.max(1.0)), lines * lay.line_h)
    }

    /// Draw an image cover-fitted into `r` with rounded corners.
    pub fn image(&mut self, img: &Image, r: Rect, radius: f32) {
        if img.width == 0 || img.height == 0 || r.w <= 0.0 || r.h <= 0.0 {
            return;
        }
        let Some(cg) = self.g.cg_image(img) else { return };
        let (iw, ih) = (img.width as f32, img.height as f32);
        let s = (r.w / iw).max(r.h / ih);
        let (dw, dh) = (iw * s, ih * s);
        let ox = r.x + (r.w - dw) / 2.0;
        let oy = r.y + (r.h - dh) / 2.0;
        CGContext::save_g_state(self.cg());
        CGContext::begin_path(self.cg());
        self.add_rrect(r, radius);
        CGContext::clip(self.cg());
        CGContext::set_alpha(self.cg(), self.alpha as CGFloat);
        // Images draw bottom-up; flip them back in this y-down space.
        CGContext::translate_ctm(self.cg(), ox as CGFloat, (oy + dh) as CGFloat);
        CGContext::scale_ctm(self.cg(), 1.0, -1.0);
        CGContext::draw_image(self.cg(), cgrect(Rect::new(0.0, 0.0, dw, dh)), Some(&cg));
        CGContext::restore_g_state(self.cg());
    }

    /// Draw a vector icon (24×24 design grid) scaled into `r`.
    pub fn icon(&mut self, icon: Icon, r: Rect, color: Color) {
        let paths = self.g.icon_paths(icon);
        if paths.is_empty() {
            return;
        }
        let s = r.w.min(r.h) / 24.0;
        let ox = r.x + (r.w - 24.0 * s) / 2.0;
        let oy = r.y + (r.h - 24.0 * s) / 2.0;
        CGContext::save_g_state(self.cg());
        CGContext::translate_ctm(self.cg(), ox as CGFloat, oy as CGFloat);
        CGContext::scale_ctm(self.cg(), s as CGFloat, s as CGFloat);
        self.set_text_fill(color);
        self.set_text_stroke(color, icons::STROKE);
        for (p, fill) in &paths {
            CGContext::begin_path(self.cg());
            CGContext::add_path(self.cg(), Some(p));
            if *fill {
                CGContext::fill_path(self.cg());
            } else {
                CGContext::stroke_path(self.cg());
            }
        }
        CGContext::restore_g_state(self.cg());
    }

    /// Clip subsequent drawing to `r` until [`Canvas::unclip`].
    pub fn clip(&mut self, r: Rect) {
        CGContext::save_g_state(self.cg());
        self.stack += 1;
        CGContext::clip_to_rect(self.cg(), cgrect(r));
    }

    pub fn unclip(&mut self) {
        self.pop_state();
    }

    fn set_transform(&mut self, rel: Matrix3x2) {
        let want = rel * self.base;
        let now = Matrix3x2::from_cg(CGContext::ctm(self.cg()));
        if now != want {
            CGContext::concat_ctm(self.cg(), (want * now.inverse()).to_cg());
        }
    }

    /// Translate subsequent drawing by (dx, dy) until [`Canvas::reset_transform`].
    pub fn translate(&mut self, dx: f32, dy: f32) {
        CGContext::translate_ctm(self.cg(), dx as CGFloat, dy as CGFloat);
    }

    pub fn reset_transform(&mut self) {
        self.set_transform(Matrix3x2::identity());
    }

    /// The current transform (pair with [`Canvas::restore_transform`]
    /// instead of `reset_transform`, which would also drop outer transforms).
    pub fn save_transform(&self) -> Matrix3x2 {
        Matrix3x2::from_cg(CGContext::ctm(self.cg())) * self.base.inverse()
    }

    pub fn restore_transform(&mut self, m: Matrix3x2) {
        self.set_transform(m);
    }

    /// Scale subsequent drawing by `s` about point `p` (on top of the current transform).
    pub fn scale_about(&mut self, p: Point, s: f32) {
        CGContext::concat_ctm(self.cg(), scale_about(p, s).to_cg());
    }

    /// Draw `f` as one group covering `bounds`, composited at opacity `alpha`
    /// and moved by `xf` (on top of the current transform). Overlapping parts
    /// don't show through each other while it fades. `f` always runs, even
    /// when the group ends up invisible, so hit regions and animation state
    /// stay continuous.
    pub fn layered<R>(&mut self, bounds: Rect, alpha: f32, xf: Matrix3x2, f: impl FnOnce(&mut Canvas<'a>) -> R) -> R {
        let a = (alpha * self.alpha).clamp(0.0, 1.0);
        CGContext::save_g_state(self.cg());
        CGContext::concat_ctm(self.cg(), xf.to_cg());
        CGContext::set_alpha(self.cg(), a as CGFloat);
        unsafe { CGContext::begin_transparency_layer_with_rect(self.cg(), cgrect(bounds), None) };
        let outer_alpha = self.alpha;
        let depth = self.stack;
        self.alpha = 1.0;
        let r = f(self);
        while self.stack > depth {
            self.pop_state();
        }
        self.alpha = outer_alpha;
        CGContext::end_transparency_layer(self.cg());
        CGContext::restore_g_state(self.cg());
        r
    }
}

impl Drop for Canvas<'_> {
    fn drop(&mut self) {
        // A page that forgot an unclip mustn't leak a clip into the next frame.
        while self.stack > 0 {
            self.pop_state();
        }
    }
}

// ---------------------------------------------------------------- offscreen bitmaps

/// An offscreen premultiplied BGRA bitmap with a DIP canvas transform (top
/// left origin, y down), for `--render` and the menu bar icon.
pub struct Bitmap {
    ctx: CFRetained<CGContext>,
    pub w: usize,
    pub h: usize,
}

impl Bitmap {
    pub fn new(w: usize, h: usize, scale: f32) -> Option<Bitmap> {
        let srgb = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }))?;
        let info = CGImageAlphaInfo::PremultipliedFirst.0 | CGImageByteOrderInfo::Order32Little.0;
        let ctx = unsafe { CGBitmapContextCreate(ptr::null_mut(), w, h, 8, w * 4, Some(&srgb), info) }?;
        CGContext::translate_ctm(Some(&ctx), 0.0, h as CGFloat);
        CGContext::scale_ctm(Some(&ctx), scale as CGFloat, -(scale as CGFloat));
        Some(Bitmap { ctx, w, h })
    }

    pub fn ctx(&self) -> &CGContext {
        &self.ctx
    }

    /// The pixels, top row first (premultiplied BGRA).
    pub fn pixels(&self) -> Vec<u8> {
        let p = CGBitmapContextGetData(Some(&self.ctx)) as *const u8;
        if p.is_null() {
            return Vec::new();
        }
        unsafe { std::slice::from_raw_parts(p, self.w * self.h * 4) }.to_vec()
    }

    pub fn image(&self) -> Option<CFRetained<CGImage>> {
        CGBitmapContextCreateImage(Some(&self.ctx))
    }
}
