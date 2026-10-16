// FiiO DSP wire protocol — platform-free.
//
// FiiO's dongles, amps and Bluetooth receivers speak a protocol of their own, shared with
// JadeAudio and SnowSky parts and unrelated to either Moondrop generation. Unlike both of
// those, the device does its own filter maths: the host sends freq / gain / Q / type and the
// DSP designs the biquad. So there are no coefficients here, and gain is a real 0.1 dB value
// rather than a label sitting beside them.
//
// Wire format — one HID report each way on the interrupt endpoints. Report 7 on most models
// (1 on the KA17, 2 on JadeAudio parts), 32 bytes on the BTR17 and 63 on the KA15; both come
// from the device's descriptor, see hid_report.rs:
//
//     byte 0   AA (set) / BB (get)
//     byte 1   0A (set) / 0B (get)
//     byte 2-3 sequence number, big-endian
//     byte 4   CMD
//     byte 5   LEN — payload length
//     byte 6.. payload, then a CRC-8 over everything before it, then EE
//
// Replies come back as input reports headed BB 0B with the CMD echoed at byte 4. The BTR17's
// don't echo the sequence number and carry a zero checksum (the KA15's do both), so the only
// thing to pair a reply with its request is CMD, plus the band index for band reads.
//
// The device holds a set of EQ banks: factory voicings, one to ten USER banks and usually an
// EQ-off slot. Band writes always land on whichever bank is active, so writing a given bank
// means selecting it first. The timing rules below were measured on a real BTR17 rather than
// taken on trust from FiiO's web app: see `set_preset`.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU16, Ordering};
use std::thread::sleep;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::dsp::{Band, BandIn, WriteResult};

/// FiiO's two USB vendor ids.
pub const VIDS: [u16; 2] = [0x2972, 0x0A12];
/// Report ids FiiO parts have been seen on, most common first.
pub const REPORT_IDS: [u8; 3] = [7, 1, 2];

const SET: [u8; 2] = [0xAA, 0x0A];
const GET: [u8; 2] = [0xBB, 0x0B];
const END: u8 = 0xEE;

const CMD_BAND: u8 = 0x15;
const CMD_PRESET: u8 = 0x16;
const CMD_GLOBAL_GAIN: u8 = 0x17;
const CMD_SAVE: u8 = 0x19;
const CMD_BAND_COUNT: u8 = 0x18;

// The only commands this file will ever send as a *set*. FiiO's own app names 0x21
// RESET_TO_FACTORY, and at least one third-party tool sends 0x21 as "save" on this very
// device, so the whitelist is the guarantee rather than care at each call site.
const SETTABLE: [u8; 4] = [CMD_BAND, CMD_PRESET, CMD_GLOBAL_GAIN, CMD_SAVE];

// Timing, measured on a BTR17. Replies arrive in ~4 ms, so the timeout is generous headroom.
const REPLY_TIMEOUT_MS: u64 = 250;
// The BTR17 saves each band write to flash by itself, but a write that arrives less than about
// 50 ms after the previous one only reaches the working copy: it plays, reads back fine, and is
// gone at the next power-off. At 20 ms bands 5 and 9 were lost on every write; 50 ms and up lost
// nothing in testing. 150 ms is three times that.
const DELAY_BAND_MS: u64 = 150;
const DELAY_SAVE_MS: u64 = 300;
// After a bank switch the active-bank register updates within ~35 ms, but band reads keep
// returning the *previous* bank's data for 0.35–0.7 s. FiiO's web app reads straight after
// switching and shows the wrong EQ. This waits past the measured worst case; `read_bank`'s
// read-twice check then catches anything slower.
const DELAY_SETTLE_MS: u64 = 900;
const SWITCH_TIMEOUT_MS: u64 = 1500;

/// One FiiO device, opened. Interrupt OUT and IN, like gen-2, so the read is filtered and timed
/// out by the protocol layer rather than the transport.
pub trait TransportFiio {
    /// Sends one frame (report id excluded; the transport adds it and pads the report).
    fn send(&self, frame: &[u8]) -> bool;
    /// Waits up to `timeout_ms` for one input report on the control report, id stripped.
    fn recv(&self, timeout_ms: u64) -> Option<Vec<u8>>;
    /// Throws away input reports still queued from an earlier request.
    fn drain(&self);
}

// ---- device profiles ----
//
// One row per model FiiO's own web app (fiiocontrol.fiio.com) drives over HID, with its banks,
// gain and Q range, filter types and pacing taken from that app's tables. FiiO's app picks the
// row by USB product name, not product id, and so does this.
//
// Only the BTR17 and the KA15 have been checked on hardware here. The rest are FiiO's numbers,
// driven through the same paced write and saved-copy check, and say so in `tested`.

