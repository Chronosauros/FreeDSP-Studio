// Public HID API. Picks a platform transport, serialises access to the cable, and refuses
// to write to a cable that isn't the one the user connected. The protocol itself lives in
// dsp.rs; everything here is bookkeeping.

use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, OnceLock};

use crate::dsp;
use crate::dsp2;
use crate::dsp_fiio;
use crate::dsp_fosi;
use crate::dsp_kt;
use crate::hid_fiio;
use crate::hid_gen2;
use crate::hid_report::{self, Via};

pub use crate::dsp::{Band, BandIn, CableInfo, WriteResult};

/// Vendor ids we look for. 0x35D8 is Moondrop (both generations); 0x3302 is CrinEar, whose
/// cables are Walkplay-chipset parts speaking the same gen-2 protocol.
pub const VIDS: [u16; 2] = [0x35D8, 0x3302];

/// The other vendor ids Walkplay-chipset parts ship under, from devicePEQ's catalogue. Some are
/// generic (C-Media's 0x0D8C is on every cheap USB sound card), so these only get the gen-2
/// handshake when their descriptor declares report 0x4B both ways.
const WALKPLAY_VIDS: [u16; 17] = [
    0x0104, 0x011B, 0x011D, 0x0661, 0x0663, 0x0666, 0x0762, 0x0909, 0x0D8C, 0x2FC6, 0x34BE, 0x36A7,
    0x373B, 0x60C1, 0x60E1, 0xB445, 0xB44D,
];

#[cfg(windows)]
use crate::hid_win as backend;

#[cfg(not(windows))]
use crate::hid_hidapi as backend;

// ---- the HID thread ----
//
// Every device op runs on this one thread, for the life of the process. That's not just
// tidiness: hidapi's macOS backend binds its global IOHIDManager to whatever thread first
// calls hid_init(), by way of
//
//   IOHIDManagerScheduleWithRunLoop(hid_mgr, CFRunLoopGetCurrent(), ...)
//
// and the Rust wrapper inits once and never calls hid_exit(). Each later enumerate then
// schedules the devices it matches onto that same stored run loop. Run these ops on tokio's
// blocking pool and the binding lands on a pool thread that idles out and exits a few
// seconds later, taking its CFRunLoop with it — the next enumerate hands IOKit a dangling
// CFRunLoopRef and the process dies on a pointer-authentication trap in CFRunLoopAddSource.
// A thread that never exits keeps that run loop alive and valid.
//
// Serialising the ops is the other half of the job: the cable answers one request at a
// time, and the channel gives that for free.

type Job = Box<dyn FnOnce() + Send + 'static>;

fn hid_thread() -> &'static Sender<Job> {
    static TX: OnceLock<Sender<Job>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("hid".to_string())
            .spawn(move || {
                for job in rx {
                    job();
                }
            })
            .expect("could not start the HID thread");
        tx
    })
}

// Hands `f` to the HID thread and waits for it. A panic inside `f` drops the reply channel
// rather than killing the thread, so one bad op doesn't take the rest of the session down.
fn on_hid_thread<T, F>(f: F) -> Result<T, String>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let (done, wait) = mpsc::channel();
    let job: Job = Box::new(move || {
        if let Ok(v) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
            let _ = done.send(v);
        }
    });
    hid_thread()
        .send(job)
        .map_err(|_| "the HID thread has stopped".to_string())?;
    wait.recv()
        .map_err(|_| "the HID operation failed".to_string())
}

// Remembers which cable we opened. Read and written from the HID thread and from close(),
// so it keeps its own lock.
static EXPECTED: Mutex<Option<Expected>> = Mutex::new(None);

#[derive(Clone)]
struct Expected {
    vid: u16,
    pid: u16,
    name: String,
    kind: Kind,
}

/// Which protocol the open device speaks. The gen-2 and FiiO arms carry the device's
/// capabilities, which every op on those paths needs.
#[derive(Clone)]
enum Kind {
    Gen1,
    Gen2(dsp2::Profile),
    Fiio(dsp_fiio::Profile),
    Kt(dsp_kt::Profile),
    Fosi(dsp_fosi::Profile),
}

fn expected() -> Option<Expected> {
    EXPECTED.lock().unwrap().clone()
}

fn expected_pid() -> Option<u16> {
    EXPECTED.lock().unwrap().as_ref().map(|e| e.pid)
}

