// Gen-2 HID transport, via hidapi on every platform.
//
// Gen-1 needs control-pipe SET_REPORT/GET_REPORT, which is why there is a hand-rolled Win32
// backend for it. Gen-2 needs neither: it is plain interrupt OUT and IN, which hid_write and
// hid_read express identically on Windows, macOS and Linux. So this backend is shared, and
// the Win32 code stays a gen-1 concern.
//
// Replies arriving on the interrupt endpoint is also why these cables can be driven from a
// browser where gen-1 cannot: WebHID can send an output report and listen for input reports,
// but cannot issue a control-pipe GET_REPORT at all.

use hidapi::HidDevice;

use crate::dsp2::{Transport2, FRAME_LEN, REPORT_ID};
use crate::hid_api::with_api;

pub struct Dev {
    dev: HidDevice,
}

impl Transport2 for Dev {
    fn send(&self, frame: &[u8]) -> bool {
        self.dev.write(frame).is_ok()
    }

    fn drain(&self) {
        // Non-blocking: pull until the queue is empty. Anything still here answers a request
        // that has already been given up on, and would otherwise be mistaken for the reply to
        // the next one.
        loop {
            let mut buf = [0u8; FRAME_LEN + 1];
            match self.dev.read_timeout(&mut buf, 0) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
        }
    }

    fn recv(&self, cmd: u8, timeout_ms: u64) -> Option<Vec<u8>> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
        loop {
            let mut buf = [0u8; FRAME_LEN + 1];
            match self.dev.read_timeout(&mut buf, 100) {
                // The cable also pushes its volume/transport keys on report 3, and a reply to
                // an earlier command can still be in flight, so match on both the report id
                // and the echoed CMD byte rather than taking the first report that arrives.
                Ok(n) if n > 2 && buf[0] == REPORT_ID && buf[2] == cmd => {
                    return Some(buf[1..n].to_vec())
                }
                Ok(_) => {}
                Err(_) => return None,
            }
            if std::time::Instant::now() >= deadline {
                return None;
            }
        }
    }
}

/// Opens one candidate by (vid, pid). Collections of a single device share a path on macOS,
/// so the first path that opens is the right one — report 0x4B routes to the same endpoint
/// whichever collection you came in through.
pub fn open(vid: u16, pid: u16) -> Result<Dev, String> {
    with_api(|api| {
        let mut last = String::new();
        let mut tried: Vec<std::ffi::CString> = Vec::new();
        for d in api
            .device_list()
            .filter(|d| d.vendor_id() == vid && d.product_id() == pid)
            .filter(|d| !d.path().to_bytes().is_empty())
        {
            if tried.contains(&d.path().to_owned()) {
                continue;
            }
            tried.push(d.path().to_owned());
            match api.open_path(d.path()) {
                Ok(dev) => return Ok(Dev { dev }),
                Err(e) => last = e.to_string(),
            }
        }
        Err(if last.is_empty() {
            format!("no device at {:04x}:{:04x}", vid, pid)
        } else {
            format!("could not open {:04x}:{:04x}: {}", vid, pid, last)
        })
    })
}

/// Every attached device on a vendor id we know about, as (vid, pid, product), de-duplicated
/// across the collections that share one physical cable.
pub fn candidates(vids: &[u16]) -> Result<Vec<(u16, u16, String)>, String> {
    with_api(|api| {
        let mut out: Vec<(u16, u16, String)> = Vec::new();
        for d in api
            .device_list()
            .filter(|d| vids.contains(&d.vendor_id()) && !d.path().to_bytes().is_empty())
        {
            let key = (d.vendor_id(), d.product_id());
            if out.iter().any(|(v, p, _)| (*v, *p) == key) {
                continue;
            }
            out.push((
                d.vendor_id(),
                d.product_id(),
                d.product_string().unwrap_or("").to_string(),
            ));
        }
        Ok(out)
    })
}
