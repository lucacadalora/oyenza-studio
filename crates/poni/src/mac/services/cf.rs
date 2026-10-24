//! The few Core Foundation calls the services make on objects handed over as
//! plain C pointers (IOKit's power sources, ImageIO's options).

use objc2_core_foundation::{CFRetained, CFString};
use std::ffi::c_void;

pub type CFTypeRef = *const c_void;

#[allow(non_upper_case_globals)]
extern "C" {
    pub fn CFRelease(cf: CFTypeRef);
    pub fn CFDictionaryGetValue(dict: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
    pub fn CFArrayGetCount(array: CFTypeRef) -> isize;
    pub fn CFArrayGetValueAtIndex(array: CFTypeRef, index: isize) -> CFTypeRef;
    fn CFNumberGetValue(number: CFTypeRef, the_type: isize, value: *mut c_void) -> u8;
    fn CFBooleanGetValue(boolean: CFTypeRef) -> u8;
    fn CFGetTypeID(cf: CFTypeRef) -> usize;
    fn CFNumberGetTypeID() -> usize;
    fn CFBooleanGetTypeID() -> usize;
    fn CFEqual(a: CFTypeRef, b: CFTypeRef) -> u8;
    fn CFStringGetTypeID() -> usize;
    fn CFDataGetTypeID() -> usize;
    fn CFDataGetLength(data: CFTypeRef) -> isize;
    fn CFDataGetBytePtr(data: CFTypeRef) -> *const u8;
    pub fn CFRunLoopGetMain() -> CFTypeRef;
    pub fn CFRunLoopAddSource(rl: CFTypeRef, source: CFTypeRef, mode: CFTypeRef);
    pub static kCFRunLoopCommonModes: CFTypeRef;
}

/// `kCFNumberSInt64Type`.
const SINT64: isize = 4;

/// A CFString key (kept alive by the caller while it is used as a pointer).
pub fn key(s: &str) -> CFRetained<CFString> {
    CFString::from_str(s)
}

fn ptr(s: &CFString) -> CFTypeRef {
    s as *const CFString as CFTypeRef
}

/// `dict[k]` as an integer, when it is a CFNumber.
pub fn dict_i64(dict: CFTypeRef, k: &str) -> Option<i64> {
    let k = key(k);
    unsafe {
        let v = CFDictionaryGetValue(dict, ptr(&k));
        if v.is_null() || CFGetTypeID(v) != CFNumberGetTypeID() {
            return None;
        }
        let mut out: i64 = 0;
        (CFNumberGetValue(v, SINT64, &mut out as *mut i64 as *mut c_void) != 0).then_some(out)
    }
}

/// `dict[k]` as a bool, when it is a CFBoolean.
pub fn dict_bool(dict: CFTypeRef, k: &str) -> Option<bool> {
    let k = key(k);
    unsafe {
        let v = CFDictionaryGetValue(dict, ptr(&k));
        if v.is_null() || CFGetTypeID(v) != CFBooleanGetTypeID() {
            return None;
        }
        Some(CFBooleanGetValue(v) != 0)
    }
}

/// Whether `dict[k]` is the string `want`.
pub fn dict_str_is(dict: CFTypeRef, k: &str, want: &str) -> bool {
    let (k, want) = (key(k), key(want));
    unsafe {
        let v = CFDictionaryGetValue(dict, ptr(&k));
        !v.is_null() && CFEqual(v, ptr(&want)) != 0
    }
}

/// Whether a CFString pointer equals `want`.
pub fn str_is(v: CFTypeRef, want: &str) -> bool {
    let want = key(want);
    !v.is_null() && unsafe { CFEqual(v, ptr(&want)) != 0 }
}

/// A CFNumber as an integer (IOKit's registry values).
pub fn number_i64(v: CFTypeRef) -> Option<i64> {
    unsafe {
        if v.is_null() || CFGetTypeID(v) != CFNumberGetTypeID() {
            return None;
        }
        let mut out: i64 = 0;
        (CFNumberGetValue(v, SINT64, &mut out as *mut i64 as *mut c_void) != 0).then_some(out)
    }
}

/// A CFData's bytes.
pub fn data_bytes(v: CFTypeRef) -> Option<Vec<u8>> {
    unsafe {
        if v.is_null() || CFGetTypeID(v) != CFDataGetTypeID() {
            return None;
        }
        let (p, n) = (CFDataGetBytePtr(v), CFDataGetLength(v));
        Some(if p.is_null() || n <= 0 { Vec::new() } else { std::slice::from_raw_parts(p, n as usize).to_vec() })
    }
}

/// A CFString's text, or a CFData's as a C string (how the registry keeps
/// some names); None for anything else.
pub fn text(v: CFTypeRef) -> Option<String> {
    if v.is_null() {
        return None;
    }
    if unsafe { CFGetTypeID(v) } == unsafe { CFStringGetTypeID() } {
        // SAFETY: a CFString, borrowed for the call.
        return Some(unsafe { &*(v as *const CFString) }.to_string());
    }
    let b = data_bytes(v)?;
    let end = b.iter().position(|c| *c == 0).unwrap_or(b.len());
    Some(String::from_utf8_lossy(&b[..end]).trim().to_string())
}
