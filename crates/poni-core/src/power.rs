//! The whole PC's power draw, for the System page's Power module.
//!
//! Where the number comes from, best first:
//! * a power supply that reports over USB: Corsair's HXi and RMi (PMBus
//!   commands inside a vendor HID report) and MSI's MEG Ai and MPG Ai…TS
//!   models (a register protocol of MSI's own). Both are written here from
//!   their public descriptions: the Linux `corsair-psu` driver's
//!   documentation, and the protocol notes of the MIT-licensed
//!   msi-psu-monitor. Only reads go to a PSU, and the handshake its own app
//!   sends too;
//! * else, on a desktop, an estimate: what the CPU package and the graphics
//!   card report of their own power, plus [`REST_W`] for the rest of the PC.
//!
//! The Windows side (`poni::services::psu`, `services::sysmon`) does the I/O;
//! the decoding and the sums are here, so they're tested anywhere.

/// A PSU's HID report without its report id (always 0 on these PSUs).
pub const REPORT: usize = 64;

pub const CORSAIR_VID: u16 = 0x1b1c;
pub const MSI_VID: u16 = 0x0db0;

/// Corsair's PMBus commands this reads.
pub mod corsair {
    pub const READ_VIN: u8 = 0x88;
    /// Input current: answered by some models only.
    pub const READ_IIN: u8 = 0x89;
    pub const READ_TEMP_1: u8 = 0x8d;
    pub const READ_TEMP_2: u8 = 0x8e;
    pub const READ_FAN: u8 = 0x90;
    /// The total output power.
    pub const TOTAL_OUT: u8 = 0xee;
}

/// MSI's registers this reads.
pub mod msi {
    pub const VIN: u8 = 0x20;
    /// Everything at once, in the model's own layout ([`super::MsiLayout`]).
    pub const ALL: u8 = 0xe0;
}

/// How a power supply talks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protocol {
    /// Corsair HXi / RMi: PMBus commands, read one value at a time.
    Corsair,
    /// MSI: register reads; the bulk read comes in the model's layout.
    Msi(MsiLayout),
}

/// The layouts of MSI's bulk read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MsiLayout {
    /// MEG Ai1000P / Ai1300P: five 12 V rails and the combined one (40 bytes).
    MegP,
    /// MEG Ai1600T: one 12 V rail (20 bytes).
    MegT,
    /// MPG Ai1300TS / Ai1600TS: the 12V-2x6 connectors' pin currents too
    /// (44 bytes of data).
    MpgTs,
}

impl MsiLayout {
    /// The layout for the product name the PSU gives in its handshake (MSI's
    /// software goes by the name, not the USB id).
    pub fn from_name(name: &str) -> Option<MsiLayout> {
        let n = name.to_ascii_uppercase();
        if n.contains("TS") {
            Some(MsiLayout::MpgTs)
        } else if n.contains("AI1600T") {
            Some(MsiLayout::MegT)
        } else if n.contains("AI1300P") || n.contains("AI1000P") {
            Some(MsiLayout::MegP)
        } else {
            None
        }
    }

    /// The bulk read's data length.
    fn len(self) -> usize {
        match self {
            MsiLayout::MegP => 40,
            MsiLayout::MegT => 20,
            MsiLayout::MpgTs => 44,
        }
    }
}

/// A power supply this knows, by its USB ids.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PsuModel {
    pub vid: u16,
    pub pid: u16,
    pub name: &'static str,
    pub protocol: Protocol,
}

const fn corsair_model(pid: u16, name: &'static str) -> PsuModel {
    PsuModel { vid: CORSAIR_VID, pid, name, protocol: Protocol::Corsair }
}

const fn msi_model(pid: u16, name: &'static str, layout: MsiLayout) -> PsuModel {
    PsuModel { vid: MSI_VID, pid, name, protocol: Protocol::Msi(layout) }
}

