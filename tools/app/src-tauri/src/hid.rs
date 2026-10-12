// Public HID API. Picks a platform transport, serialises access to the cable, and refuses
// to write to a cable that isn't the one the user connected. The protocol itself lives in
// dsp.rs; everything here is bookkeeping.

use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, OnceLock};

use crate::dsp;
use crate::dsp2;
use crate::dsp_fiio;
use crate::hid_fiio;
use crate::hid_gen2;

pub use crate::dsp::{Band, BandIn, CableInfo, WriteResult};

/// Vendor ids we look for. 0x35D8 is Moondrop (both generations); 0x3302 is CrinEar, whose
/// cables are Walkplay-chipset parts speaking the same gen-2 protocol.
pub const VIDS: [u16; 2] = [0x35D8, 0x3302];

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
        let dev = hid_fiio::open(exp.vid, exp.pid).map_err(|e| {
            format!("{} is no longer connected: {}. Nothing was written.", exp.name, e)
        })?;
        Ok(f(&dev, &profile))
    })?
}

/// A FiiO device we have a profile for and that answers the handshake (an active-bank read).
fn probe_fiio(vid: u16, pid: u16, product: &str) -> Option<dsp_fiio::Profile> {
    let profile = dsp_fiio::profile(vid, pid, product)?;
    let dev = hid_fiio::open(vid, pid).ok()?;
    dsp_fiio::read_preset(&dev)?;
    Some(profile)
}

fn all_vids() -> Vec<u16> {
    VIDS.iter().chain(dsp_fiio::VIDS.iter()).copied().collect()
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
            if dsp_fiio::VIDS.contains(&vid) {
                if let Some(p) = probe_fiio(vid, pid, &product) {
                    out.push(Listed { vid: vid as u32, pid: pid as u32, name: p.name, gen: 3 });
                }
                continue;
            }
            let gen2 = hid_gen2::open(vid, pid)
                .ok()
                .and_then(|d| dsp2::read_version(&d));
            if gen2.is_some() {
                let p = dsp2::cable_profile(vid, pid, &product);
                out.push(Listed { vid: vid as u32, pid: pid as u32, name: p.name, gen: 2 });
            } else if vid == dsp::MOONDROP_VID {
                let (name, _, _) = dsp::cable_profile(pid, &product);
                out.push(Listed { vid: vid as u32, pid: pid as u32, name, gen: 1 });
            }
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

        // FiiO devices are told apart by vendor id, and never sent the gen-2 probe.
        for (vid, pid, product) in candidates.iter().filter(|c| dsp_fiio::VIDS.contains(&c.0)) {
            let Some(p) = probe_fiio(*vid, *pid, product) else {
                continue;
            };
            *EXPECTED.lock().unwrap() = Some(Expected {
                vid: *vid,
                pid: *pid,
                name: p.name.clone(),
                kind: Kind::Fiio(p.clone()),
            });
            return Ok(CableInfo {
                name: p.name.clone(),
                vid: *vid as u32,
                pid: *pid as u32,
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
            });
        }
        candidates.retain(|c| !dsp_fiio::VIDS.contains(&c.0));

        // Gen-2 first: probe each candidate for the report-0x4B handshake.
        for (vid, pid, product) in &candidates {
            let dev = match hid_gen2::open(*vid, *pid) {
                Ok(d) => d,
                Err(_) => continue,
            };
            let Some(firmware) = dsp2::read_version(&dev) else {
                continue;
            };
            let mut profile = dsp2::cable_profile(*vid, *pid, product);
            // An uncharacterised cable tells us its editable slot rather than us guessing.
            if profile.presets.is_empty() {
                let slot = dsp2::read_slot(&dev).unwrap_or(0);
                profile.custom_slot = slot;
                profile.presets.insert(slot, "Custom EQ".to_string());
            }
            *EXPECTED.lock().unwrap() = Some(Expected {
                vid: *vid,
                pid: *pid,
                name: profile.name.clone(),
                kind: Kind::Gen2(profile.clone()),
            });
            return Ok(CableInfo {
                name: profile.name.clone(),
                vid: *vid as u32,
                pid: *pid as u32,
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
            });
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
        })
    })?
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

    // End-to-end against an attached FiiO device, through the UI's entry points. Uses USER7
    // as a scratch bank, and only if it is flat, then puts it back and returns to the bank the
    // device started on — so it won't touch an EQ you care about.
    //   cargo test --lib -- --ignored --nocapture fiio_round_trip
    #[test]
    #[ignore = "needs a FiiO BTR17 attached; writes and restores its USER7 bank"]
    fn fiio_round_trip() {
        const SCRATCH: i32 = 166; // USER7 on a BTR17
        let found = list().expect("list failed");
        let d = found.iter().find(|d| d.gen == 3).expect("no FiiO device attached");
        let info = open(Some((d.vid as u16, d.pid as u16))).expect("open failed");
        assert_eq!(info.presets.get(&(SCRATCH as u8)).map(String::as_str), Some("USER7"));

        let start = read_mode().expect("read_mode failed").expect("no active bank");
        println!("{} · {} bands · on {:?}", info.name, info.bands, info.presets.get(&(start as u8)));
        let t = std::time::Instant::now();
        assert_eq!(set_preset(SCRATCH).expect("switch failed"), Some(SCRATCH));
        println!("switch + settle: {:?}", t.elapsed());

        let t = std::time::Instant::now();
        let before = read_bank().expect("read failed");
        println!("read: {:?}", t.elapsed());
        let pre_before = read_preamp().expect("preamp read failed").expect("no preamp");
        assert!(before.iter().all(|b| b.ok));
        if before.iter().any(|b| b.gain != 0.0) || pre_before != 0.0 {
            set_preset(start).ok();
            panic!("USER7 isn't flat, so it isn't safe to use as a scratch bank");
        }

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
        assert_eq!(w.active, Some(SCRATCH));
        let back = back.unwrap();
        assert_eq!((back[0].freq, back[0].gain), (100, -3.5));
        assert_eq!((back[9].freq, back[9].typ.as_str()), (1000, "HSQ"));
        assert_eq!(pre_back.unwrap(), Some(-4.2));
        assert_eq!(r.expect("restore failed").acked as usize, info.bands);
        assert!(flat_again.unwrap().iter().zip(&before).all(|(a, b)| a.freq == b.freq && a.gain == b.gain));
        assert_eq!(end.unwrap(), Some(start), "should be back on the starting bank");
    }

    // A factory voicing can't be written; the write must refuse rather than land on it.
    #[test]
    #[ignore = "needs a FiiO BTR17 attached; switches bank and back, writes nothing"]
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
