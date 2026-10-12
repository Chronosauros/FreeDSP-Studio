// FiiO HID transport, via hidapi on every platform. Plain interrupt reads and writes, like
// gen-2; the protocol lives in dsp_fiio.rs.
//
// A FiiO device exposes several HID collections and only one of them carries report 7: the
// generic-desktop one (usage page 0x01, usage 0), with 32-byte input and output reports.
// Which *interface* that collection sits on is not fixed — a BTR17 with its EQ switched off
// enumerates an extra vendor interface (an MQA decoder status report) and the control
// collection moves from interface 3 to 4. So it is found by usage, never by interface number.

use hidapi::HidDevice;

use crate::dsp_fiio::{TransportFiio, REPORT_ID, REPORT_LEN};
use crate::hid_api::with_api;

const USAGE_PAGE: u16 = 0x0001;
const USAGE: u16 = 0x0000;

pub struct Dev {
    dev: HidDevice,
}

impl TransportFiio for Dev {
    fn send(&self, frame: &[u8]) -> bool {
        if frame.len() > REPORT_LEN {
            return false;
        }
        // Always a whole report: Windows pads short writes itself, macOS doesn't.
        let mut buf = [0u8; REPORT_LEN + 1];
        buf[0] = REPORT_ID;
        buf[1..1 + frame.len()].copy_from_slice(frame);
        self.dev.write(&buf).is_ok()
    }

    fn recv(&self, timeout_ms: u64) -> Option<Vec<u8>> {
        let mut buf = [0u8; 64];
        match self.dev.read_timeout(&mut buf, timeout_ms.min(i32::MAX as u64) as i32) {
            Ok(n) if n > 1 && buf[0] == REPORT_ID => Some(buf[1..n].to_vec()),
            // A report on another id (the consumer-control keys) is not an answer, but the
            // caller's deadline is what ends the wait, so hand back an empty one to loop on.
            Ok(n) if n > 0 => Some(Vec::new()),
            _ => None,
        }
    }

    fn drain(&self) {
        let mut buf = [0u8; 64];
        while let Ok(n) = self.dev.read_timeout(&mut buf, 0) {
            if n == 0 {
                return;
            }
        }
    }
}

pub fn open(vid: u16, pid: u16) -> Result<Dev, String> {
    with_api(|api| {
        let found = api.device_list().find(|d| {
            d.vendor_id() == vid
                && d.product_id() == pid
                && d.usage_page() == USAGE_PAGE
                && d.usage() == USAGE
                && !d.path().to_bytes().is_empty()
        });
        let Some(d) = found else {
            return Err(format!("no FiiO control interface at {:04x}:{:04x}", vid, pid));
        };
        api.open_path(d.path())
            .map(|dev| Dev { dev })
            .map_err(|e| format!("could not open {:04x}:{:04x}: {}", vid, pid, e))
    })
}