pub const MODELS: [PsuModel; 20] = [
    corsair_model(0x1c03, "Corsair HX550i"),
    corsair_model(0x1c04, "Corsair HX650i"),
    corsair_model(0x1c05, "Corsair HX750i"),
    corsair_model(0x1c06, "Corsair HX850i"),
    corsair_model(0x1c07, "Corsair HX1000i"),
    corsair_model(0x1c08, "Corsair HX1200i"),
    corsair_model(0x1c09, "Corsair RM550i"),
    corsair_model(0x1c0a, "Corsair RM650i"),
    corsair_model(0x1c0b, "Corsair RM750i"),
    corsair_model(0x1c0c, "Corsair RM850i"),
    corsair_model(0x1c0d, "Corsair RM1000i"),
    corsair_model(0x1c1e, "Corsair HX1000i"),
    corsair_model(0x1c1f, "Corsair HX1500i"),
    corsair_model(0x1c23, "Corsair HX1200i"),
    corsair_model(0x1c27, "Corsair HX1200i"),
    msi_model(0xe749, "MSI MEG Ai1000P", MsiLayout::MegP),
    msi_model(0x56d4, "MSI MEG Ai1300P", MsiLayout::MegP),
    msi_model(0xc9eb, "MSI MEG Ai1600T", MsiLayout::MegT),
    msi_model(0xaa6f, "MSI MPG Ai1300TS", MsiLayout::MpgTs),
    msi_model(0x808c, "MSI MPG Ai1600TS", MsiLayout::MpgTs),
];

pub fn model(vid: u16, pid: u16) -> Option<&'static PsuModel> {
    MODELS.iter().find(|m| m.vid == vid && m.pid == pid)
}

/// The USB ids in a HID device path (`\\?\hid#vid_1b1c&pid_1c1f#...`).
pub fn usb_ids(path: &str) -> Option<(u16, u16)> {
    let p = path.to_ascii_lowercase();
    let hex = |key: &str| {
        let at = p.find(key)? + key.len();
        u16::from_str_radix(p.get(at..at + 4)?, 16).ok()
    };
    Some((hex("vid_")?, hex("pid_")?))
}

/// A PMBus LINEAR11 word: a two's-complement exponent in the top five bits
/// over an eleven-bit mantissa. PMBus (and Corsair) sign the mantissa too;
/// MSI's own software reads it unsigned, which only differs above 1023.
pub fn linear11(word: u16, signed: bool) -> f32 {
    let exp = (word as i16) >> 11;
    let low = i32::from(word & 0x7ff);
    let mant = if signed && low & 0x400 != 0 { low - 0x800 } else { low };
    mant as f32 * 2f32.powi(i32::from(exp))
}

fn report(b0: u8, b1: u8) -> [u8; REPORT] {
    let mut r = [0u8; REPORT];
    r[0] = b0;
    r[1] = b1;
    r
}

/// Corsair: the handshake a PSU wants after power-up before it answers (the
/// one iCUE and Linux send; it changes no setting).
pub fn corsair_hello() -> [u8; REPORT] {
    report(0xfe, 0x03)
}

/// Corsair: read `cmd` (the PSU's address 0x02, with the read bit).
pub fn corsair_read(cmd: u8) -> [u8; REPORT] {
    report(0x03, cmd)
}

/// Corsair: the value in `reply` if it answers `cmd` (an unsupported
/// command comes back with 0 in its place).
pub fn corsair_value(cmd: u8, reply: &[u8]) -> Option<f32> {
    if reply.len() < 4 || reply[0] != 0x03 || reply[1] != cmd {
        return None;
    }
    Some(linear11(u16::from_le_bytes([reply[2], reply[3]]), true)).filter(|v| v.is_finite())
}

/// MSI: the handshake, answered with the product name.
pub fn msi_hello() -> [u8; REPORT] {
    report(0xfa, 0x51)
}

/// MSI: read register `reg`.
pub fn msi_read(reg: u8) -> [u8; REPORT] {
    report(0x51, reg)
}

/// MSI: the product name in the handshake's answer ("MPG Ai1600TS").
pub fn msi_name(reply: &[u8]) -> Option<String> {
    if reply.len() < 3 || reply[0] != 0xfa || reply[1] != 0x51 {
        return None;
    }
    let text = &reply[2..];
    let end = text.iter().position(|b| *b == 0).unwrap_or(text.len());
    let name: String = text[..end].iter().filter(|b| b.is_ascii_graphic() || **b == b' ').map(|b| char::from(*b)).collect();
    let name = name.trim().to_string();
    (!name.is_empty()).then_some(name)
}

/// MSI: the data in `reply` if it answers `reg` (an unsupported register
/// answers `[0x51, 0xFE]`).
pub fn msi_data(reg: u8, reply: &[u8]) -> Option<&[u8]> {
    (reply.len() > 2 && reply[0] == 0x51 && reply[1] == reg).then(|| &reply[2..])
}

