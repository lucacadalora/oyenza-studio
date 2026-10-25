//! The Mac's own power sensor, for the System page's Power module: its
//! System Management Controller (SMC), read through IOKit the way Stats
//! reads it. `PSTR` is the whole Mac's draw in watts on Apple silicon (a
//! 32-bit float); it's opened while the System page is open, on a Mac
//! without a battery. No admin rights, no helper, no private framework.
//!
//! Apple doesn't document the SMC's keys: a Mac without this one (an Intel
//! Mac, a virtual Mac) just has no Power module.

use std::ffi::{c_char, c_void};

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Vers {
    major: u8,
    minor: u8,
    build: u8,
    reserved: u8,
    release: u16,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Limits {
    version: u16,
    length: u16,
    cpu: u32,
    gpu: u32,
    memory: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct KeyInfo {
    size: u32,
    kind: u32,
    attributes: u8,
}

/// What the SMC's user client takes and gives back (`SMCKeyData_t`).
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Params {
    key: u32,
    vers: Vers,
    limits: Limits,
    info: KeyInfo,
    result: u8,
    status: u8,
    command: u8,
    data32: u32,
    bytes: [u8; 32],
}

const _: () = assert!(std::mem::size_of::<Params>() == 80);

/// The user client's one method, and the two commands this sends it.
const KERNEL_INDEX: u32 = 2;
const READ_BYTES: u8 = 5;
const READ_KEY_INFO: u8 = 9;

const fn fourcc(s: &[u8; 4]) -> u32 {
    ((s[0] as u32) << 24) | ((s[1] as u32) << 16) | ((s[2] as u32) << 8) | s[3] as u32
}

/// The whole Mac's draw (W), what Stats shows as System total.
pub const SYSTEM_TOTAL: u32 = fourcc(b"PSTR");
const FLOAT: u32 = fourcc(b"flt ");

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOServiceMatching(name: *const c_char) -> *const c_void;
    fn IOServiceGetMatchingService(main_port: u32, matching: *const c_void) -> u32;
    fn IOServiceOpen(service: u32, owning_task: u32, kind: u32, connect: *mut u32) -> i32;
    fn IOServiceClose(connect: u32) -> i32;
    fn IOObjectRelease(object: u32) -> i32;
    fn IOConnectCallStructMethod(connection: u32, selector: u32, input: *const c_void, input_size: usize, output: *mut c_void, output_size: *mut usize) -> i32;
}

extern "C" {
    static mach_task_self_: u32;
}

/// An open connection to the SMC (closed when dropped).
pub struct Smc {
    conn: u32,
}

impl Smc {
    pub fn open() -> Option<Smc> {
        let matching = unsafe { IOServiceMatching(c"AppleSMC".as_ptr()) };
        if matching.is_null() {
            return None;
        }
        // The call takes the matching dictionary over.
        let service = unsafe { IOServiceGetMatchingService(0, matching) };
        if service == 0 {
            return None;
        }
        let mut conn = 0u32;
        let r = unsafe { IOServiceOpen(service, mach_task_self_, 0, &mut conn) };
        unsafe { IOObjectRelease(service) };
        (r == 0 && conn != 0).then_some(Smc { conn })
    }

    fn call(&self, input: &Params) -> Option<Params> {
        let mut out = Params::default();
        let mut size = std::mem::size_of::<Params>();
        let r = unsafe {
            IOConnectCallStructMethod(self.conn, KERNEL_INDEX, input as *const Params as *const c_void, size, &mut out as *mut Params as *mut c_void, &mut size)
        };
        (r == 0 && out.result == 0).then_some(out)
    }

    /// A key's value when it's a float, the way Apple silicon keeps its power readings.
    pub fn float(&self, key: u32) -> Option<f32> {
        let info = self.call(&Params { key, command: READ_KEY_INFO, ..Default::default() })?.info;
        if info.kind != FLOAT || info.size != 4 {
            return None;
        }
        let out = self.call(&Params { key, info, command: READ_BYTES, ..Default::default() })?;
        Some(f32::from_le_bytes([out.bytes[0], out.bytes[1], out.bytes[2], out.bytes[3]]))
    }
}

impl Drop for Smc {
    fn drop(&mut self) {
        unsafe { IOServiceClose(self.conn) };
    }
}

/// `poni --smc`: what the SMC says the whole Mac draws now, for checking a
/// Mac by hand (the Power module shows it only on a Mac without a battery).
pub fn report() -> i32 {
    match Smc::open() {
        None => {
            println!("SMC: not available on this Mac");
            1
        }
        Some(smc) => match smc.float(SYSTEM_TOTAL) {
            Some(w) => {
                println!("SMC: the whole Mac draws {w:.2} W (PSTR)");
                0
            }
            None => {
                println!("SMC: open, but no system power reading (PSTR)");
                1
            }
        },
    }
}
