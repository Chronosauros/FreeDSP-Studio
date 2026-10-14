// KT Micro DSP wire protocol — platform-free.
//
// KT Micro's KT02/KT0211/KT1132/KT3016 chips sit in Moondrop's CDSP and Chu II DSP, Tanchjim's
// One, Bunny and Fission, Kiwi Ears' Allegro, Chorus and S-Link, JCally's JM12 and others. The
// device is a register file: each band is two registers, one holding gain and frequency and
// the next Q and filter type, and the host reads and writes them one at a time.
//
// Wire format — HID report 0x4B (the same id Walkplay uses, which is why these are told apart
// by vendor id 0x31B2 and never sent the Walkplay handshake), 10 bytes each way:
//
//     byte 0   register
//     byte 4   52 'R' read / 57 'W' write / 53 'S' commit
//     byte 6.. value — gain (i16 LE, 0.1 dB) + freq (u16 LE), or Q (u16 LE, /1000) + type
//
// Reads are answered with the register and command echoed. Writes are not acknowledged on
// every model (the Tanchjim One processes them silently), so success is judged by reading the
// bands back after the commit. Nothing here sends 0x43 ('C'), which devicePEQ names "clear".
//
// The protocol and every model below come from devicePEQ (github.com/jeromeof/devicePEQ,
// ktmicroUsbHidHandler.js); none of it has been checked on hardware in this app yet.

use std::thread::sleep;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::dsp::{Band, BandIn, WriteResult};

pub const VID: u16 = 0x31B2;
pub const REPORT_ID: u8 = 0x4B;

const READ: u8 = 0x52;
const WRITE: u8 = 0x57;
const COMMIT: u8 = 0x53;

const REG_EQ: u8 = 0x24;
const REG_PREGAIN: u8 = 0x66;
const EQ_CUSTOM: u8 = 0x03;
const EQ_OFF: u8 = 0x02;

const REPLY_TIMEOUT_MS: u64 = 300;
const DELAY_WRITE_MS: u64 = 20;
// devicePEQ waits a second after the commit before anything else touches the device.
const DELAY_COMMIT_MS: u64 = 1000;

/// One KT Micro device, opened: interrupt OUT and IN on report 0x4B.
pub trait TransportKt {
    fn send(&self, frame: &[u8]) -> bool;
    fn recv(&self, timeout_ms: u64) -> Option<Vec<u8>>;
    fn drain(&self);
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub name: String,
    pub bands: usize,
    pub gain_min: f64,
    pub gain_max: f64,
    pub q_min: f64,
    pub q_max: f64,
    pub types: Vec<String>,
    /// (gain/freq register, Q/type register) per band.
    pub registers: Vec<(u8, u8)>,
    /// Older KT Micro parts run their filters at twice the frequency they are given, so they
    /// are sent half of it and read back as double.
    pub halve_freq: bool,
    /// "global" if the host writes a whole-dB pregain, "none" if the device works out its own.
    pub preamp: String,
}

fn consecutive(base: u8, n: usize) -> Vec<(u8, u8)> {
    (0..n as u8).map(|i| (base + 2 * i, base + 2 * i + 1)).collect()
}

pub fn profile(pid: u16, product: &str) -> Profile {
    let mut p = Profile {
        name: if product.is_empty() { format!("KT Micro {:04x}", pid) } else { product.to_string() },
        bands: 5,
        gain_min: -10.0,
        gain_max: 10.0,
        q_min: 0.1,
        q_max: 5.0,
        types: vec!["PK".into(), "LSQ".into(), "HSQ".into()],
        registers: consecutive(0x26, 5),
        // devicePEQ's default for a KT Micro part it doesn't know. Every named model below
        // turns it off.
        halve_freq: true,
        preamp: "none".into(),
    };
    match pid {
        // Kiwi Ears Chorus family (KT1132L). Band 5's Q sits at 0x3F: 0x3E is a phantom that
        // always reads 0.7.
        0x1132 | 0x3006 => {
            p.registers = vec![(0x35, 0x36), (0x37, 0x38), (0x39, 0x3A), (0x3B, 0x3C), (0x3D, 0x3F)];
            p.halve_freq = false;
            return p;
        }
        // Kiwi Ears S-Link (KT3016L): bands stored out of order.
        0x3016 => {
            p.registers = vec![(0x37, 0x38), (0x3B, 0x3C), (0x3D, 0x3F), (0x39, 0x3A), (0x35, 0x36)];
            p.halve_freq = false;
            return p;
        }
        0x0113 => {
            p.halve_freq = false;
            return p;
        }
        _ => {}
    }
    let n = product.trim().to_lowercase();
    let named = |s: &str| n == s.to_lowercase();
    if ["Kiwi Ears-Allegro PRO", "Kiwi Ears Allegro Mini", "Kiwi Ears-Allegro Mini", "TANCHJIM-ONE DSP", "CDSP", "Chu2 DSP"]
        .iter()
        .any(|s| named(s))
    {
        p.halve_freq = false;
    } else if ["TANCHJIM BUNNY DSP", "TANCHJIM FISSION", "TANCHJIM-FISSION  DSP"].iter().any(|s| named(s)) {
        p.halve_freq = false;
        p.preamp = "global".into();
    } else if named("KT02H20 HIFI Audio") {
        // JCally JM12: peaking only.
        p.types = vec!["PK".into()];
    }
    p
}