/// One of a PSU's outputs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rail {
    /// What it's named for: 12.0, 5.0 or 3.3 (V).
    pub nominal: f32,
    pub volts: f32,
    pub amps: f32,
}

impl Rail {
    pub fn watts(&self) -> f32 {
        self.volts * self.amps
    }
}

/// What a power supply reports.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PsuReading {
    /// "Corsair HX1500i", "MSI MEG Ai1600T".
    pub name: String,
    /// Its output: what the PC draws (W).
    pub out_w: f32,
    /// From the wall (W): its input reading, or its output over its own efficiency.
    pub in_w: Option<f32>,
    /// 0..=1, as the PSU works it out.
    pub efficiency: Option<f32>,
    /// The mains voltage (V).
    pub mains_v: Option<f32>,
    pub temp_c: Option<f32>,
    pub fan_rpm: Option<u32>,
    /// +12 V, +5 V and +3.3 V, when it reports them.
    pub rails: Vec<Rail>,
}

/// Only readings a working PSU could give count.
fn watts(w: f32) -> Option<f32> {
    (w.is_finite() && (0.0..=4000.0).contains(&w)).then_some(w)
}

fn temp(c: f32) -> Option<f32> {
    (c.is_finite() && c > -20.0 && c < 150.0).then_some(c)
}

fn rpm(r: f32) -> Option<u32> {
    (r.is_finite() && (0.0..=20_000.0).contains(&r)).then(|| r.round() as u32)
}

fn mains(v: f32) -> Option<f32> {
    (v.is_finite() && (80.0..=300.0).contains(&v)).then_some(v)
}

/// An efficiency as a share: PSUs give it as a share or in percent.
fn efficiency(v: f32) -> Option<f32> {
    let share = if v > 1.5 { v / 100.0 } else { v };
    (share.is_finite() && (0.3..=1.0).contains(&share)).then_some(share)
}

fn rail(nominal: f32, volts: f32, amps: f32) -> Option<Rail> {
    let ok = volts.is_finite() && amps.is_finite() && volts > nominal * 0.5 && volts < nominal * 1.5 && (0.0..=400.0).contains(&amps);
    ok.then_some(Rail { nominal, volts, amps })
}

/// Corsair: the values read one by one (`None` where the PSU didn't answer)
/// as one reading. Without the total output there's nothing to show.
pub fn corsair_reading(name: &str, out_w: Option<f32>, mains_v: Option<f32>, in_amps: Option<f32>, temp_c: Option<f32>, fan: Option<f32>) -> Option<PsuReading> {
    let out_w = watts(out_w?)?;
    let mains_v = mains_v.and_then(mains);
    // Volts times amps at the input is its apparent power: within a few
    // percent of the real power behind active power factor correction, and
    // never below the output.
    let in_w = mains_v.zip(in_amps).map(|(v, a)| v * a).and_then(watts).filter(|w| *w >= out_w && out_w > 0.0);
    Some(PsuReading {
        name: name.to_string(),
        out_w,
        in_w,
        efficiency: in_w.map(|i| out_w / i).and_then(efficiency),
        mains_v,
        temp_c: temp_c.and_then(temp),
        fan_rpm: fan.and_then(rpm),
        rails: Vec::new(),
    })
}

/// MSI: the bulk read's data in `layout`, as one reading.
pub fn msi_reading(name: &str, layout: MsiLayout, data: &[u8], mains_v: Option<f32>) -> Option<PsuReading> {
    if data.len() < layout.len() {
        return None;
    }
    let w = |at: usize| linear11(u16::from_le_bytes([data[at], data[at + 1]]), false);
    // (12 V, 5 V, 3.3 V) as (volts, amps) offsets, then output, efficiency,
    // temperature and fan.
    let (rails, rest) = match layout {
        MsiLayout::MegP => ([(20, 22), (24, 26), (28, 30)], 32),
        MsiLayout::MegT => ([(0, 2), (4, 6), (8, 10)], 12),
        MsiLayout::MpgTs => ([(38, 0), (40, 2), (42, 4)], 30),
    };
    let out_w = watts(w(rest))?;
    let eff = efficiency(w(rest + 2));
    let rails = [12.0, 5.0, 3.3].into_iter().zip(rails).filter_map(|(n, (v, a))| rail(n, w(v), w(a))).collect();
    Some(PsuReading {
        name: name.to_string(),
        out_w,
        in_w: eff.filter(|_| out_w > 0.0).map(|e| out_w / e),
        efficiency: eff,
        mains_v: mains_v.and_then(mains),
        temp_c: temp(w(rest + 4)),
        fan_rpm: rpm(w(rest + 6)),
        rails,
    })
}

