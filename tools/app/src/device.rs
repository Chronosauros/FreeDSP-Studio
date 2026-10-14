// Runs device I/O off the UI thread, so a write (about a second of frames) never freezes the
// window. Each request is a closure on a short-lived thread; it reports back through a channel
// the UI drains every frame. The HID layer itself serialises the actual device access.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use crate::eq::{round1, Band, FType};
use crate::hid;

/// What the device side reports back to the UI.
pub enum Reply {
    Listed(Result<Vec<hid::Listed>, String>),
    Opened(Result<hid::CableInfo, String>),
    Mode(Option<i32>),
    /// The bands on the cable (None where a band couldn't be read) and its preamp, if asked for.
    Read(Result<(Vec<Option<Band>>, Option<f64>), String>),
    /// After a write: the result, and the EQ that was sent.
    Wrote(Result<hid::WriteResult, String>, Vec<Band>),
    /// After switching preset: the active one, then the cable's EQ.
    Switched(Result<(), String>),
    /// A line for the bank window and the log.
    BankMsg(String, bool),
    BankWritten(BankWriteOutcome),
    BankRead(BankReadOutcome),
}

pub struct Worker {
    tx: Sender<Reply>,
    pub rx: Receiver<Reply>,
    busy: Arc<AtomicUsize>,
    pub stop: Arc<AtomicBool>,
    ctx: eframe::egui::Context,
}

impl Worker {
    pub fn new(ctx: eframe::egui::Context) -> Worker {
        let (tx, rx) = channel();
        Worker { tx, rx, busy: Arc::new(AtomicUsize::new(0)), stop: Arc::new(AtomicBool::new(false)), ctx }
    }

    pub fn busy(&self) -> bool {
        self.busy.load(Ordering::SeqCst) > 0
    }

    pub fn run(&self, f: impl FnOnce(&Sender<Reply>) + Send + 'static) {
        let tx = self.tx.clone();
        let busy = self.busy.clone();
        let ctx = self.ctx.clone();
        busy.fetch_add(1, Ordering::SeqCst);
        std::thread::spawn(move || {
            f(&tx);
            busy.fetch_sub(1, Ordering::SeqCst);
            ctx.request_repaint();
        });
    }

    pub fn list(&self) {
        self.run(|tx| {
            let _ = tx.send(Reply::Listed(hid::list()));
        });
    }

    pub fn open(&self, want: Option<(u16, u16)>) {
        self.run(move |tx| {
            let _ = tx.send(Reply::Opened(hid::open(want)));
        });
    }

    pub fn close(&self) {
        self.run(|_| hid::close());
    }

    pub fn read_mode(&self) {
        self.run(|tx| {
            let _ = tx.send(Reply::Mode(hid::read_mode().ok().flatten()));
        });
    }

    /// Reads the cable's EQ, and its preamp on devices that report one (gen 3 and up).
    pub fn read(&self, with_preamp: bool) {
        self.run(move |tx| {
            let _ = tx.send(Reply::Read(read_cable(with_preamp)));
        });
    }

    pub fn write(&self, bands: Vec<Band>, preamp: f64) {
        self.run(move |tx| {
            let r = hid::write_bank(to_in(&bands), preamp);
            let _ = tx.send(Reply::Wrote(r, bands));
        });
    }

    /// Switches preset, then reports the active one and the cable's EQ.
    pub fn switch(&self, idx: i32, with_preamp: bool) {
        self.run(move |tx| {
            match hid::set_preset(idx) {
                Ok(_) => {
                    let _ = tx.send(Reply::Switched(Ok(())));
                    let _ = tx.send(Reply::Mode(hid::read_mode().ok().flatten()));
                    let _ = tx.send(Reply::Read(read_cable(with_preamp)));
                }
                Err(e) => {
                    let _ = tx.send(Reply::Switched(Err(e)));
                }
            }
        });
    }
}

pub fn to_in(bands: &[Band]) -> Vec<hid::BandIn> {
    bands.iter().map(|b| hid::BandIn { freq: b.freq, gain: b.gain, q: b.q, typ: b.typ.code().to_string() }).collect()
}

fn from_hid(b: &hid::Band) -> Option<Band> {
    if !b.ok {
        return None;
    }
    Some(Band { freq: b.freq as f64, gain: b.gain, q: b.q, typ: FType::from_code(&b.typ) })
}

fn read_cable(with_preamp: bool) -> Result<(Vec<Option<Band>>, Option<f64>), String> {
    let bands = hid::read_bank()?;
    let pre = if with_preamp { hid::read_preamp()? } else { None };
    Ok((bands.iter().map(from_hid).collect(), pre))
}

/// One bank as the device holds it: whether every band read, the active bands, and the preamp.
pub struct DeviceBank {
    pub ok: bool,
    pub bands: Vec<Band>,
    pub preamp: f64,
}

