// A report-addressed HID transport, via hidapi on every platform. Shared by the protocols that
// sit on one known report id and need nothing cleverer than "send this report, wait for one
// back": FiiO (report 7, 1 or 2), KT Micro (report 0x4B) and Fosi Audio (feature report 1).
//
// What it adds over a bare HidDevice is that the collection, the report id and the report
// lengths all come from the device's own descriptor (see hid_desc.rs), never from interface
// numbers or a table. A FiiO BTR17 moves its control collection between interfaces depending
// on whether its EQ is switched on; the KA15 has no generic-desktop collection at all and
// carries its control report on the same Consumer collection as its media keys.

use hidapi::{HidApi, HidDevice};

use crate::hid_api::with_api;
use crate::hid_desc;

pub struct Dev {
    dev: HidDevice,
    id: u8,
    out_len: usize,
    feat_len: usize,
}

/// Which way a protocol talks, so `open` can insist the chosen report supports it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Via {
    /// Output reports out, input reports back (interrupt endpoints).
    Interrupt,
    /// Feature reports both ways (control pipe).
    Feature,
}

impl Dev {
    pub fn report_id(&self) -> u8 {
        self.id
    }

    /// Sends one output report: the payload, zero-padded to the declared length. Always a whole
    /// report — Windows pads short writes itself, macOS sends exactly what it is handed.
    pub fn send(&self, payload: &[u8]) -> bool {
        if payload.len() > self.out_len {
            return false;
        }
        let mut buf = vec![0u8; self.out_len + 1];
        buf[0] = self.id;
        buf[1..1 + payload.len()].copy_from_slice(payload);
        self.dev.write(&buf).is_ok()
    }

    /// Waits up to `timeout_ms` for one input report. Returns it with the report id stripped,
    /// an empty Vec for a report on some other id (media keys) so the caller's deadline keeps
    /// running, or None on timeout.
    pub fn recv(&self, timeout_ms: u64) -> Option<Vec<u8>> {
        let mut buf = [0u8; 1025];
        match self.dev.read_timeout(&mut buf, timeout_ms.min(i32::MAX as u64) as i32) {
            Ok(n) if n > 1 && buf[0] == self.id => Some(buf[1..n].to_vec()),
            Ok(n) if n > 0 => Some(Vec::new()),
            _ => None,
        }
    }

    /// Throws away input reports still queued from an earlier request.
    pub fn drain(&self) {
        let mut buf = [0u8; 1025];
        while let Ok(n) = self.dev.read_timeout(&mut buf, 0) {
            if n == 0 {
                return;
            }
        }
    }

    /// SET_REPORT(Feature), zero-padded to the declared length.
    pub fn send_feature(&self, payload: &[u8]) -> bool {
        if payload.len() > self.feat_len {
            return false;
        }
        let mut buf = vec![0u8; self.feat_len + 1];
        buf[0] = self.id;
        buf[1..1 + payload.len()].copy_from_slice(payload);
        self.dev.send_feature_report(&buf).is_ok()
    }

    /// GET_REPORT(Feature), report id stripped.
    pub fn get_feature(&self) -> Option<Vec<u8>> {
        let mut buf = vec![0u8; self.feat_len + 1];
        buf[0] = self.id;
        match self.dev.get_feature_report(&mut buf) {
            Ok(n) if n > 1 => {
                // Some platforms hand back the id in byte 0, some don't; the length says which.
                let start = if n > self.feat_len { 1 } else { 0 };
                Some(buf[start..n].to_vec())
            }
            _ => None,
        }
    }
}

/// The report ids and lengths a device declares, for the protocol layer to choose from.
pub fn reports(vid: u16, pid: u16) -> Result<Vec<hid_desc::Report>, String> {
    with_api(|api| {
        let mut last = format!("no device at {:04x}:{:04x}", vid, pid);
        for d in paths(api, vid, pid) {
            match api.open_path(&d) {
                Ok(dev) => {
                    let mut buf = [0u8; 4096];
                    if let Ok(n) = dev.get_report_descriptor(&mut buf) {
                        return Ok(hid_desc::reports(&buf[..n]));
                    }
                }
                Err(e) => last = format!("could not open {:04x}:{:04x}: {}", vid, pid, e),
            }
        }
        Err(last)
    })
}

/// Opens the collection that declares the first of `ids` usable `via` this direction.
pub fn open(vid: u16, pid: u16, ids: &[u8], via: Via) -> Result<Dev, String> {
    with_api(|api| {
        let mut last = format!("no device at {:04x}:{:04x}", vid, pid);
        // Every collection with its declared reports, so the preferred id wins wherever it is.
        let mut opened: Vec<(HidDevice, Vec<hid_desc::Report>)> = Vec::new();
        for path in paths(api, vid, pid) {
            let dev = match api.open_path(&path) {
                Ok(d) => d,
                Err(e) => {
                    last = format!("could not open {:04x}:{:04x}: {}", vid, pid, e);
                    continue;
                }
            };
            let mut buf = [0u8; 4096];
            let Ok(n) = dev.get_report_descriptor(&mut buf) else { continue };
            opened.push((dev, hid_desc::reports(&buf[..n])));
        }
        for want in ids {
            let usable = |r: &&hid_desc::Report| {
                r.id == *want
                    && match via {
                        Via::Interrupt => r.output > 0 && r.input > 0,
                        Via::Feature => r.feature > 0,
                    }
            };
            if let Some(k) = opened.iter().position(|(_, rs)| rs.iter().any(|r| usable(&r))) {
                let (dev, rs) = opened.swap_remove(k);
                let r = *rs.iter().find(|r| usable(r)).expect("just found");
                return Ok(Dev { dev, id: r.id, out_len: r.output, feat_len: r.feature });
            }
        }
        if !opened.is_empty() {
            last = format!(
                "{:04x}:{:04x} has no control report {}",
                vid,
                pid,
                ids.iter().map(|i| format!("{:#04x}", i)).collect::<Vec<_>>().join("/")
            );
        }
        Err(last)
    })
}

/// Distinct paths for one vid:pid. Collections of one device share a path on macOS, and get
/// one each on Windows, where each carries only its own reports.
fn paths(api: &HidApi, vid: u16, pid: u16) -> Vec<std::ffi::CString> {
    let mut out: Vec<std::ffi::CString> = Vec::new();
    for d in api.device_list() {
        if d.vendor_id() == vid && d.product_id() == pid && !d.path().to_bytes().is_empty() {
            let p = d.path().to_owned();
            if !out.contains(&p) {
                out.push(p);
            }
        }
    }
    out
}

impl crate::dsp_kt::TransportKt for Dev {
    fn send(&self, frame: &[u8]) -> bool {
        Dev::send(self, frame)
    }
    fn recv(&self, timeout_ms: u64) -> Option<Vec<u8>> {
        Dev::recv(self, timeout_ms)
    }
    fn drain(&self) {
        Dev::drain(self)
    }
}

impl crate::dsp_fosi::TransportFosi for Dev {
    fn send_feature(&self, payload: &[u8]) -> bool {
        Dev::send_feature(self, payload)
    }
    fn get_feature(&self) -> Option<Vec<u8>> {
        Dev::get_feature(self)
    }
    fn send_output(&self, payload: &[u8]) -> bool {
        if self.out_len > 0 {
            Dev::send(self, payload)
        } else {
            Dev::send_feature(self, payload)
        }
    }
}