/// MSI's input voltage register (the MEG models answer it).
pub fn msi_mains(data: &[u8]) -> Option<f32> {
    (data.len() >= 2).then(|| linear11(u16::from_le_bytes([data[0], data[1]]), false)).and_then(mains)
}

/// The estimate's allowance for what reports no power of its own: the
/// board's chipset and power stages, memory, drives, fans and USB devices.
/// A desktop's usually take 30 to 60 W together.
pub const REST_W: f32 = 40.0;

/// The whole PC's power.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PowerReading {
    /// What the PC draws from its power supply (W): the PSU's own reading,
    /// or the estimate.
    pub total_w: f32,
    /// What the CPU package and the graphics card report of their own (W).
    pub cpu_w: Option<f32>,
    pub gpu_w: Option<f32>,
    /// Everything else: the PSU's reading less the two, or [`REST_W`].
    pub rest_w: f32,
    /// The power supply behind the reading; None for an estimate.
    pub psu: Option<PsuReading>,
}

impl PowerReading {
    /// From the power supply, with what the CPU and graphics card say they use.
    pub fn from_psu(psu: PsuReading, cpu_w: Option<f32>, gpu_w: Option<f32>) -> PowerReading {
        let known = cpu_w.unwrap_or(0.0) + gpu_w.unwrap_or(0.0);
        PowerReading { total_w: psu.out_w, cpu_w, gpu_w, rest_w: (psu.out_w - known).max(0.0), psu: Some(psu) }
    }

    /// A desktop without a PSU that reports: the CPU package, the graphics
    /// card and [`REST_W`]. None when a graphics card of its own doesn't
    /// report its power, since the total would leave out what may be most
    /// of it; a GPU built into the processor is inside the package's reading.
    pub fn estimate(cpu_w: Option<f32>, gpu_w: Option<f32>, dedicated_gpu: bool) -> Option<PowerReading> {
        let cpu = cpu_w?;
        let gpu = if dedicated_gpu { Some(gpu_w?) } else { None };
        Some(PowerReading { total_w: cpu + gpu.unwrap_or(0.0) + REST_W, cpu_w: Some(cpu), gpu_w: gpu, rest_w: REST_W, psu: None })
    }

    pub fn estimated(&self) -> bool {
        self.psu.is_none()
    }
}

/// "73 W", "8.5 W".
pub fn fmt_w(w: f32) -> String {
    if w < 9.95 {
        format!("{w:.1} W")
    } else {
        format!("{w:.0} W")
    }
}