/// What a given FiiO model can do.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub name: String,
    /// FiiO's product name for the model, which is what the row was matched on.
    pub model: String,
    /// Bands, as the device reports them at open (FiiO's tables don't carry a count).
    pub bands: usize,
    pub gain_min: f64,
    pub gain_max: f64,
    pub q_min: f64,
    pub q_max: f64,
    pub types: Vec<String>,
    /// Every bank the device has, keyed by bank number.
    pub presets: BTreeMap<u8, String>,
    /// The banks that can be written. Factory voicings and bypass can't.
    pub user_slots: Vec<u8>,
    /// The report FiiO's app talks on. The device's descriptor has the final say.
    pub report_id: u8,
    /// Gap between band writes, ms. FiiO's app queues commands 100, 200 or 300 ms apart by
    /// model; the BTR17 needs at least 50 ms for a band to reach flash, and 150 is the floor.
    pub pace_ms: u64,
    /// Whether the device realises a peaking band's Q divided by the RBJ amplitude A at that
    /// gain, as devicePEQ measured on the QX13 and KA17 (see `q_to_send`).
    pub q_gain_law: bool,
    /// The steps the device stores its global gain (the app's preamp) in. The KA15 keeps whole
    /// dB: -4.2 reads back as -4.0.
    pub preamp_step: f64,
    /// What the device divides a shelf's stored Q by when it plays it (see `q_to_send`).
    pub shelf_q_scale: f64,
    /// dB the device plays its global gain below the stored value, on a USER bank.
    pub preamp_offset: f64,
    /// Checked against real hardware in this app.
    pub tested: bool,
}

struct Model {
    name: &'static str,
    gain: (f64, f64),
    types: &'static [&'static str],
    /// Factory voicings: switchable, never written.
    voicings: &'static [(u8, &'static str)],
    /// The writable banks.
    users: &'static [(u8, &'static str)],
    /// The EQ-off slot, if the model has one.
    off: Option<(u8, &'static str)>,
    report_id: u8,
    pace_ms: u64,
    q_gain_law: bool,
    preamp_step: f64,
    shelf_q_scale: f64,
    preamp_offset: f64,
    tested: bool,
}

const ALL: &[&str] = &["PK", "LSQ", "HSQ", "LP", "HP"];
const SHELVES: &[&str] = &["PK", "LSQ", "HSQ"];
const WIDE: (f64, f64) = (-24.0, 12.0);
const NARROW: (f64, f64) = (-12.0, 12.0);

const MUSIC: &[(u8, &str)] = &[(0, "Jazz"), (1, "Pop"), (2, "Rock"), (3, "Dance"), (4, "R&B"), (5, "Classic"), (6, "Hip-hop")];
const MUSIC_RETRO: &[(u8, &str)] = &[(0, "Jazz"), (1, "Pop"), (2, "Rock"), (3, "Dance"), (4, "R&B"), (5, "Classic"), (6, "Hip-hop"), (8, "Retro")];
const MUSIC_DAMP: &[(u8, &str)] = &[(0, "Jazz"), (1, "Pop"), (2, "Rock"), (3, "Dance"), (4, "R&B"), (5, "Classic"), (6, "Hip-hop"),
                                     (8, "Retro"), (9, "sDamp-1"), (10, "sDamp-2")];
const USER1: &[(u8, &str)] = &[(160, "USER1")];
const USER3: &[(u8, &str)] = &[(160, "USER1"), (161, "USER2"), (162, "USER3")];
const USER10: &[(u8, &str)] = &[
    (160, "USER1"), (161, "USER2"), (162, "USER3"), (163, "USER4"), (164, "USER5"),
    (165, "USER6"), (166, "USER7"), (167, "USER8"), (168, "USER9"), (169, "USER10"),
];
const LOW_USER3: &[(u8, &str)] = &[(7, "USER1"), (8, "USER2"), (9, "USER3")];
const BYPASS: Option<(u8, &str)> = Some((240, "Bypass"));
const CLOSE: Option<(u8, &str)> = Some((240, "Close EQ"));