pub fn read_device_bank() -> Result<DeviceBank, String> {
    let bands = hid::read_bank()?;
    let pre = hid::read_preamp()?;
    Ok(DeviceBank {
        ok: bands.iter().all(|b| b.ok),
        bands: bands.iter().filter(|b| b.ok && b.gain != 0.0).filter_map(from_hid).map(|b| Band { gain: round1(b.gain), ..b }).collect(),
        preamp: pre.map(round1).unwrap_or(0.0),
    })
}

fn same_bank(dev: &DeviceBank, bank: &crate::store::Bank) -> bool {
    if !dev.ok || dev.bands.len() != bank.bands.len() || (dev.preamp - bank.preamp).abs() > 0.05 {
        return false;
    }
    dev.bands.iter().zip(&bank.bands).all(|(d, b)| {
        d.freq == crate::eq::jround(b.freq) && (d.gain - b.gain).abs() < 0.05 && (d.q - b.q).abs() < 0.02 && d.typ == b.typ
    })
}

pub struct BankJob {
    /// (slot, its label, the bank) in write order.
    pub banks: Vec<(u8, String, crate::store::Bank)>,
    /// Go back to this bank afterwards (a whole-set write); None leaves the device on the last one.
    pub return_to: Option<i32>,
    /// What the device's bands are padded to.
    pub caps: crate::eq::Caps,
}

pub struct BankWriteOutcome {
    pub wrote: usize,
    pub same: usize,
    /// What each written bank held before, keyed by slot.
    pub before: Vec<(u8, crate::store::Bank)>,
    /// Banks that are now on the device, and the name they carry.
    pub names: Vec<(u8, String)>,
    pub err: Option<String>,
    pub stopped: bool,
    pub only_one: bool,
}

pub struct BankReadOutcome {
    pub banks: Vec<(u8, crate::store::Bank)>,
    pub err: Option<String>,
    pub stopped: bool,
}

impl Worker {
    /// Writes each bank unless the device already holds it, keeping what it overwrote.
    pub fn write_banks(&self, job: BankJob) {
        let stop = self.stop.clone();
        stop.store(false, Ordering::SeqCst);
        self.run(move |tx| {
            let total = job.banks.len();
            let mut out = BankWriteOutcome { wrote: 0, same: 0, before: Vec::new(), names: Vec::new(), err: None, stopped: false, only_one: job.return_to.is_none() };
            for (slot, label, bank) in &job.banks {
                if stop.load(Ordering::SeqCst) {
                    out.stopped = true;
                    break;
                }
                let _ = tx.send(Reply::BankMsg(format!("{}: switching and checking… ({} of {})", label, out.wrote + out.same + 1, total), false));
                let r = (|| -> Result<bool, String> {
                    hid::set_preset(*slot as i32)?;
                    let cur = read_device_bank()?;
                    if same_bank(&cur, bank) {
                        return Ok(false);
                    }
                    let r = hid::write_bank(to_in(&job.caps.fill(&bank.bands)), bank.preamp)?;
                    if (r.acked as usize) < job.caps.bands {
                        return Err(format!("{}: only {} of {} bands read back as written.", label, r.acked, job.caps.bands));
                    }
                    out.before.push((*slot, crate::store::Bank { name: String::new(), bands: cur.bands, preamp: cur.preamp }));
                    Ok(true)
                })();
                match r {
                    Ok(true) => out.wrote += 1,
                    Ok(false) => out.same += 1,
                    Err(e) => {
                        out.err = Some(e);
                        break;
                    }
                }
                out.names.push((*slot, bank.name.clone()));
            }
            if let (Some(s), None) = (job.return_to, &out.err) {
                let _ = hid::set_preset(s);
            }
            let _ = tx.send(Reply::BankWritten(out));
            let _ = tx.send(Reply::Mode(hid::read_mode().ok().flatten()));
        });
    }

    /// Reads every given bank, then returns to the starting one.
    pub fn read_banks(&self, slots: Vec<(u8, String)>, return_to: Option<i32>) {
        let stop = self.stop.clone();
        stop.store(false, Ordering::SeqCst);
        self.run(move |tx| {
            let mut out = BankReadOutcome { banks: Vec::new(), err: None, stopped: false };
            let total = slots.len();
            for (n, (slot, label)) in slots.iter().enumerate() {
                if stop.load(Ordering::SeqCst) {
                    out.stopped = true;
                    break;
                }
                let _ = tx.send(Reply::BankMsg(format!("Reading {}… ({} of {})", label, n + 1, total), false));
                match hid::set_preset(*slot as i32).and_then(|_| read_device_bank()) {
                    Ok(cur) => {
                        if !cur.bands.is_empty() || cur.preamp != 0.0 {
                            out.banks.push((*slot, crate::store::Bank { name: String::new(), bands: cur.bands, preamp: cur.preamp }));
                        }
                    }
                    Err(e) => {
                        out.err = Some(e);
                        break;
                    }
                }
            }
            if let Some(s) = return_to {
                let _ = hid::set_preset(s);
            }
            let _ = tx.send(Reply::BankRead(out));
            let _ = tx.send(Reply::Mode(hid::read_mode().ok().flatten()));
        });
    }
}