/// A round top for a power chart at or above `peak` watts.
pub fn watts_ceiling(peak: f32) -> f32 {
    const TOPS: [f32; 11] = [50.0, 100.0, 150.0, 200.0, 300.0, 400.0, 500.0, 750.0, 1000.0, 1500.0, 2000.0];
    let peak = if peak.is_finite() { peak } else { 0.0 };
    TOPS.into_iter().find(|t| peak <= *t).unwrap_or_else(|| (peak / 500.0).ceil() * 500.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(v: f32, exp: i32) -> u16 {
        let mant = (v / 2f32.powi(exp)).round() as i32;
        (((exp & 0x1f) as u16) << 11) | (mant as u16 & 0x7ff)
    }

    #[test]
    fn linear11_words() {
        // 12.0 V as 768 × 2^-6, 3.3 V as 422 × 2^-7, 400 W as 800 × 2^-1.
        assert_eq!(linear11(word(12.0, -6), true), 12.0);
        assert!((linear11(word(3.3, -7), true) - 3.297).abs() < 0.001);
        assert_eq!(linear11(word(400.0, -1), true), 400.0);
        // Exponents 0 and 1, and a mantissa past 1023 (MSI reads it unsigned).
        assert_eq!(linear11(0x0002, true), 2.0);
        assert_eq!(linear11(0x0802, false), 4.0);
        assert_eq!(linear11(0x0402, false), 1026.0);
        assert_eq!(linear11(0x0402, true), -1022.0);
    }

    #[test]
    fn usb_ids_from_hid_paths() {
        assert_eq!(usb_ids(r"\\?\hid#vid_1b1c&pid_1c1f#7&2f1f3a6&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}"), Some((0x1b1c, 0x1c1f)));
        assert_eq!(usb_ids(r"\\?\HID#VID_0DB0&PID_C9EB&MI_00#8&1a2b3c&0&0000#{4d1e55b2}"), Some((0x0db0, 0xc9eb)));
        assert_eq!(usb_ids(r"\\?\hid#vid_1b1c#nothing"), None);
        assert_eq!(model(0x0db0, 0xc9eb).map(|m| m.name), Some("MSI MEG Ai1600T"));
        assert_eq!(model(0x1b1c, 0x1c1f).map(|m| m.protocol), Some(Protocol::Corsair));
        // MSI's other devices (its RGB controllers share the vendor id) aren't PSUs.
        assert_eq!(model(0x0db0, 0x0076), None);
    }

    #[test]
    fn corsair_replies_must_answer_the_request() {
        let mut reply = [0u8; REPORT];
        reply[..2].copy_from_slice(&[0x03, corsair::TOTAL_OUT]);
        reply[2..4].copy_from_slice(&word(412.0, -1).to_le_bytes());
        assert_eq!(corsair_value(corsair::TOTAL_OUT, &reply), Some(412.0));
        // Another app's answer, and an unsupported command (0 in its place).
        assert_eq!(corsair_value(corsair::READ_VIN, &reply), None);
        reply[1] = 0;
        assert_eq!(corsair_value(corsair::TOTAL_OUT, &reply), None);
        assert_eq!(corsair_read(corsair::READ_FAN)[..3], [0x03, 0x90, 0]);
        assert_eq!(corsair_hello()[..2], [0xfe, 0x03]);
    }

    #[test]
    fn corsair_wall_power_from_the_input() {
        let r = corsair_reading("Corsair HX1500i", Some(412.0), Some(230.0), Some(1.95), Some(41.0), Some(0.0)).unwrap();
        assert_eq!(r.out_w, 412.0);
        assert!((r.in_w.unwrap() - 448.5).abs() < 0.01);
        assert!((r.efficiency.unwrap() - 0.9186).abs() < 0.001);
        assert_eq!(r.fan_rpm, Some(0));
        // No input current (most models): no wall reading, the rest stays.
        let r = corsair_reading("Corsair RM850i", Some(250.0), Some(118.0), None, None, Some(640.0)).unwrap();
        assert_eq!((r.in_w, r.efficiency, r.mains_v, r.fan_rpm), (None, None, Some(118.0), Some(640)));
        // No total, no reading; nonsense is dropped.
        assert_eq!(corsair_reading("x", None, Some(230.0), None, None, None), None);
        assert_eq!(corsair_reading("x", Some(9e9), None, None, None, None), None);
        let r = corsair_reading("x", Some(300.0), Some(12.0), Some(1.0), Some(900.0), None).unwrap();
        assert_eq!((r.mains_v, r.in_w, r.temp_c), (None, None, None));
    }

    /// A bulk read's data with each field at its offset.
    fn msi_data_for(layout: MsiLayout) -> Vec<u8> {
        let mut d = vec![0u8; 62];
        let mut put = |at: usize, v: f32, exp: i32| d[at..at + 2].copy_from_slice(&word(v, exp).to_le_bytes());
        let (rails, rest): ([(usize, usize); 3], usize) = match layout {
            MsiLayout::MegP => ([(20, 22), (24, 26), (28, 30)], 32),
            MsiLayout::MegT => ([(0, 2), (4, 6), (8, 10)], 12),
            MsiLayout::MpgTs => ([(38, 0), (40, 2), (42, 4)], 30),
        };
        for ((v, a), (volts, amps)) in rails.into_iter().zip([(12.06, 28.5), (5.02, 4.25), (3.31, 2.5)]) {
            put(v, volts, -7);
            put(a, amps, -4);
        }
        put(rest, 368.0, -1);
        put(rest + 2, 0.93, -10);
        put(rest + 4, 38.5, -2);
        put(rest + 6, 0.0, 0);
        d
    }

    #[test]
    fn msi_layouts() {
        for layout in [MsiLayout::MegP, MsiLayout::MegT, MsiLayout::MpgTs] {
            let d = msi_data_for(layout);
            let r = msi_reading("MSI MEG Ai1600T", layout, &d, Some(229.5)).unwrap();
            assert_eq!(r.out_w, 368.0, "{layout:?}");
            assert!((r.efficiency.unwrap() - 0.93).abs() < 0.001, "{layout:?}");
            assert!((r.in_w.unwrap() - 395.7).abs() < 0.2, "{layout:?}");
            assert_eq!((r.temp_c, r.fan_rpm, r.mains_v), (Some(38.5), Some(0), Some(229.5)), "{layout:?}");
            assert_eq!(r.rails.len(), 3, "{layout:?}");
            assert!((r.rails[0].volts - 12.06).abs() < 0.01 && (r.rails[0].amps - 28.5).abs() < 0.07, "{layout:?}");
            assert!((r.rails[2].watts() - 8.27).abs() < 0.1, "{layout:?}");
            // A frame shorter than the layout is never read from its padding.
            assert_eq!(msi_reading("x", layout, &d[..layout.len() - 1], None), None, "{layout:?}");
        }
        // Efficiency in percent reads the same as a share.
        let mut d = msi_data_for(MsiLayout::MegT);
        d[14..16].copy_from_slice(&word(93.0, 0).to_le_bytes());
        assert!((msi_reading("x", MsiLayout::MegT, &d, None).unwrap().efficiency.unwrap() - 0.93).abs() < 0.001);
    }

    #[test]
    fn msi_handshake_and_replies() {
        let mut reply = [0u8; REPORT];
        reply[..2].copy_from_slice(&[0xfa, 0x51]);
        reply[2..14].copy_from_slice(b"MPG Ai1600TS");
        assert_eq!(msi_name(&reply).as_deref(), Some("MPG Ai1600TS"));
        assert_eq!(MsiLayout::from_name("MPG Ai1600TS"), Some(MsiLayout::MpgTs));
        assert_eq!(MsiLayout::from_name("MEG Ai1600T PCIE5"), Some(MsiLayout::MegT));
        assert_eq!(MsiLayout::from_name("MEG Ai1300P PCIE5"), Some(MsiLayout::MegP));
        assert_eq!(MsiLayout::from_name("Something else"), None);
        assert_eq!(msi_name(&msi_read(msi::ALL)), None);
        // A read's data, and the refusal of an unsupported register.
        let mut reply = [0u8; REPORT];
        reply[..4].copy_from_slice(&[0x51, msi::VIN, 0, 0]);
        reply[2..4].copy_from_slice(&word(230.0, 0).to_le_bytes());
        assert_eq!(msi_data(msi::VIN, &reply).and_then(msi_mains), Some(230.0));
        assert_eq!(msi_data(msi::VIN, &[0x51, 0xfe, 0, 0]), None);
        assert_eq!(msi_data(msi::ALL, &reply), None);
    }

    #[test]
    fn whole_pc_from_the_psu_or_an_estimate() {
        let psu = PsuReading { name: "Corsair HX1500i".into(), out_w: 412.0, ..PsuReading::default() };
        let p = PowerReading::from_psu(psu, Some(73.0), Some(291.0));
        assert_eq!((p.total_w, p.rest_w, p.estimated()), (412.0, 48.0, false));
        // The parts may add up past a PSU's slower average for a moment.
        let psu = PsuReading { out_w: 100.0, ..PsuReading::default() };
        assert_eq!(PowerReading::from_psu(psu, Some(73.0), Some(91.0)).rest_w, 0.0);
        // A desktop with a graphics card that reports its power.
        let p = PowerReading::estimate(Some(73.0), Some(91.0), true).unwrap();
        assert_eq!((p.total_w, p.rest_w, p.estimated()), (204.0, REST_W, true));
        // A card that doesn't: no total. A GPU in the processor: the package has it.
        assert_eq!(PowerReading::estimate(Some(73.0), None, true), None);
        let p = PowerReading::estimate(Some(35.0), Some(9.0), false).unwrap();
        assert_eq!((p.total_w, p.gpu_w), (75.0, None));
        assert_eq!(PowerReading::estimate(None, Some(91.0), true), None);
    }

    #[test]
    fn formats_and_chart_tops() {
        assert_eq!(fmt_w(73.4), "73 W");
        assert_eq!(fmt_w(8.46), "8.5 W");
        assert_eq!(watts_ceiling(91.0), 100.0);
        assert_eq!(watts_ceiling(412.0), 500.0);
        assert_eq!(watts_ceiling(1530.0), 2000.0);
        assert_eq!(watts_ceiling(2400.0), 2500.0);
        assert_eq!(watts_ceiling(f32::NAN), 50.0);
    }
}
