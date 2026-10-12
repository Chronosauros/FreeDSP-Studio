// FiiO HID transport: the shared report transport, pointed at the FiiO control report. The
// protocol lives in dsp_fiio.rs.
//
// Which collection, report id and report length a FiiO part uses all vary by model — a BTR17
// moves its control collection from interface 3 to 4 when its EQ is switched off, and the KA15
// has no generic-desktop collection at all — so all three come from the descriptor.

use crate::dsp_fiio::{TransportFiio, REPORT_IDS};
use crate::hid_report::{self, Via};

pub use crate::hid_report::Dev;

impl TransportFiio for Dev {
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

/// Opens the device's FiiO control report, trying the model's own report id first.
pub fn open(vid: u16, pid: u16, prefer: u8) -> Result<Dev, String> {
    let mut ids = vec![prefer];
    ids.extend(REPORT_IDS.iter().copied().filter(|i| *i != prefer));
    hid_report::open(vid, pid, &ids, Via::Interrupt)
}