// One table row; the argument list is the row.
#[allow(clippy::too_many_arguments)]
const fn m(name: &'static str, gain: (f64, f64), types: &'static [&'static str], voicings: &'static [(u8, &'static str)],
           users: &'static [(u8, &'static str)], off: Option<(u8, &'static str)>, report_id: u8, pace_ms: u64) -> Model {
    Model { name, gain, types, voicings, users, off, report_id, pace_ms, q_gain_law: false, preamp_step: 0.1,
            shelf_q_scale: 1.0, preamp_offset: 0.0, tested: false }
}

const MODELS: &[Model] = &[
    Model { tested: true, ..m("FIIO BTR17", WIDE, SHELVES, MUSIC, USER10, BYPASS, 7, 100) },
    // Contour measured the KA15 on a line input (github.com/Chronosauros/contour, 2026-10-04):
    // shelves play the stored Q / sqrt(2), and a USER bank plays 12 dB below its stored gain.
    Model { tested: true, preamp_step: 1.0, shelf_q_scale: std::f64::consts::SQRT_2, preamp_offset: 12.0,
            ..m("FIIO KA15", NARROW, ALL, MUSIC, LOW_USER3, Some((10, "Close EQ")), 7, 100) },
    // The KA17 moves R&B, Classic and Hip-hop up a slot to make room for USER1 at 4.
    Model { q_gain_law: true, ..m("FIIO KA17", NARROW, ALL,
            &[(0, "Jazz"), (1, "Pop"), (2, "Rock"), (3, "Dance"), (5, "R&B"), (6, "Classic"), (7, "Hip-hop")],
            &[(4, "USER1"), (8, "USER2"), (9, "USER3")], Some((10, "Bypass")), 1, 200) },
    m("FIIO BTR13", NARROW, ALL, MUSIC, LOW_USER3, Some((11, "Close EQ")), 7, 100),
    Model { q_gain_law: true, ..m("FIIO QX13", WIDE, ALL, MUSIC_RETRO, USER10, BYPASS, 7, 100) },
    m("FIIO FX17", WIDE, ALL, MUSIC, USER1, None, 7, 100),
    m("FIIO FP3", WIDE, SHELVES, MUSIC, USER1, None, 7, 100),
    m("FIIO LS-TC2", NARROW, SHELVES, MUSIC, USER1, None, 7, 100),
    m("FIIO OAK NANO", WIDE, SHELVES, MUSIC, USER1, None, 7, 100),
    m("SNOWSKY Melody", WIDE, ALL, MUSIC, USER3, CLOSE, 7, 100),
    m("SNOWSKY TINY A", NARROW, ALL, MUSIC, USER3, CLOSE, 7, 100),
    m("SNOWSKY TINY B", NARROW, ALL, MUSIC, USER3, CLOSE, 7, 100),
    Model { tested: true, ..m("RETRO NANO", NARROW, ALL, MUSIC_DAMP, USER3, Some((11, "Close EQ")), 7, 100) },
    // FiiO's app says report 7 for the JIEZI; devicePEQ found it on 2, like the JA11. The
    // descriptor decides between them.
    m("JadeAudio JIEZI", NARROW, ALL, MUSIC, USER3, CLOSE, 2, 100),
    m("JadeAudio JA11", NARROW, SHELVES, &[(0, "Vocal"), (1, "Classic"), (2, "Bass")], &[(3, "USER1")],
      Some((4, "Close EQ")), 2, 100),
    m("FIIO Air Link", WIDE, ALL, MUSIC, USER10, None, 7, 300),
    m("FIIO BT11", WIDE, ALL, MUSIC, USER10, None, 7, 300),
    m("FIIO K13 R2R", WIDE, ALL, MUSIC_DAMP, USER10, BYPASS, 7, 200),
    m("FIIO BR15 R2R", WIDE, &["PK", "LSQ", "HSQ", "LP", "HP"], MUSIC_DAMP, USER10, BYPASS, 7, 200),
    m("FIIO QX11", NARROW, ALL, MUSIC_DAMP, USER10, None, 7, 100),
    m("FIIO AIR AMP", WIDE, ALL,
      &[(0, "Jazz"), (1, "Pop"), (2, "Rock"), (3, "Dance"), (4, "R&B"), (5, "Classic"), (6, "Hip-hop"),
        (7, "Monitor"), (9, "sDamp-1"), (10, "sDamp-2")], USER10, BYPASS, 7, 100),
    // The FG3's game and cinema voicings sit at 257 and up, past what a one-byte bank switch
    // can reach, so only its music voicings and USER banks are offered.
    m("FIIO FG3", NARROW, ALL, &[(0, "Jazz"), (1, "Pop"), (4, "R&B")], USER3, None, 7, 100),
];

fn to_profile(m: &Model, product: &str) -> Profile {
    Profile {
        name: if product.is_empty() { m.name.to_string() } else { product.to_string() },
        model: m.name.to_string(),
        bands: 10,
        gain_min: m.gain.0,
        gain_max: m.gain.1,
        q_min: 0.25,
        q_max: 8.0,
        types: m.types.iter().map(|s| s.to_string()).collect(),
        presets: m.voicings.iter().chain(m.users).chain(m.off.iter())
            .map(|(k, v)| (*k, v.to_string())).collect(),
        user_slots: m.users.iter().map(|(k, _)| *k).collect(),
        report_id: m.report_id,
        pace_ms: m.pace_ms.max(DELAY_BAND_MS),
        q_gain_law: m.q_gain_law,
        preamp_step: m.preamp_step,
        shelf_q_scale: m.shelf_q_scale,
        preamp_offset: m.preamp_offset,
        tested: m.tested,
    }
}

/// The model row for a USB product name, matched the way FiiO's app does it: exact, or one
/// name containing the other ("FIIO KA17 (MQA HID)", "Oak Nano"), ignoring case. FiiO's LS-TC2
/// enumerates as "FIIO DSP Audio".
fn model_for(product: &str) -> Option<&'static Model> {
    let t = product.trim().to_lowercase();
    let t = if t == "fiio dsp audio" { "fiio ls-tc2".to_string() } else { t };
    if t.is_empty() {
        return None;
    }
    MODELS.iter().find(|m| m.name.to_lowercase() == t).or_else(|| {
        MODELS.iter().find(|m| {
            let n = m.name.to_lowercase();
            t.contains(&n) || (t.len() >= 5 && n.contains(&t))
        })
    })
}

pub fn profile(_vid: u16, pid: u16, product: &str) -> Option<Profile> {
    if let Some(m) = model_for(product) {
        return Some(to_profile(m, product));
    }
    // A BTR17 that didn't give a product string.
    (pid == 0x0088).then(|| to_profile(&MODELS[0], product))
}

/// Every model this file knows, for the app's device list and offline design.
pub fn all_profiles() -> Vec<Profile> {
    MODELS.iter().map(|m| to_profile(m, "")).collect()
}

// ---- frames ----

fn crc8(bytes: &[u8]) -> u8 {
    // Dallas/Maxim CRC-8 (reflected polynomial 0x8C), which is what FiiO's table works out to.
    let mut c = 0u8;
    for &b in bytes {
        let mut x = c ^ b;
        for _ in 0..8 {
            x = if x & 1 != 0 { (x >> 1) ^ 0x8C } else { x >> 1 };
        }
        c = x;
    }
    c
}

static SEQ: AtomicU16 = AtomicU16::new(0);

fn frame(head: [u8; 2], cmd: u8, payload: &[u8]) -> Vec<u8> {
    frame_seq(SEQ.fetch_add(1, Ordering::Relaxed), head, cmd, payload)
}

fn frame_seq(seq: u16, head: [u8; 2], cmd: u8, payload: &[u8]) -> Vec<u8> {
    let mut b = vec![head[0], head[1], (seq >> 8) as u8, seq as u8, cmd, payload.len() as u8];
    b.extend_from_slice(payload);
    b.push(crc8(&b));
    b.push(END);
    b
}

/// Sends a set. Refuses anything outside SETTABLE.
fn send_set<T: TransportFiio + ?Sized>(dev: &T, cmd: u8, payload: &[u8]) -> bool {
    if !SETTABLE.contains(&cmd) {
        return false;
    }
    dev.send(&frame(SET, cmd, payload))
}

/// The payload of a reply to `cmd`, if `r` is one.
fn reply_payload(r: &[u8], cmd: u8) -> Option<&[u8]> {
    if r.len() < 6 || r[0..2] != GET || r[4] != cmd {
        return None;
    }
    r.get(6..6 + r[5] as usize)
}

/// Asks a question and waits for the matching answer. `accept` sees the reply payload and
/// decides whether it answers *this* request (band reads check the echoed index).
fn request<T: TransportFiio + ?Sized>(
    dev: &T,
    cmd: u8,
    payload: &[u8],
    accept: impl Fn(&[u8]) -> bool,
) -> Option<Vec<u8>> {
    for _ in 0..2 {
        dev.drain();
        if !dev.send(&frame(GET, cmd, payload)) {
            continue;
        }
        let deadline = Instant::now() + Duration::from_millis(REPLY_TIMEOUT_MS);
        while Instant::now() < deadline {
            let left = deadline.saturating_duration_since(Instant::now()).as_millis() as u64;
            let Some(r) = dev.recv(left.max(1)) else { break };
            if let Some(p) = reply_payload(&r, cmd) {
                if accept(p) {
                    return Some(p.to_vec());
                }
            }
        }
    }
    None
}

fn s16(hi: u8, lo: u8) -> i16 {
    i16::from_be_bytes([hi, lo])
}

fn type_code(t: &str) -> u8 {
    match t {
        "LSQ" => 1,
        "HSQ" => 2,
        "BP" => 3,
        "LP" => 4,
        "HP" => 5,
        "AP" => 6,
        _ => 0,
    }
}

fn type_name(c: u8) -> &'static str {
    match c {
        1 => "LSQ",
        2 => "HSQ",
        3 => "BP",
        4 => "LP",
        5 => "HP",
        6 => "AP",
        _ => "PK",
    }
}

