// Fosi Audio DSP wire protocol — platform-free. The DS3 so far.
//
// Feature report 1, 63 bytes, headed 0x77 and a command. Like FiiO, the device designs its own
// filters from freq / Q / gain / type, here as little-endian float32s:
//
//     byte 0   77
//     byte 1   CMD
//     byte 2   preset (bank)
//     byte 3   band index
//     byte 4   filter type
//     byte 5.. freq, Q, bandwidth, gain — f32 LE each
//
// The DS3 holds five custom banks (7-11) and a bypass slot (0). Band parameters are written
// with a feature report and each one committed with an output report carrying 0x8E; reads ask
// with 0x8E as a feature report and fetch the answer with GET_REPORT.
//
// devicePEQ (github.com/jeromeof/devicePEQ, fosiAudioUsbHidHandler.js) is the source for all
// of this, including two measured corrections applied below; none of it has been checked on
// hardware in this app yet.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::thread::sleep;
use std::time::Duration;

use serde::Serialize;

use crate::dsp::{Band, BandIn, WriteResult};

pub const VID: u16 = 0x152A;
pub const REPORT_ID: u8 = 1;

const HEADER: u8 = 0x77;
const SET_EQ_MODE: u8 = 0x8A;
const SET_EQ_PARAMS: u8 = 0x8D;
const GET_EQ_PARAMS: u8 = 0x8E;
const GET_EQ_MODE_COUNT: u8 = 0x91;
const SET_AND_SAVE_EQ_MODE: u8 = 0x92;
const SET_EQ_ENABLE: u8 = 0x9D;
const GET_EQ_ENABLE: u8 = 0x9E;

// The rate the DS3's DSP runs at, for the shelf correction's prewarp.
const DSP_FS: f64 = 48_000.0;

/// One Fosi device, opened.
pub trait TransportFosi {
    /// SET_REPORT(Feature).
    fn send_feature(&self, payload: &[u8]) -> bool;
    /// GET_REPORT(Feature), report id stripped.
    fn get_feature(&self) -> Option<Vec<u8>>;
    /// An output report, or a feature report on a device that declares no output report.
    fn send_output(&self, payload: &[u8]) -> bool;
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
    pub presets: BTreeMap<u8, String>,
    pub user_slots: Vec<u8>,
}

/// Every model this file knows, for offline design.
pub fn all_profiles() -> Vec<Profile> {
    profile("Fosi Audio DS3").into_iter().collect()
}

/// Only the DS3, by name: Topping shares the vendor id and speaks something else.
pub fn profile(product: &str) -> Option<Profile> {
    if !product.trim().eq_ignore_ascii_case("Fosi Audio DS3") {
        return None;
    }
    let user_slots: Vec<u8> = (7..=11).collect();
    let mut presets: BTreeMap<u8, String> = user_slots.iter().map(|s| (*s, format!("Custom {}", s - 6))).collect();
    presets.insert(0, "Bypass".into());
    Some(Profile {
        name: product.trim().to_string(),
        bands: 8,
        gain_min: -12.0,
        gain_max: 12.0,
        q_min: 0.1,
        q_max: 10.0,
        types: ["PK", "LSQ", "HSQ", "LP", "HP"].iter().map(|s| s.to_string()).collect(),
        presets,
        user_slots,
    })
}

// The bank the app last switched to. The DS3 has no read for it that devicePEQ trusts, so the
// app keeps it, starting from Custom 1.
static ACTIVE: AtomicU8 = AtomicU8::new(7);

fn packet(cmd: u8, preset: u8, band: u8) -> [u8; 63] {
    let mut p = [0u8; 63];
    p[0] = HEADER;
    p[1] = cmd;
    p[2] = preset;
    p[3] = band;
    p
}

fn type_code(t: &str) -> u8 {
    match t {
        "LSQ" => 9,
        "HSQ" => 10,
        "LP" => 3,
        "HP" => 4,
        _ => 2,
    }
}

fn type_name(c: u8) -> &'static str {
    match c {
        9 => "LSQ",
        10 => "HSQ",
        3 => "LP",
        4 => "HP",
        _ => "PK",
    }
}

// ---- the DS3's two filter-design conventions, as devicePEQ measured them ----
//
// Peaking bands come out at Q / A, A = 10^(|gain|/40): 0.714 at 6 dB and 0.537 at 12 dB
// against predictions of 0.708 and 0.501. So peaks are sent Q x A.
//
// Shelves use the other corner-frequency convention: realised = asked x sqrt(A) for a low
// shelf and / sqrt(A) for a high one, applied to the analog prototype, so near Nyquist it is
// done through the bilinear prewarp (a 10 kHz shelf measured x0.769, predicted x0.760).