/// Opens the remembered gen-2 cable and hands it to `f`. Refuses if the remembered cable is
/// gone or is a gen-1 one — the same "don't write to the wrong cable" guarantee `guard` gives
/// the gen-1 path, but enforced by identity rather than after the fact, since a gen-2 handle
/// is opened by exact vid:pid.
fn on_gen2<T>(f: impl FnOnce(&hid_gen2::Dev, &dsp2::Profile) -> T + Send + 'static) -> Result<T, String>
where
    T: Send + 'static,
{
    let exp = expected().ok_or_else(|| "no cable is open".to_string())?;
    let Kind::Gen2(profile) = exp.kind.clone() else {
        return Err("this cable does not speak the gen-2 protocol".to_string());
    };
    on_hid_thread(move || {
        let dev = hid_gen2::open(exp.vid, exp.pid).map_err(|e| {
            format!(
                "{} is no longer connected: {}. Nothing was written.",
                exp.name, e
            )
        })?;
        Ok(f(&dev, &profile))
    })?
}

/// The FiiO counterpart of `on_gen2`: reopens the remembered device by exact vid:pid, so an op
/// can never land on a different one.
fn on_fiio<T>(f: impl FnOnce(&hid_fiio::Dev, &dsp_fiio::Profile) -> T + Send + 'static) -> Result<T, String>
where
    T: Send + 'static,
{
    let exp = expected().ok_or_else(|| "no cable is open".to_string())?;
    let Kind::Fiio(profile) = exp.kind.clone() else {
        return Err("this device does not speak the FiiO protocol".to_string());
    };
    on_hid_thread(move || {
        let dev = hid_fiio::open(exp.vid, exp.pid, profile.report_id).map_err(|e| {
            format!("{} is no longer connected: {}. Nothing was written.", exp.name, e)
        })?;
        Ok(f(&dev, &profile))
    })?
}

/// A FiiO device we have a profile for and that answers the handshake (an active-bank read).
/// The band count and the report it actually answered on are filled in from the device.
fn probe_fiio(vid: u16, pid: u16, product: &str) -> Option<dsp_fiio::Profile> {
    let mut profile = dsp_fiio::profile(vid, pid, product)?;
    let dev = hid_fiio::open(vid, pid, profile.report_id).ok()?;
    dsp_fiio::read_preset(&dev)?;
    profile.report_id = dev.report_id();
    profile.bands = dsp_fiio::read_band_count(&dev);
    Some(profile)
}

/// The KT Micro and Fosi counterparts of `on_fiio`.
fn on_kt<T>(f: impl FnOnce(&hid_report::Dev, &dsp_kt::Profile) -> T + Send + 'static) -> Result<T, String>
where
    T: Send + 'static,
{
    let exp = expected().ok_or_else(|| "no cable is open".to_string())?;
    let Kind::Kt(profile) = exp.kind.clone() else {
        return Err("this device does not speak the KT Micro protocol".to_string());
    };
    on_hid_thread(move || {
        let dev = hid_report::open(exp.vid, exp.pid, &[dsp_kt::REPORT_ID], Via::Interrupt)
            .map_err(|e| format!("{} is no longer connected: {}. Nothing was written.", exp.name, e))?;
        Ok(f(&dev, &profile))
    })?
}

fn on_fosi<T>(f: impl FnOnce(&hid_report::Dev, &dsp_fosi::Profile) -> T + Send + 'static) -> Result<T, String>
where
    T: Send + 'static,
{
    let exp = expected().ok_or_else(|| "no cable is open".to_string())?;
    let Kind::Fosi(profile) = exp.kind.clone() else {
        return Err("this device does not speak the Fosi Audio protocol".to_string());
    };
    on_hid_thread(move || {
        let dev = hid_report::open(exp.vid, exp.pid, &[dsp_fosi::REPORT_ID], Via::Feature)
            .map_err(|e| format!("{} is no longer connected: {}. Nothing was written.", exp.name, e))?;
        Ok(f(&dev, &profile))
    })?
}

fn probe_kt(pid: u16, product: &str) -> Option<dsp_kt::Profile> {
    let dev = hid_report::open(dsp_kt::VID, pid, &[dsp_kt::REPORT_ID], Via::Interrupt).ok()?;
    dsp_kt::read_eq(&dev)?;
    Some(dsp_kt::profile(pid, product))
}

fn probe_fosi(pid: u16, product: &str) -> Option<dsp_fosi::Profile> {
    let p = dsp_fosi::profile(product)?;
    let dev = hid_report::open(dsp_fosi::VID, pid, &[dsp_fosi::REPORT_ID], Via::Feature).ok()?;
    dsp_fosi::read_enabled(&dev)?;
    Some(p)
}

/// Whether a device on one of the long-tail Walkplay vendor ids declares report 0x4B both ways.
fn declares_walkplay(vid: u16, pid: u16) -> bool {
    hid_report::reports(vid, pid)
        .map(|rs| rs.iter().any(|r| r.id == dsp2::REPORT_ID && r.input > 0 && r.output > 0))
        .unwrap_or(false)
}

fn all_vids() -> Vec<u16> {
    VIDS.iter()
        .chain(WALKPLAY_VIDS.iter())
        .chain(dsp_fiio::VIDS.iter())
        .chain([dsp_kt::VID, dsp_fosi::VID].iter())
        .copied()
        .collect()
}

/// What a candidate turned out to be, once asked.
enum Probed {
    Fiio(dsp_fiio::Profile),
    Kt(dsp_kt::Profile),
    Fosi(dsp_fosi::Profile),
    Gen2(dsp2::Profile, String),
    /// A Moondrop cable that didn't answer gen-2: the original report-0x01 protocol.
    Gen1,
    Nothing,
}

/// Settles a candidate's protocol by vendor id and then by asking it. FiiO, KT Micro and Fosi
/// parts are told apart by vendor id and never sent the Walkplay handshake (the KA15 and KT
/// Micro parts both carry a report 0x4B of their own).
fn probe(vid: u16, pid: u16, product: &str) -> Probed {
    if dsp_fiio::VIDS.contains(&vid) {
        return probe_fiio(vid, pid, product).map_or(Probed::Nothing, Probed::Fiio);
    }
    if vid == dsp_kt::VID {
        return probe_kt(pid, product).map_or(Probed::Nothing, Probed::Kt);
    }
    if vid == dsp_fosi::VID {
        return probe_fosi(pid, product).map_or(Probed::Nothing, Probed::Fosi);
    }
    if WALKPLAY_VIDS.contains(&vid) && !declares_walkplay(vid, pid) {
        return Probed::Nothing;
    }
    if let Ok(dev) = hid_gen2::open(vid, pid) {
        if let Some(fw) = dsp2::read_version(&dev) {
            let mut profile = dsp2::cable_profile(vid, pid, product);
            // An uncharacterised cable tells us its editable slot rather than us guessing.
            if profile.presets.is_empty() {
                let slot = dsp2::read_slot(&dev).unwrap_or(0);
                profile.custom_slot = slot;
                profile.presets.insert(slot, "Custom EQ".to_string());
            }
            return Probed::Gen2(profile, fw);
        }
    }
    if vid == dsp::MOONDROP_VID {
        Probed::Gen1
    } else {
        Probed::Nothing
    }
}

fn guard(dev: &backend::Dev) -> Result<(), String> {
    use dsp::Transport;
    if let Some(exp) = EXPECTED.lock().unwrap().as_ref() {
        if dev.pid() != exp.pid {
            let found = dsp::cable_profile(dev.pid(), dev.product()).0;
            return Err(format!(
                "Wrong cable connected: expected {}, found {}. Nothing was written.",
                exp.name, found
            ));
        }
    }
    Ok(())
}

// ---- public API (each opens/closes its own handle, serialised) ----

/// One attached cable, as offered to the user when more than one is plugged in.
#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Listed {
    pub vid: u32,
    pub pid: u32,
    pub name: String,
    pub gen: u8,
}

/// Every cable we can drive, with its protocol generation already probed.
///
/// Generation is settled by asking, not by a product-id table: a gen-2 cable answers a
/// firmware-version read on report 0x4B, and a gen-1 cable has no such report and stays
/// silent. That keeps the many other Walkplay-chipset cables working without enumerating
/// every product id they ship under.
pub fn list() -> Result<Vec<Listed>, String> {
    on_hid_thread(|| {
        let mut out = Vec::new();
        for (vid, pid, product) in hid_gen2::candidates(&all_vids())? {
            let (name, gen) = match probe(vid, pid, &product) {
                Probed::Fiio(p) => (p.name, 3),
                Probed::Kt(p) => (p.name, 4),
                Probed::Fosi(p) => (p.name, 5),
                Probed::Gen2(p, _) => (p.name, 2),
                Probed::Gen1 => (dsp::cable_profile(pid, &product).0, 1),
                Probed::Nothing => continue,
            };
            out.push(Listed { vid: vid as u32, pid: pid as u32, name, gen });
        }
        Ok(out)
    })?
}

/// Opens a cable. With `want` set, opens exactly that vid:pid; otherwise takes the first one
/// found, preferring gen-2 since those are the cables a gen-1-only open would silently
/// mis-drive.
pub fn open(want: Option<(u16, u16)>) -> Result<CableInfo, String> {
    on_hid_thread(move || {
        let mut candidates = hid_gen2::candidates(&all_vids())?;
        if let Some((v, p)) = want {
            candidates.retain(|(cv, cp, _)| (*cv, *cp) == (v, p));
            if candidates.is_empty() {
                return Err(format!("no cable at {:04x}:{:04x}", v, p));
            }
        }

        // Anything that isn't a gen-1 Moondrop cable, first one that answers. Gen-1 comes last:
        // a gen-1-only open would silently mis-drive the newer cables.
        for (vid, pid, product) in &candidates {
            let (vid, pid) = (*vid, *pid);
            let (info, kind) = match probe(vid, pid, product) {
                Probed::Fiio(p) => (
                    CableInfo {
                        name: p.name.clone(),
                        vid: vid as u32,
                        pid: pid as u32,
                        has_presets: Some(true),
                        presets: p.presets.clone(),
                        gen: 3,
                        bands: p.bands,
                        gain_min: p.gain_min,
                        gain_max: p.gain_max,
                        q_min: p.q_min,
                        q_max: p.q_max,
                        types: p.types.clone(),
                        preamp: "global".into(),
                        firmware: None,
                        user_slots: p.user_slots.clone(),
                        family: "FiiO".into(),
                        tested: p.tested,
                        preamp_step: p.preamp_step,
                    },
                    Kind::Fiio(p),
                ),
                Probed::Kt(p) => (
                    CableInfo {
                        name: p.name.clone(),
                        vid: vid as u32,
                        pid: pid as u32,
                        has_presets: Some(false),
                        presets: [(3u8, "Custom EQ".to_string())].into_iter().collect(),
                        gen: 4,
                        bands: p.bands,
                        gain_min: p.gain_min,
                        gain_max: p.gain_max,
                        q_min: p.q_min,
                        q_max: p.q_max,
                        types: p.types.clone(),
                        preamp: p.preamp.clone(),
                        firmware: None,
                        user_slots: Vec::new(),
                        family: "KT Micro".into(),
                        tested: false,
                        preamp_step: 1.0,
                    },
                    Kind::Kt(p),
                ),
                Probed::Fosi(p) => (
                    CableInfo {
                        name: p.name.clone(),
                        vid: vid as u32,
                        pid: pid as u32,
                        has_presets: Some(true),
                        presets: p.presets.clone(),
                        gen: 5,
                        bands: p.bands,
                        gain_min: p.gain_min,
                        gain_max: p.gain_max,
                        q_min: p.q_min,
                        q_max: p.q_max,
                        types: p.types.clone(),
                        preamp: "none".into(),
                        firmware: None,
                        user_slots: p.user_slots.clone(),
                        family: "Fosi Audio".into(),
                        tested: false,
                        preamp_step: 0.1,
                    },
                    Kind::Fosi(p),
                ),
                Probed::Gen2(profile, firmware) => (
                    CableInfo {
                        name: profile.name.clone(),
                        vid: vid as u32,
                        pid: pid as u32,
                        has_presets: Some(profile.presets.len() > 1),
                        presets: profile.presets.clone(),
                        gen: 2,
                        bands: profile.bands,
                        gain_min: profile.gain_min,
                        gain_max: profile.gain_max,
                        q_min: profile.q_min,
                        q_max: profile.q_max,
                        types: profile.types.clone(),
                        preamp: profile.preamp.clone(),
                        firmware: Some(firmware),
                        user_slots: Vec::new(),
                        family: profile.family.clone(),
                        tested: profile.tested,
                        // GLOBAL_GAIN holds whole dB; PRE_GAIN 1/256 dB, offered in tenths.
                        preamp_step: if profile.preamp == "global" { 1.0 } else { 0.1 },
                    },
                    Kind::Gen2(profile),
                ),
                Probed::Gen1 | Probed::Nothing => continue,
            };
            *EXPECTED.lock().unwrap() = Some(Expected { vid, pid, name: info.name.clone(), kind });
            return Ok(info);
        }

        // Gen-1: the original report-0x01 cables, Moondrop only.
        use dsp::Transport;
        let dev = backend::open_best(want.map(|(_, p)| p))?;
        let (name, has, presets) = dsp::cable_profile(dev.pid(), dev.product());
        let (bands, gain_min, gain_max, q_min, q_max, types) = dsp::gen1_limits();
        *EXPECTED.lock().unwrap() = Some(Expected {
            vid: dsp::MOONDROP_VID,
            pid: dev.pid(),
            name: name.clone(),
            kind: Kind::Gen1,
        });
        Ok(CableInfo {
            name,
            vid: dsp::MOONDROP_VID as u32,
            pid: dev.pid() as u32,
            has_presets: has,
            presets,
            gen: 1,
            bands,
            gain_min,
            gain_max,
            q_min,
            q_max,
            types,
            preamp: "baked".into(),
            firmware: None,
            user_slots: Vec::new(),
            family: "Moondrop".into(),
            tested: true,
            preamp_step: 0.1,
        })
    })?
}

/// A device that holds several writable banks, described for designing offline.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BankTarget {
    pub name: String,
    pub family: String,
    pub bands: usize,
    pub gain_min: f64,
    pub gain_max: f64,
    pub q_min: f64,
    pub q_max: f64,
    pub types: Vec<String>,
    pub presets: std::collections::BTreeMap<u8, String>,
    pub user_slots: Vec<u8>,
    pub tested: bool,
}

/// Every bank-holding model the app knows, whether or not one is attached.
pub fn bank_targets() -> Vec<BankTarget> {
    let fiio = dsp_fiio::all_profiles().into_iter().map(|p| BankTarget {
        name: p.model,
        family: "FiiO".into(),
        bands: p.bands,
        gain_min: p.gain_min,
        gain_max: p.gain_max,
        q_min: p.q_min,
        q_max: p.q_max,
        types: p.types,
        presets: p.presets,
        user_slots: p.user_slots,
        tested: p.tested,
    });
    let fosi = dsp_fosi::all_profiles().into_iter().map(|p| BankTarget {
        name: p.name,
        family: "Fosi Audio".into(),
        bands: p.bands,
        gain_min: p.gain_min,
        gain_max: p.gain_max,
        q_min: p.q_min,
        q_max: p.q_max,
        types: p.types,
        presets: p.presets,
        user_slots: p.user_slots,
        tested: false,
    });
    fiio.chain(fosi).collect()
}

pub fn close() {
    *EXPECTED.lock().unwrap() = None;
}

fn kind() -> Kind {
    EXPECTED.lock().unwrap().as_ref().map(|e| e.kind.clone()).unwrap_or(Kind::Gen1)
}

fn is_gen2() -> bool {
    matches!(kind(), Kind::Gen2(_))
}

fn is_fiio() -> bool {
    matches!(kind(), Kind::Fiio(_))
}

pub fn read_mode() -> Result<Option<i32>, String> {
    match kind() {
        Kind::Kt(_) => return on_kt(|dev, _| dsp_kt::read_eq(dev).map(|s| s as i32)),
        Kind::Fosi(_) => return Ok(Some(dsp_fosi::active() as i32)),
        _ => {}
    }
    if is_fiio() {
        return on_fiio(|dev, _| dsp_fiio::read_preset(dev).map(|s| s as i32));
    }
    if is_gen2() {
        return on_gen2(|dev, _| dsp2::read_slot(dev).map(|s| s as i32));
    }
    on_hid_thread(|| {
        let dev = backend::open_best(expected_pid())?;
        Ok(dsp::read_mode(&dev))
    })?
}

pub fn read_bank() -> Result<Vec<Band>, String> {
    match kind() {
        Kind::Kt(_) => return on_kt(|dev, p| dsp_kt::read_bank(dev, p)),
        Kind::Fosi(_) => return on_fosi(|dev, p| dsp_fosi::read_bank(dev, p)),
        _ => {}
    }
    if is_fiio() {
        return on_fiio(|dev, p| dsp_fiio::read_bank(dev, p))?;
    }
    if is_gen2() {
        return on_gen2(|dev, p| dsp2::read_bank(dev, p.bands));
    }
    on_hid_thread(|| {
        let dev = backend::open_best(expected_pid())?;
        Ok(dsp::read_bank(&dev))
    })?
}

pub fn set_preset(idx: i32) -> Result<Option<i32>, String> {
    match kind() {
        // One editable EQ and nothing to switch to.
        Kind::Kt(_) => return on_kt(|dev, _| Ok(dsp_kt::read_eq(dev).map(|s| s as i32)))?,
        Kind::Fosi(_) => {
            let slot = u8::try_from(idx).map_err(|_| format!("there is no bank {}", idx))?;
            return on_fosi(move |dev, p| dsp_fosi::set_preset(dev, p, slot))?;
        }
        _ => {}
    }
    if is_fiio() {
        let slot = u8::try_from(idx).map_err(|_| format!("there is no bank {}", idx))?;
        return on_fiio(move |dev, p| dsp_fiio::set_preset(dev, p, slot))?;
    }
    if is_gen2() {
        return on_gen2(move |dev, _| dsp2::set_preset(dev, idx as u8));
    }
    on_hid_thread(move || {
        let dev = backend::open_best(expected_pid())?;
        guard(&dev)?;
        Ok(dsp::set_preset(&dev, idx))
    })?
}

pub fn write_bank(bands: Vec<BandIn>, preamp: f64) -> Result<WriteResult, String> {
    match kind() {
        Kind::Kt(_) => return on_kt(move |dev, p| dsp_kt::write_bank(dev, p, &bands, preamp))?,
        Kind::Fosi(_) => return on_fosi(move |dev, p| dsp_fosi::write_bank(dev, p, &bands))?,
        _ => {}
    }
    if is_fiio() {
        return on_fiio(move |dev, p| dsp_fiio::write_bank(dev, p, &bands, preamp))?;
    }
    if is_gen2() {
        return on_gen2(move |dev, p| dsp2::write_bank(dev, p, &bands, preamp));
    }
    on_hid_thread(move || {
        let dev = backend::open_best(expected_pid())?;
        guard(&dev)?;
        Ok(dsp::write_bank(&dev, &bands, preamp))
    })?
}

/// The preamp the cable is currently holding. Gen-2 and FiiO only — gen-1 bakes its preamp
/// into a band's coefficients, so there is no register to read it back from. On FiiO it is
/// the active bank's global gain, which is stored per bank.
pub fn read_preamp() -> Result<Option<f64>, String> {
    match kind() {
        Kind::Kt(_) => return on_kt(|dev, p| dsp_kt::read_preamp(dev, p)),
        Kind::Fosi(_) => return Ok(None),
        _ => {}
    }
    if is_fiio() {
        return on_fiio(|dev, _| dsp_fiio::read_gain(dev));
    }
    if is_gen2() {
        return on_gen2(|dev, p| dsp2::read_preamp(dev, p));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every op must land on the one long-lived HID thread — see the note above on why the
    // macOS backend can't be driven from a churn of short-lived threads.
    #[test]
    fn ops_share_one_permanent_thread() {
        let name = || {
            on_hid_thread(|| std::thread::current().name().map(str::to_string)).unwrap()
        };
        let first = name();
        assert_eq!(first.as_deref(), Some("hid"));

        // Called from other threads, and after a gap long enough for a tokio blocking-pool
        // thread to have idled out and taken its CFRunLoop with it.
        std::thread::sleep(std::time::Duration::from_millis(50));
        let from_elsewhere = std::thread::spawn(name).join().unwrap();
        assert_eq!(from_elsewhere, first);
    }

    // A panicking op must not take the HID thread — and so every later op — down with it.
    #[test]
    fn a_panicking_op_leaves_the_thread_usable() {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let boom = on_hid_thread(|| panic!("boom"));
        std::panic::set_hook(prev);

        assert!(boom.is_err());
        assert_eq!(on_hid_thread(|| 42).unwrap(), 42);
    }

    // Repeated real device ops: the genuine open path, over and over, with gaps. This is the
    // shape that killed the app on macOS 27, so it's worth running for real rather than
    // against a stub. Passes whether or not a cable is attached — it's the crash we're
    // testing for, not the cable.
    #[test]
    fn repeated_ops_stay_alive() {
        for _ in 0..10 {
            if let Err(e) = read_bank() {
                assert!(
                    e.contains("no Moondrop cable found") || e.contains("could not open"),
                    "unexpected error: {e}"
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(120));
        }
    }

    // End-to-end against whatever is plugged in, through the same entry points the UI calls.
    // Writes a probe bank to each gen-2 cable and puts the original back, so it is safe to
    // run on a cable you care about — but it does touch the hardware, so it stays ignored.
    //   cargo test --lib -- --ignored --nocapture gen2_round_trip
    #[test]
    #[ignore = "needs a gen-2 cable attached; writes and restores its EQ"]
    fn gen2_round_trip() {
        let found = list().expect("list failed");
        println!("attached: {:?}", found.iter().map(|d| (&d.name, d.gen)).collect::<Vec<_>>());
        let gen2: Vec<_> = found.iter().filter(|d| d.gen == 2).collect();
        assert!(!gen2.is_empty(), "no gen-2 cable attached");

        for d in gen2 {
            let info = open(Some((d.vid as u16, d.pid as u16))).expect("open failed");
            println!(
                "\n{} ({:04x}:{:04x}) fw={:?} bands={} gain={}..{} types={:?} preamp={}",
                info.name, info.vid, info.pid, info.firmware, info.bands,
                info.gain_min, info.gain_max, info.types, info.preamp
            );

            let before = read_bank().expect("read failed");
            assert_eq!(before.len(), info.bands);
            assert!(before.iter().all(|b| b.ok), "every band should read back");
            for b in &before {
                println!("  band {}: {} Hz  Q {:.2}  {:+.2} dB  {}", b.band, b.freq, b.q, b.gain, b.typ);
            }

            // A pattern that is nothing like a default bank, so a no-op write can't pass.
            let probe: Vec<BandIn> = (0..info.bands)
                .map(|i| BandIn {
                    freq: 100.0 * (i as f64 + 1.0),
                    gain: if i % 2 == 0 { -3.5 } else { 2.5 },
                    q: 1.25,
                    typ: "PK".into(),
                })
                .collect();
            let w = write_bank(probe, -2.0).expect("write failed");
            println!("  wrote {}/{} bands, slot {:?}", w.acked, info.bands, w.active);
            let back = read_bank().expect("read-back failed");

            // Put the cable back before asserting anything. A failed assertion panics, and a
            // panic here would otherwise leave the probe bank on someone's cable.
            let restore: Vec<BandIn> = before
                .iter()
                .map(|b| BandIn { freq: b.freq as f64, gain: b.gain, q: b.q, typ: b.typ.clone() })
                .collect();
            let r = write_bank(restore, 0.0).expect("restore failed");
            println!("  restored {}/{} bands", r.acked, info.bands);
            close();

            assert_eq!(w.acked as usize, info.bands, "every band should verify");
            assert_eq!(back[0].freq, 100);
            assert!((back[0].gain - -3.5).abs() < 0.01);
            assert!((back[1].gain - 2.5).abs() < 0.01);
            assert_eq!(r.acked as usize, info.bands, "original bank should go back");
        }
    }

    // End-to-end against an attached FiiO device, through the UI's entry points. Uses the first
    // flat USER bank as a scratch bank, puts it back and returns to the bank the device started
    // on — so it won't touch an EQ you care about. Checks the *saved* copy (by switching away and
    // back), since a plain read returns the working copy even for bands that never reached flash.
    //   cargo test --lib -- --ignored --nocapture fiio_round_trip
    #[test]
    #[ignore = "needs a FiiO device attached; writes and restores a USER bank"]
    fn fiio_round_trip() {
        let found = list().expect("list failed");
        let d = found.iter().find(|d| d.gen == 3).expect("no FiiO device attached");
        let info = open(Some((d.vid as u16, d.pid as u16))).expect("open failed");

        let start = read_mode().expect("read_mode failed").expect("no active bank");
        println!("{} · {} bands · on {:?}", info.name, info.bands, info.presets.get(&(start as u8)));
        let users: Vec<i32> = info.user_slots.iter().map(|s| *s as i32).collect();
        // A flat USER bank if there is one; otherwise the last USER bank that isn't the one the
        // device started on, whose exact contents go back afterwards.
        let mut banks = Vec::new();
        for s in &users {
            assert_eq!(set_preset(*s).expect("switch failed"), Some(*s));
            let b = read_bank().expect("read failed");
            let pre = read_preamp().expect("preamp read failed").expect("no preamp");
            println!("  {:?}: {} active bands, preamp {:.1}", info.presets.get(&(*s as u8)),
                     b.iter().filter(|x| x.gain != 0.0).count(), pre);
            assert!(b.iter().all(|x| x.ok), "every band should read");
            banks.push((*s, b, pre));
        }
        let pick = banks
            .iter()
            .position(|(_, b, pre)| b.iter().all(|x| x.gain == 0.0) && *pre == 0.0)
            .or_else(|| banks.iter().rposition(|(s, _, _)| *s != start));
        let Some(k) = pick else {
            set_preset(start).ok();
            panic!("no USER bank to use as a scratch bank");
        };
        let (scratch, before, pre_before) = banks.swap_remove(k);
        println!("scratch bank: {:?}", info.presets.get(&(scratch as u8)));
        let away = *users.iter().find(|s| **s != scratch).expect("needs two USER banks");

        let probe: Vec<BandIn> = (0..info.bands)
            .map(|i| BandIn {
                freq: 100.0 * (i as f64 + 1.0),
                gain: if i % 2 == 0 { -3.5 } else { 2.5 },
                q: 1.25,
                typ: if i == 9 { "HSQ".into() } else { "PK".into() },
            })
            .collect();
        let t = std::time::Instant::now();
        let w = write_bank(probe, -4.2);
        println!("write + verify: {:?} -> {:?}", t.elapsed(), w.as_ref().map(|w| w.acked));
        // Independently of write_bank's own check: away and back reloads the saved copy.
        set_preset(away).ok();
        set_preset(scratch).ok();
        let back = read_bank();
        let pre_back = read_preamp();

        // Restore before asserting, so a failure can't leave the probe on the device.
        let restore: Vec<BandIn> = before
            .iter()
            .map(|b| BandIn { freq: b.freq as f64, gain: b.gain, q: b.q, typ: b.typ.clone() })
            .collect();
        let r = write_bank(restore, pre_before);
        let flat_again = read_bank();
        set_preset(start).ok();
        let end = read_mode();
        close();

        let w = w.expect("write failed");
        assert_eq!(w.acked as usize, info.bands, "every band should verify");
        assert_eq!(w.active, Some(scratch));
        let back = back.unwrap();
        assert_eq!((back[0].freq, back[0].gain), (100, -3.5));
        assert_eq!((back[9].freq, back[9].typ.as_str()), (1000, "HSQ"));
        // Rounded down to the device's step: -4.2 stays -4.2, or goes to -5 on a whole-dB part.
        let want_pre = ((-4.2 / info.preamp_step + 1e-9).floor() * info.preamp_step * 10.0).round() / 10.0;
        assert_eq!(pre_back.unwrap(), Some(want_pre));
        assert_eq!(r.expect("restore failed").acked as usize, info.bands);
        assert!(flat_again.unwrap().iter().zip(&before).all(|(a, b)| a.freq == b.freq && a.gain == b.gain && a.typ == b.typ));
        assert_eq!(end.unwrap(), Some(start), "should be back on the starting bank");
    }

    // A factory voicing can't be written; the write must refuse rather than land on it.
    #[test]
    #[ignore = "needs a FiiO device attached; switches bank and back, writes nothing"]
    fn fiio_refuses_to_write_a_factory_bank() {
        let found = list().expect("list failed");
        let d = found.iter().find(|d| d.gen == 3).expect("no FiiO device attached");
        open(Some((d.vid as u16, d.pid as u16))).expect("open failed");
        let start = read_mode().unwrap().unwrap();
        set_preset(0).expect("switch to Jazz failed");
        let jazz = read_bank().expect("read failed");
        let w = write_bank(
            jazz.iter().map(|b| BandIn { freq: b.freq as f64, gain: b.gain, q: b.q, typ: b.typ.clone() }).collect(),
            0.0,
        );
        set_preset(start).ok();
        close();
        let e = w.err().expect("writing a factory bank should be refused");
        assert!(e.contains("Pick a USER bank"), "{e}");
    }

    // The pre-fix shape, kept as a stress harness: a fresh short-lived thread per operation,
    // which is what tauri's spawn_blocking gave us. Worth knowing that this did *not*
    // reproduce the field crash on the bench — 300 rounds against a live cable under
    // MallocScribble came back clean — so it stands as a regression harness for the HID path
    // rather than as proof of the cause. Ignored by default: it needs a cable, it's slow, and
    // if it ever does fail the way the wild reports do it takes the test process down with
    // SIGTRAP rather than failing an assertion. Run it with:
    //   HID_ROUNDS=300 cargo test --release -- --ignored --nocapture old_shape
    #[test]
    #[ignore = "slow, needs a cable attached; run explicitly"]
    fn old_shape_churns_threads() {
        let rounds: usize = std::env::var("HID_ROUNDS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(10);
        for i in 0..rounds {
            let r = std::thread::spawn(|| {
                let dev = backend::open_best(None)?;
                Ok::<_, String>(dsp::read_bank(&dev).len())
            })
            .join()
            .expect("worker thread panicked");
            if i % 10 == 0 {
                println!("pass {i}: {r:?}");
            }
            // Recycle the freed run loops' memory.
            let mut keep: Vec<Vec<u8>> = Vec::new();
            for k in 0..2000 {
                let v = vec![0xABu8; 64 + (k % 1024)];
                if k % 3 == 0 {
                    keep.push(v);
                }
            }
            drop(keep);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}