// ---- reads ----

/// The active bank. Doubles as the FiiO handshake: it answers even with the device's EQ
/// switched off, when nothing else does.
pub fn read_preset<T: TransportFiio + ?Sized>(dev: &T) -> Option<u8> {
    request(dev, CMD_PRESET, &[], |p| !p.is_empty()).map(|p| p[0])
}

/// How many bands the device has. Every model FiiO's app drives over HID answers this; a
/// silent or implausible answer falls back to ten, the count FiiO's own app starts from.
pub fn read_band_count<T: TransportFiio + ?Sized>(dev: &T) -> usize {
    request(dev, CMD_BAND_COUNT, &[], |p| !p.is_empty())
        .map(|p| p[0] as usize)
        .filter(|n| (1..=31).contains(n))
        .unwrap_or(10)
}

/// The active bank's global gain as stored. It is stored per bank.
fn read_gain<T: TransportFiio + ?Sized>(dev: &T) -> Option<f64> {
    request(dev, CMD_GLOBAL_GAIN, &[], |p| p.len() >= 2).map(|p| s16(p[0], p[1]) as f64 / 10.0)
}

/// The active bank's preamp as played: its global gain less any offset the model applies.
pub fn read_preamp<T: TransportFiio + ?Sized>(dev: &T, p: &Profile) -> Option<f64> {
    read_gain(dev).map(|g| g - p.preamp_offset)
}

/// (freq, gain, q, type code) of one band of the active bank.
type RawBand = (u16, f64, f64, u8);

fn read_band<T: TransportFiio + ?Sized>(dev: &T, i: u8) -> Option<RawBand> {
    let p = request(dev, CMD_BAND, &[i], |p| p.len() >= 8 && p[0] == i)?;
    Some((
        u16::from_be_bytes([p[3], p[4]]),
        s16(p[1], p[2]) as f64 / 10.0,
        u16::from_be_bytes([p[5], p[6]]) as f64 / 100.0,
        p[7],
    ))
}

fn read_raw<T: TransportFiio + ?Sized>(dev: &T, n: usize) -> Vec<Option<RawBand>> {
    (0..n).map(|i| read_band(dev, i as u8)).collect()
}

const EQ_OFF: &str = "isn't sharing its EQ, which is what it does with EQ switched off. Turn EQ on in its settings, then read again.";

/// Reads the active bank, twice, and only returns once two reads agree — a bank switch made
/// on the device itself has the same stale-data window as one made from here.
pub fn read_bank<T: TransportFiio + ?Sized>(dev: &T, p: &Profile) -> Result<Vec<Band>, String> {
    if read_band(dev, 0).is_none() {
        return Err(if read_preset(dev).is_some() {
            format!("The {} {}", p.name, EQ_OFF)
        } else {
            format!("{} is no longer connected.", p.name)
        });
    }
    let mut prev = read_raw(dev, p.bands);
    for _ in 0..5 {
        let next = read_raw(dev, p.bands);
        if next == prev {
            break;
        }
        sleep(Duration::from_millis(250));
        prev = next;
    }
    Ok(prev
        .into_iter()
        .enumerate()
        .map(|(i, b)| match b {
            Some((freq, gain, q, typ)) => Band {
                ok: true,
                band: (i + 1) as u8,
                freq: freq as i32,
                gain,
                q: q_heard(p, q, gain, type_name(typ)),
                typ: type_name(typ).to_string(),
            },
            None => Band { ok: false, band: (i + 1) as u8, freq: 0, gain: 0.0, q: 0.0, typ: "PK".into() },
        })
        .collect())
}