fn rbj_a(gain: f64) -> f64 {
    10f64.powf(gain.abs() / 40.0)
}

fn shelf_dir(gain: f64, typ: &str) -> Option<f64> {
    let m = rbj_a(gain).sqrt();
    match typ {
        "LSQ" => Some(m),
        "HSQ" => Some(1.0 / m),
        _ => None,
    }
}

fn warp(f: f64) -> f64 {
    (std::f64::consts::PI * f / DSP_FS).tan()
}

fn unwarp(t: f64) -> f64 {
    DSP_FS / std::f64::consts::PI * t.atan()
}

/// (freq, q) to send for the band the user asked for.
fn to_device(b: &BandIn, p: &Profile) -> (f64, f64) {
    let freq = match shelf_dir(b.gain, &b.typ) {
        Some(d) if b.freq < DSP_FS / 2.0 => unwarp(warp(b.freq) / d),
        Some(d) => b.freq / d,
        None => b.freq,
    };
    let q = if b.typ == "PK" { (b.q * rbj_a(b.gain)).clamp(p.q_min, p.q_max) } else { b.q };
    (freq, q)
}

/// (freq, q) the listener hears, from what the device stores.
fn from_device(freq: f64, q: f64, gain: f64, typ: &str) -> (f64, f64) {
    let f = match shelf_dir(gain, typ) {
        Some(d) if freq < DSP_FS / 2.0 => unwarp(warp(freq) * d),
        Some(d) => freq * d,
        None => freq,
    };
    let q = if typ == "PK" { q / rbj_a(gain) } else { q };
    (f, q)
}

fn f32_at(r: &[u8], i: usize) -> f64 {
    f32::from_le_bytes([r[i], r[i + 1], r[i + 2], r[i + 3]]) as f64
}

fn band_packet(slot: u8, i: u8, b: &BandIn, p: &Profile) -> [u8; 63] {
    let (freq, q) = to_device(b, p);
    let mut pk = packet(SET_EQ_PARAMS, slot, i);
    pk[4] = type_code(&b.typ);
    pk[5..9].copy_from_slice(&(freq as f32).to_le_bytes());
    pk[9..13].copy_from_slice(&(q as f32).to_le_bytes());
    pk[13..17].copy_from_slice(&0f32.to_le_bytes());
    pk[17..21].copy_from_slice(&(b.gain.clamp(p.gain_min, p.gain_max) as f32).to_le_bytes());
    pk
}

// ---- reads ----

/// Whether the EQ is on. Doubles as the handshake.
pub fn read_enabled<T: TransportFosi + ?Sized>(dev: &T) -> Option<bool> {
    if !dev.send_output(&packet(GET_EQ_ENABLE, 0, 0)) {
        return None;
    }
    let r = dev.get_feature()?;
    (r.len() >= 3 && r[0] == HEADER && r[1] == GET_EQ_ENABLE).then(|| r[2] == 1)
}

pub fn active() -> u8 {
    ACTIVE.load(Ordering::Relaxed)
}