// ---- frames ----

fn packet(reg: u8, cmd: u8, value: [u8; 4]) -> [u8; 10] {
    [reg, 0, 0, 0, cmd, 0, value[0], value[1], value[2], value[3]]
}

fn type_code(t: &str) -> u8 {
    match t {
        "LSQ" => 3,
        "HSQ" => 4,
        _ => 0,
    }
}

fn type_name(c: u8) -> &'static str {
    match c {
        3 => "LSQ",
        4 => "HSQ",
        _ => "PK",
    }
}

fn read_reg<T: TransportKt + ?Sized>(dev: &T, reg: u8, arg: u8) -> Option<[u8; 4]> {
    for _ in 0..2 {
        dev.drain();
        if !dev.send(&packet(reg, READ, [arg, 0, 0, 0])) {
            continue;
        }
        let deadline = Instant::now() + Duration::from_millis(REPLY_TIMEOUT_MS);
        while Instant::now() < deadline {
            let left = deadline.saturating_duration_since(Instant::now()).as_millis() as u64;
            let Some(r) = dev.recv(left.max(1)) else { break };
            if r.len() >= 10 && r[0] == reg && r[4] == READ {
                return Some([r[6], r[7], r[8], r[9]]);
            }
        }
    }
    None
}

// ---- reads ----

/// The EQ state: 3 for the custom EQ, 2 for off. Doubles as the handshake.
pub fn read_eq<T: TransportKt + ?Sized>(dev: &T) -> Option<u8> {
    // devicePEQ sends the read with 3 in the value byte; kept, since it is what devices answer.
    read_reg(dev, REG_EQ, EQ_CUSTOM).map(|v| v[0])
}

pub fn read_preamp<T: TransportKt + ?Sized>(dev: &T, p: &Profile) -> Option<f64> {
    if p.preamp != "global" {
        return None;
    }
    read_reg(dev, REG_PREGAIN, 0).map(|v| v[0] as i8 as f64)
}