// ---- writes ----

/// Switches the active bank, waits for the device to confirm it, then waits out the window in
/// which band reads still return the old bank. Returns the bank the device reports.
pub fn set_preset<T: TransportFiio + ?Sized>(dev: &T, p: &Profile, slot: u8) -> Result<Option<i32>, String> {
    if !p.presets.contains_key(&slot) {
        return Err(format!("The {} has no bank {}.", p.name, slot));
    }
    // A switch sends no acknowledgement; reading the bank back is the confirmation.
    send_set(dev, CMD_PRESET, &[slot]);
    let deadline = Instant::now() + Duration::from_millis(SWITCH_TIMEOUT_MS);
    let mut now = read_preset(dev);
    while now != Some(slot) && Instant::now() < deadline {
        sleep(Duration::from_millis(50));
        now = read_preset(dev);
    }
    if now == Some(slot) {
        sleep(Duration::from_millis(DELAY_SETTLE_MS));
    }
    Ok(now.map(|s| s as i32))
}

// devicePEQ measured, on a QX13 and a KA17, that a FiiO peaking band realises Q / A, where A is
// the RBJ amplitude 10^(|gain|/40): Q_mult x A came out at 1.001 +/- 0.015 over 17 cases from
// +/-6 to +/-24 dB. Models flagged with `q_gain_law` are sent Q x A so they land on the Q asked
// for, and read back as stored / A. The BTR17 and KA15 are not flagged.
//
// Contour measured the KA15 playing a shelf's stored Q divided by sqrt(2), at every frequency,
// gain and Q tried (LS 80 Hz +4 dB stored 0.70 played 0.50), and peaking bands as stored. So
// its shelves are sent Q x `shelf_q_scale`. FiiO's own app sends the raw Q, so a KA15 bank it
// wrote reads back here with a lower shelf Q than it shows: that is the Q actually heard.
fn rbj_a(gain: f64) -> f64 {
    10f64.powf(gain.abs() / 40.0)
}

fn is_shelf(typ: &str) -> bool {
    typ == "LSQ" || typ == "HSQ"
}

fn q_to_send(p: &Profile, b: &BandIn) -> f64 {
    if p.q_gain_law && b.typ == "PK" {
        (b.q * rbj_a(b.gain)).clamp(p.q_min, p.q_max)
    } else if is_shelf(&b.typ) && p.shelf_q_scale != 1.0 {
        (b.q * p.shelf_q_scale).clamp(p.q_min, p.q_max)
    } else {
        b.q
    }
}

fn q_heard(p: &Profile, q: f64, gain: f64, typ: &str) -> f64 {
    if p.q_gain_law && typ == "PK" {
        q / rbj_a(gain)
    } else if is_shelf(typ) {
        q / p.shelf_q_scale
    } else {
        q
    }
}

fn band_payload(i: u8, b: &BandIn) -> [u8; 8] {
    let freq = b.freq.round().clamp(20.0, 20000.0) as u16;
    let gain = (b.gain * 10.0).round().clamp(-32768.0, 32767.0) as i16;
    let q = (b.q * 100.0).round().clamp(1.0, 65535.0) as u16;
    let [gh, gl] = gain.to_be_bytes();
    let [fh, fl] = freq.to_be_bytes();
    let [qh, ql] = q.to_be_bytes();
    [i, gh, gl, fh, fl, qh, ql, type_code(&b.typ)]
}

fn same_band(got: &RawBand, want: &BandIn) -> bool {
    let p = band_payload(0, want);
    got.0 == u16::from_be_bytes([p[3], p[4]])
        && (got.1 - s16(p[1], p[2]) as f64 / 10.0).abs() < 0.05
        // The BTR17 stores Q a little coarser than the 0.01 steps it is sent in, so some values
        // read back one step low (1.18 as 1.17). That is under 1% and inaudible, not a failed write.
        && (got.2 - u16::from_be_bytes([p[5], p[6]]) as f64 / 100.0).abs() < 0.02
        && got.3 == p[7]
}