type RawBand = (f64, f64, f64, &'static str); // freq, gain, q, type — as heard

fn read_band<T: TransportFosi + ?Sized>(dev: &T, slot: u8, i: u8) -> Option<RawBand> {
    if !dev.send_feature(&packet(GET_EQ_PARAMS, slot, i)) {
        return None;
    }
    let r = dev.get_feature()?;
    if r.len() < 21 || r[0] != HEADER || r[3] != i {
        return None;
    }
    let typ = type_name(r[4]);
    let gain = f32_at(&r, 17);
    let (freq, q) = from_device(f32_at(&r, 5), f32_at(&r, 9), gain, typ);
    Some((freq, gain, q, typ))
}

pub fn read_bank<T: TransportFosi + ?Sized>(dev: &T, p: &Profile) -> Vec<Band> {
    let slot = active();
    dev.send_feature(&packet(SET_EQ_MODE, slot, 0));
    sleep(Duration::from_millis(30));
    (0..p.bands as u8)
        .map(|i| {
            let b = read_band(dev, slot, i);
            sleep(Duration::from_millis(20));
            match b {
                Some((freq, gain, q, typ)) => Band {
                    ok: true,
                    band: i + 1,
                    freq: freq.round() as i32,
                    gain: (gain * 10.0).round() / 10.0,
                    q: (q * 100.0).round() / 100.0,
                    typ: typ.into(),
                },
                None => Band { ok: false, band: i + 1, freq: 0, gain: 0.0, q: 0.0, typ: "PK".into() },
            }
        })
        .collect()
}

// ---- writes ----

pub fn set_preset<T: TransportFosi + ?Sized>(dev: &T, p: &Profile, slot: u8) -> Result<Option<i32>, String> {
    if !p.presets.contains_key(&slot) {
        return Err(format!("The {} has no bank {}.", p.name, slot));
    }
    if !dev.send_feature(&packet(SET_EQ_MODE, slot, 0)) {
        return Err(format!("{} is no longer connected.", p.name));
    }
    ACTIVE.store(slot, Ordering::Relaxed);
    sleep(Duration::from_millis(50));
    Ok(Some(slot as i32))
}

/// Writes the active custom bank, saves it, switches the EQ on and reads the bands back.
/// The DS3 works out its own headroom, so there is no preamp to send.
pub fn write_bank<T: TransportFosi + ?Sized>(dev: &T, p: &Profile, bands: &[BandIn]) -> Result<WriteResult, String> {
    let slot = active();
    if !p.user_slots.contains(&slot) {
        return Err(format!("The {} is on Bypass, which can't be written. Pick a Custom bank first. Nothing was written.", p.name));
    }
    dev.send_feature(&packet(GET_EQ_MODE_COUNT, 0, 0));
    sleep(Duration::from_millis(50));
    dev.send_feature(&packet(SET_EQ_MODE, slot, 0));
    sleep(Duration::from_millis(30));
    let bands: Vec<&BandIn> = bands.iter().take(p.bands).collect();
    for (i, b) in bands.iter().enumerate() {
        if !dev.send_feature(&band_packet(slot, i as u8, b, p)) {
            return Err(format!("{} stopped answering at band {}.", p.name, i + 1));
        }
        sleep(Duration::from_millis(20));
        dev.send_output(&packet(GET_EQ_PARAMS, slot, i as u8));
        sleep(Duration::from_millis(20));
    }
    dev.send_feature(&packet(SET_AND_SAVE_EQ_MODE, slot, 0));
    sleep(Duration::from_millis(50));
    dev.send_output(&packet(SET_EQ_ENABLE, 1, 0));
    dev.get_feature();
    sleep(Duration::from_millis(30));

    let acked = bands
        .iter()
        .enumerate()
        .filter(|(i, want)| {
            read_band(dev, slot, *i as u8).is_some_and(|(f, g, q, t)| {
                (f - want.freq).abs() <= want.freq * 0.002 + 0.5
                    && (g - want.gain.clamp(p.gain_min, p.gain_max)).abs() < 0.05
                    && (t != "PK" || (q - want.q).abs() < 0.01 * want.q.max(1.0))
                    && t == type_name(type_code(&want.typ))
            })
        })
        .count() as u32;
    Ok(WriteResult { acked, active: Some(slot as i32) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_ds3_is_claimed() {
        assert!(profile("Fosi Audio DS3").is_some());
        assert!(profile("DX1 II").is_none());
    }

    // A band sent to the device and read back must come out as the band asked for, or a read
    // then write would drift.
    #[test]
    fn corrections_round_trip() {
        let p = profile("Fosi Audio DS3").unwrap();
        for (typ, freq, gain, q) in [("PK", 1000.0, 6.0, 1.0), ("LSQ", 105.0, 5.5, 0.7), ("HSQ", 10000.0, -6.0, 0.7)] {
            let b = BandIn { freq, gain, q, typ: typ.into() };
            let (f, qs) = to_device(&b, &p);
            let (f2, q2) = from_device(f, qs, gain, typ);
            assert!((f2 - freq).abs() < 1e-6 && (q2 - q).abs() < 1e-9, "{typ}");
        }
        // The measured direction: a +6 dB peak is sent about 1.41x the Q.
        let (_, q) = to_device(&BandIn { freq: 1000.0, gain: 6.0, q: 1.0, typ: "PK".into() }, &p);
        assert!((q - 1.4125).abs() < 0.001);
    }

    #[test]
    fn band_packet_layout() {
        let p = profile("Fosi Audio DS3").unwrap();
        let pk = band_packet(7, 2, &BandIn { freq: 1000.0, gain: 0.0, q: 2.0, typ: "PK".into() }, &p);
        assert_eq!(&pk[..5], &[0x77, SET_EQ_PARAMS, 7, 2, 2]);
        assert_eq!(f32_at(&pk, 5), 1000.0);
        assert_eq!(f32_at(&pk, 9), 2.0);
    }
}