type RawBand = (f64, f64, f64, &'static str); // freq, gain, q, type

fn read_band<T: TransportKt + ?Sized>(dev: &T, p: &Profile, i: usize) -> Option<RawBand> {
    let (gf, qt) = p.registers[i];
    let a = read_reg(dev, gf, 0)?;
    let b = read_reg(dev, qt, 0)?;
    let gain = i16::from_le_bytes([a[0], a[1]]) as f64 / 10.0;
    let raw = u16::from_le_bytes([a[2], a[3]]) as f64;
    let freq = if p.halve_freq { raw * 2.0 } else { raw };
    let q = u16::from_le_bytes([b[0], b[1]]) as f64 / 1000.0;
    Some((freq, gain, q, type_name(b[2])))
}

pub fn read_bank<T: TransportKt + ?Sized>(dev: &T, p: &Profile) -> Vec<Band> {
    (0..p.bands)
        .map(|i| match read_band(dev, p, i) {
            Some((freq, gain, q, typ)) => Band { ok: true, band: (i + 1) as u8, freq: freq.round() as i32, gain, q, typ: typ.into() },
            None => Band { ok: false, band: (i + 1) as u8, freq: 0, gain: 0.0, q: 0.0, typ: "PK".into() },
        })
        .collect()
}

// ---- writes ----

fn band_packets(p: &Profile, i: usize, b: &BandIn) -> ([u8; 10], [u8; 10]) {
    let (gf, qt) = p.registers[i];
    let freq = if p.halve_freq { b.freq / 2.0 } else { b.freq };
    let [fl, fh] = (freq.round().clamp(10.0, 24000.0) as u16).to_le_bytes();
    let [gl, gh] = ((b.gain.clamp(p.gain_min, p.gain_max) * 10.0).round() as i16).to_le_bytes();
    let [ql, qh] = ((b.q.clamp(p.q_min, p.q_max) * 1000.0).round() as u16).to_le_bytes();
    (packet(gf, WRITE, [gl, gh, fl, fh]), packet(qt, WRITE, [ql, qh, type_code(&b.typ), 0]))
}

fn matches(got: &RawBand, want: &BandIn, p: &Profile) -> bool {
    // Frequency survives as a whole number of Hz (of half-Hz on a halving part).
    let step = if p.halve_freq { 2.0 } else { 1.0 };
    (got.0 - want.freq.round()).abs() <= step
        && (got.1 - want.gain.clamp(p.gain_min, p.gain_max)).abs() < 0.05
        && (got.2 - want.q.clamp(p.q_min, p.q_max)).abs() < 0.0015
        && got.3 == type_name(type_code(&want.typ))
}

/// Writes every band, commits, then reads the bands back; `acked` counts the ones that match.
pub fn write_bank<T: TransportKt + ?Sized>(dev: &T, p: &Profile, bands: &[BandIn], preamp: f64) -> Result<WriteResult, String> {
    let state = read_eq(dev).ok_or_else(|| format!("{} is no longer connected. Nothing was written.", p.name))?;
    // A part with its EQ switched off is switched to the custom EQ, as devicePEQ does.
    if state == EQ_OFF {
        dev.send(&packet(REG_EQ, WRITE, [EQ_CUSTOM, 0, 0, 0]));
        sleep(Duration::from_millis(DELAY_WRITE_MS * 5));
    }
    let bands: Vec<&BandIn> = bands.iter().take(p.bands).collect();
    for (i, b) in bands.iter().enumerate() {
        let (a, q) = band_packets(p, i, b);
        dev.send(&a);
        sleep(Duration::from_millis(DELAY_WRITE_MS));
        dev.send(&q);
        sleep(Duration::from_millis(DELAY_WRITE_MS));
    }
    if p.preamp == "global" {
        let v = preamp.floor().clamp(-128.0, 0.0) as i8;
        dev.send(&packet(REG_PREGAIN, WRITE, [v as u8, 0, 0, 0]));
        sleep(Duration::from_millis(DELAY_WRITE_MS));
    }
    dev.send(&packet(0, COMMIT, [0; 4]));
    sleep(Duration::from_millis(DELAY_COMMIT_MS));

    let acked = bands
        .iter()
        .enumerate()
        .filter(|(i, want)| read_band(dev, p, *i).is_some_and(|got| matches(&got, want, p)))
        .count() as u32;
    Ok(WriteResult { acked, active: read_eq(dev).map(|s| s as i32) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::{HashMap, VecDeque};

    // A register file that answers reads and takes writes, like a KT0211.
    struct Regs {
        regs: RefCell<HashMap<u8, [u8; 4]>>,
        queued: RefCell<VecDeque<Vec<u8>>>,
        sent: RefCell<Vec<Vec<u8>>>,
    }
    impl TransportKt for Regs {
        fn send(&self, f: &[u8]) -> bool {
            self.sent.borrow_mut().push(f.to_vec());
            match f[4] {
                READ => {
                    let v = self.regs.borrow().get(&f[0]).copied().unwrap_or([0; 4]);
                    let mut r = f[..10].to_vec();
                    r[6..10].copy_from_slice(&v);
                    self.queued.borrow_mut().push_back(r);
                }
                WRITE => {
                    self.regs.borrow_mut().insert(f[0], [f[6], f[7], f[8], f[9]]);
                }
                _ => {}
            }
            true
        }
        fn recv(&self, _: u64) -> Option<Vec<u8>> { self.queued.borrow_mut().pop_front() }
        fn drain(&self) {}
    }

    fn regs() -> Regs {
        let mut m = HashMap::new();
        m.insert(REG_EQ, [EQ_CUSTOM, 0, 0, 0]);
        Regs { regs: RefCell::new(m), queued: RefCell::new(VecDeque::new()), sent: RefCell::new(vec![]) }
    }

    #[test]
    fn writes_land_in_the_right_registers_and_read_back() {
        let p = profile(0x0111, "Kiwi Ears-Allegro Mini");
        assert!(!p.halve_freq);
        let dev = regs();
        let bands: Vec<BandIn> = (0..5)
            .map(|i| BandIn { freq: 100.0 * (i + 1) as f64, gain: -2.5 + i as f64, q: 1.234, typ: if i == 4 { "HSQ".into() } else { "PK".into() } })
            .collect();
        let w = write_bank(&dev, &p, &bands, -3.0).unwrap();
        assert_eq!(w.acked, 5);
        // Band 1: gain -2.5 dB = -25 = 0xFFE7 LE, 100 Hz = 0x0064 LE, at 0x26; Q 1.234 at 0x27.
        assert_eq!(dev.regs.borrow()[&0x26], [0xE7, 0xFF, 0x64, 0x00]);
        assert_eq!(dev.regs.borrow()[&0x27], [0xD2, 0x04, 0, 0]);
        assert_eq!(dev.regs.borrow()[&0x2F][2], 4, "band 5 is a high shelf");
        // No pregain on a part that handles its own, and nothing but R/W/S on the wire.
        assert!(!dev.regs.borrow().contains_key(&REG_PREGAIN));
        assert!(dev.sent.borrow().iter().all(|f| [READ, WRITE, COMMIT].contains(&f[4])));
    }

    #[test]
    fn s_link_bands_go_to_their_scrambled_registers() {
        let p = profile(0x3016, "Kiwi Ears");
        let (a, q) = band_packets(&p, 2, &BandIn { freq: 1000.0, gain: 1.0, q: 1.0, typ: "PK".into() });
        assert_eq!((a[0], q[0]), (0x3D, 0x3F));
    }

    #[test]
    fn halving_parts_are_sent_half_the_frequency() {
        let p = profile(0x0042, "KT02H20 HIFI Audio");
        assert!(p.halve_freq && p.types == vec!["PK".to_string()]);
        let (a, _) = band_packets(&p, 0, &BandIn { freq: 200.0, gain: 0.0, q: 1.0, typ: "PK".into() });
        assert_eq!(u16::from_le_bytes([a[8], a[9]]), 100);
    }
}