/// Writes the whole bank to the active USER bank, then checks what the device actually saved.
///
/// Band writes are one-way, so `acked` is the number of bands found afterwards — and found in
/// the *saved* copy, not the working one. Reading straight after a write returns the working
/// copy, which holds every band even when some never reached flash. Switching to another bank
/// and back reloads the saved copy (it matched the power-on state exactly in testing), so that
/// is what is read back. You hear the other bank for a moment. The save command is sent as
/// FiiO's app does, though band writes save themselves when paced.
pub fn write_bank<T: TransportFiio + ?Sized>(
    dev: &T,
    p: &Profile,
    bands: &[BandIn],
    preamp: f64,
) -> Result<WriteResult, String> {
    let slot = read_preset(dev).ok_or_else(|| format!("{} is no longer connected. Nothing was written.", p.name))?;
    if !p.user_slots.contains(&slot) {
        let name = p.presets.get(&slot).cloned().unwrap_or_else(|| format!("bank {}", slot));
        return Err(format!(
            "The {} is on {}, which can't be overwritten. Pick a USER bank first. Nothing was written.",
            p.name, name
        ));
    }

    // What goes on the wire, after any Q correction; the read-back is checked against this.
    let bands: Vec<BandIn> = bands
        .iter()
        .take(p.bands)
        .map(|b| BandIn { freq: b.freq, gain: b.gain, q: q_to_send(p, b), typ: b.typ.clone() })
        .collect();
    for (i, b) in bands.iter().enumerate() {
        send_set(dev, CMD_BAND, &band_payload(i as u8, b));
        sleep(Duration::from_millis(p.pace_ms));
    }
    // Rounded down to the device's step, so a coarse device gets more headroom, never less,
    // then raised by whatever the model plays its gain down by.
    let heard = preamp.clamp(p.gain_min - p.preamp_offset, p.gain_max - p.preamp_offset);
    let stepped = (heard / p.preamp_step + 1e-9).floor() * p.preamp_step;
    let gain = ((stepped + p.preamp_offset) * 10.0).round() as i16;
    send_set(dev, CMD_GLOBAL_GAIN, &gain.to_be_bytes());
    sleep(Duration::from_millis(p.pace_ms));
    send_set(dev, CMD_SAVE, &[slot]);
    sleep(Duration::from_millis(DELAY_SAVE_MS));

    // Reload the saved copy: away to a neighbouring USER bank and back — or, on a model with
    // only one USER bank, to its first factory voicing.
    let away = p
        .user_slots
        .iter()
        .chain(p.presets.keys())
        .copied()
        .find(|s| *s != slot)
        .unwrap_or(slot);
    let reload = |s: u8| set_preset(dev, p, s).ok().flatten() == Some(s as i32);
    if !(reload(away) && reload(slot)) {
        return Err(format!(
            "{} was written, but the {} didn't switch banks to confirm it was saved. Check it's still connected and on {}.",
            p.presets.get(&slot).cloned().unwrap_or_default(),
            p.name,
            p.presets.get(&slot).cloned().unwrap_or_default()
        ));
    }
    let back = read_raw(dev, bands.len());
    let acked = back
        .iter()
        .zip(&bands)
        .filter(|(got, want)| got.as_ref().is_some_and(|g| same_band(g, want)))
        .count() as u32;
    let gain_back = read_gain(dev);
    let sent = gain as f64 / 10.0;
    // A model nobody has checked may turn out to keep whole dB, as the KA15 does. That is not
    // a failed write, just a coarser one.
    let whole_db = |g: f64| !p.tested && g.fract() == 0.0 && (g - sent).abs() < 1.0;
    if acked as usize == bands.len()
        && gain_back.map(|g| (g - sent).abs() > 0.05 && !whole_db(g)).unwrap_or(true)
    {
        return Err(format!(
            "The bands were saved, but the {}'s preamp reads back as {} instead of {:.1} dB.",
            p.name,
            gain_back.map(|g| format!("{:.1} dB", g - p.preamp_offset)).unwrap_or_else(|| "nothing".into()),
            sent - p.preamp_offset
        ));
    }
    Ok(WriteResult { acked, active: Some(slot as i32) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;

    // FiiO's own CRC table starts 0, 94, 188, 226 … — the Dallas/Maxim CRC-8.
    #[test]
    fn crc_matches_fiios_table() {
        let first: Vec<u8> = (0u8..8).map(|i| crc8(&[i])).collect();
        assert_eq!(first, vec![0, 94, 188, 226, 97, 63, 221, 131]);
    }

    // Frames captured from the probe session against a real BTR17, which accepted them.
    #[test]
    fn frames_match_what_the_device_accepted() {
        let f = frame_seq;
        assert_eq!(f(1, GET, CMD_PRESET, &[]), vec![0xbb, 0x0b, 0x00, 0x01, 0x16, 0x00, 0x62, 0xee]);
        assert_eq!(f(3, GET, 0x18, &[]), vec![0xbb, 0x0b, 0x00, 0x03, 0x18, 0x00, 0xf1, 0xee]);
        assert_eq!(f(2, GET, CMD_BAND, &[0]), vec![0xbb, 0x0b, 0x00, 0x02, 0x15, 0x01, 0x00, 0x71, 0xee]);
    }

    // The KA15 plays shelves at stored Q / sqrt(2) and a USER bank 12 dB under its stored gain,
    // so both are raised on the way out and lowered on the way back. Other models are untouched.
    #[test]
    fn ka15_shelf_q_and_preamp_are_corrected() {
        let ka15 = profile(0, 0, "FIIO KA15").unwrap();
        let btr17 = profile(0, 0x0088, "").unwrap();
        let shelf = BandIn { freq: 80.0, gain: 4.0, q: 0.5, typ: "LSQ".into() };
        let peak = BandIn { freq: 1000.0, gain: 4.0, q: 0.5, typ: "PK".into() };
        assert!((q_to_send(&ka15, &shelf) - 0.7071).abs() < 1e-3);
        assert!((q_heard(&ka15, 0.70, 4.0, "HSQ") - 0.495).abs() < 1e-3);
        assert_eq!(q_to_send(&ka15, &peak), 0.5);
        assert_eq!(q_heard(&ka15, 0.5, 4.0, "PK"), 0.5);
        // A shelf Q past what the register holds is capped there, not wrapped.
        let steep = BandIn { freq: 80.0, gain: 4.0, q: 7.0, typ: "LSQ".into() };
        assert_eq!(q_to_send(&ka15, &steep), ka15.q_max);
        assert_eq!(q_to_send(&btr17, &shelf), 0.5);
        assert_eq!((ka15.preamp_offset, btr17.preamp_offset), (12.0, 0.0));
    }

    // Band 5 of USER10 as the device sent it: 5 kHz, -1.5 dB, Q 0.71, high shelf.
    #[test]
    fn band_payload_round_trips_a_real_band() {
        let b = BandIn { freq: 5000.0, gain: -1.5, q: 0.71, typ: "HSQ".into() };
        let p = band_payload(4, &b);
        assert_eq!(p, [0x04, 0xff, 0xf1, 0x13, 0x88, 0x00, 0x47, 0x02]);
        assert_eq!(s16(p[1], p[2]), -15);
    }

    #[test]
    fn factory_reset_and_other_unlisted_sets_are_refused() {
        struct Spy(RefCell<Vec<Vec<u8>>>);
        impl TransportFiio for Spy {
            fn send(&self, f: &[u8]) -> bool { self.0.borrow_mut().push(f.to_vec()); true }
            fn recv(&self, _: u64) -> Option<Vec<u8>> { None }
            fn drain(&self) {}
        }
        let spy = Spy(RefCell::new(vec![]));
        for cmd in [0x21u8, 0x1B, 0x1C, 0x18, 0x30] {
            assert!(!send_set(&spy, cmd, &[0]), "cmd {cmd:#x} must not be sendable");
        }
        assert!(spy.0.borrow().is_empty());
        assert!(send_set(&spy, CMD_SAVE, &[160]));
    }

    // A fake BTR17: replies queue up, and a leftover reply to a different band must not be
    // taken as the answer.
    struct Fake {
        queued: RefCell<VecDeque<Vec<u8>>>,
        bands: Vec<RawBand>,
        sent: RefCell<Vec<Vec<u8>>>,
    }
    impl Fake {
        fn reply(cmd: u8, payload: &[u8]) -> Vec<u8> {
            let mut r = vec![0xBB, 0x0B, 0, 0, cmd, payload.len() as u8];
            r.extend_from_slice(payload);
            r.extend_from_slice(&[0, 0xEE]);
            r
        }
    }
    impl TransportFiio for Fake {
        fn send(&self, f: &[u8]) -> bool {
            self.sent.borrow_mut().push(f.to_vec());
            if f[0] == 0xBB && f[4] == CMD_BAND {
                let i = f[6] as usize;
                let (freq, gain, q, t) = self.bands[i];
                let [gh, gl] = ((gain * 10.0) as i16).to_be_bytes();
                let [fh, fl] = freq.to_be_bytes();
                let [qh, ql] = ((q * 100.0).round() as u16).to_be_bytes();
                // A stale answer for the next band arrives first.
                let j = (i + 1) % self.bands.len();
                self.queued.borrow_mut().push_back(Fake::reply(CMD_BAND, &[j as u8, 0, 0, 0, 0, 0, 0, 0]));
                self.queued.borrow_mut().push_back(Fake::reply(CMD_BAND, &[i as u8, gh, gl, fh, fl, qh, ql, t]));
            }
            true
        }
        fn recv(&self, _: u64) -> Option<Vec<u8>> { self.queued.borrow_mut().pop_front() }
        fn drain(&self) {}
    }

    #[test]
    fn band_reads_ignore_replies_for_other_bands() {
        let p = profile(0x2972, 0x0088, "FIIO BTR17").unwrap();
        let mut bands: Vec<RawBand> = (0..10).map(|i| (100 * (i + 1), 0.0, 0.71, 0)).collect();
        bands[4] = (5000, -1.5, 0.71, 2);
        let fake = Fake { queued: RefCell::new(VecDeque::new()), bands, sent: RefCell::new(vec![]) };
        let got = read_bank(&fake, &p).unwrap();
        assert_eq!(got.len(), 10);
        assert!(got.iter().all(|b| b.ok));
        assert_eq!((got[4].freq, got[4].gain, got[4].typ.as_str()), (5000, -1.5, "HSQ"));
        assert_eq!(got[0].freq, 100);
        // Reads only: nothing headed AA (a set) went out.
        assert!(fake.sent.borrow().iter().all(|f| f[0] == 0xBB));
    }

    // Q values the BTR17 actually returned one step low after a write, from a real bank.
    // They must count as written; a wrong gain, frequency or type still must not.
    #[test]
    fn read_back_tolerates_the_devices_q_rounding() {
        let want = |freq: f64, gain: f64, q: f64| BandIn { freq, gain, q, typ: "PK".into() };
        assert!(same_band(&(20, 2.0, 1.17, 0), &want(20.0, 2.0, 1.18)));
        assert!(same_band(&(86, 2.3, 1.05, 0), &want(86.0, 2.3, 1.06)));
        assert!(same_band(&(161, 1.2, 2.11, 0), &want(161.0, 1.2, 2.12)));
        assert!(!same_band(&(161, 1.2, 2.05, 0), &want(161.0, 1.2, 2.12)));
        assert!(!same_band(&(161, 1.1, 2.12, 0), &want(161.0, 1.2, 2.12)));
        assert!(!same_band(&(160, 1.2, 2.12, 0), &want(161.0, 1.2, 2.12)));
        assert!(!same_band(&(161, 1.2, 2.12, 2), &want(161.0, 1.2, 2.12)));
    }

    // A fake BTR17 that keeps a working and a saved copy per bank, the way the real one does:
    // a band write reaches the saved copy only if it comes at least 50 ms after the previous
    // one, reads return the working copy, and switching banks reloads the saved copy.
    struct TwoCopies {
        active: std::cell::Cell<u8>,
        working: RefCell<std::collections::HashMap<u8, Vec<RawBand>>>,
        saved: RefCell<std::collections::HashMap<u8, Vec<RawBand>>>,
        gain: std::cell::Cell<i16>,
        last_band: std::cell::Cell<Option<Instant>>,
        queued: RefCell<VecDeque<Vec<u8>>>,
        never_saves: Option<u8>,
    }
    impl TwoCopies {
        fn new(never_saves: Option<u8>) -> Self {
            let flat: Vec<RawBand> = (0..10).map(|i| (100 * (i + 1), 0.0, 0.71, 0)).collect();
            let banks: std::collections::HashMap<u8, Vec<RawBand>> = (160..170).map(|s| (s, flat.clone())).collect();
            TwoCopies {
                active: std::cell::Cell::new(166),
                working: RefCell::new(banks.clone()),
                saved: RefCell::new(banks),
                gain: std::cell::Cell::new(0),
                last_band: std::cell::Cell::new(None),
                queued: RefCell::new(VecDeque::new()),
                never_saves,
            }
        }
    }
    impl TransportFiio for TwoCopies {
        fn send(&self, f: &[u8]) -> bool {
            let (get, cmd, p) = (f[0] == 0xBB, f[4], &f[6..6 + f[5] as usize]);
            let a = self.active.get();
            match (get, cmd) {
                (true, CMD_PRESET) => self.queued.borrow_mut().push_back(Fake::reply(CMD_PRESET, &[a])),
                (false, CMD_PRESET) => {
                    self.active.set(p[0]);
                    let s = self.saved.borrow()[&p[0]].clone();
                    self.working.borrow_mut().insert(p[0], s);
                }
                (true, CMD_BAND) => {
                    let (freq, gain, q, t) = self.working.borrow()[&a][p[0] as usize];
                    let [gh, gl] = ((gain * 10.0).round() as i16).to_be_bytes();
                    let [fh, fl] = freq.to_be_bytes();
                    let [qh, ql] = ((q * 100.0).round() as u16).to_be_bytes();
                    self.queued.borrow_mut().push_back(Fake::reply(CMD_BAND, &[p[0], gh, gl, fh, fl, qh, ql, t]));
                }
                (false, CMD_BAND) => {
                    let b = (u16::from_be_bytes([p[3], p[4]]), s16(p[1], p[2]) as f64 / 10.0,
                             u16::from_be_bytes([p[5], p[6]]) as f64 / 100.0, p[7]);
                    self.working.borrow_mut().get_mut(&a).unwrap()[p[0] as usize] = b;
                    let paced = self.last_band.get().map_or(true, |t| t.elapsed() >= Duration::from_millis(50));
                    if paced && self.never_saves != Some(p[0]) {
                        self.saved.borrow_mut().get_mut(&a).unwrap()[p[0] as usize] = b;
                    }
                    self.last_band.set(Some(Instant::now()));
                }
                (true, CMD_GLOBAL_GAIN) => {
                    let [h, l] = self.gain.get().to_be_bytes();
                    self.queued.borrow_mut().push_back(Fake::reply(CMD_GLOBAL_GAIN, &[h, l]));
                }
                (false, CMD_GLOBAL_GAIN) => self.gain.set(s16(p[0], p[1])),
                _ => {}
            }
            true
        }
        fn recv(&self, _: u64) -> Option<Vec<u8>> { self.queued.borrow_mut().pop_front() }
        fn drain(&self) {}
    }

    fn probe_bank() -> Vec<BandIn> {
        (0..10).map(|i| BandIn { freq: 1000.0 + 111.0 * i as f64, gain: if i % 2 == 0 { -1.5 } else { 2.0 }, q: 1.2, typ: "PK".into() }).collect()
    }

    // The pacing is what gets every band into flash on the real device.
    #[test]
    fn paced_writes_reach_the_saved_copy() {
        let p = profile(0x2972, 0x0088, "FIIO BTR17").unwrap();
        let dev = TwoCopies::new(None);
        let w = write_bank(&dev, &p, &probe_bank(), -2.0).unwrap();
        assert_eq!(w.acked, 10);
        assert_eq!(dev.active.get(), 166, "should end on the bank it wrote");
        assert!(dev.saved.borrow()[&166].iter().zip(probe_bank()).all(|(s, w)| s.0 == w.freq as u16));
    }

    // A band that never reaches flash must be counted as not written, even though the working
    // copy — what a plain read returns — has it. This is the 0.6.0 bug.
    #[test]
    fn a_band_missing_from_the_saved_copy_is_reported() {
        let p = profile(0x2972, 0x0088, "FIIO BTR17").unwrap();
        let dev = TwoCopies::new(Some(4));
        let w = write_bank(&dev, &p, &probe_bank(), -2.0).unwrap();
        assert_eq!(w.acked, 9);
    }

    #[test]
    fn btr17_banks_are_numbered_from_160() {
        let p = profile(0x2972, 0x0088, "FIIO BTR17").unwrap();
        assert_eq!(p.presets[&160], "USER1");
        assert_eq!(p.presets[&166], "USER7");
        assert_eq!(p.presets[&169], "USER10");
        assert!(!p.user_slots.contains(&0) && !p.user_slots.contains(&240));
        assert!(profile(0x2972, 0x1234, "something else").is_none());
    }
}
