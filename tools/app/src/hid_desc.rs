// HID report descriptor parsing — platform-free.
//
// Only one question is asked of a descriptor: which report ids does this device declare, and
// how long is each one in each direction. That is enough to answer two things the protocol
// tables can't be trusted to: which report a FiiO part talks on (7 on most, 1 on the KA17, 2 on
// JadeAudio parts — and the KA15 puts it on a Consumer collection beside a Walkplay report
// 0x4B), and how long its reports are (32 bytes on the BTR17, 63 on the KA15). macOS sends a
// write of the wrong length as-is and the device ignores it, so the length has to be right.
//
// It also gates detection. Walkplay chips ship under a long tail of vendor ids, some of them
// generic (C-Media's 0x0D8C is on every cheap USB sound card), and a gen-2 handshake is only
// sent to a device that actually declares report 0x4B.

/// Lengths of one report id, in bytes, report id excluded. Zero means not declared.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub id: u8,
    pub input: usize,
    pub output: usize,
    pub feature: usize,
}

/// Every report id the descriptor declares, with its lengths. A descriptor that uses no report
/// ids at all comes back as a single report 0.
pub fn reports(desc: &[u8]) -> Vec<Report> {
    // Bits accumulated per (report id, direction): 0 input, 1 output, 2 feature.
    let mut bits: Vec<(u8, [usize; 3])> = Vec::new();
    let mut add = |id: u8, dir: usize, n: usize| match bits.iter_mut().find(|(i, _)| *i == id) {
        Some((_, b)) => b[dir] += n,
        None => {
            let mut b = [0; 3];
            b[dir] = n;
            bits.push((id, b));
        }
    };

    // Global state, with Push/Pop.
    let (mut id, mut size, mut count) = (0u8, 0usize, 0usize);
    let mut stack: Vec<(u8, usize, usize)> = Vec::new();

    let mut i = 0;
    while i < desc.len() {
        let prefix = desc[i];
        if prefix == 0xFE {
            // Long item: skip it whole.
            let len = *desc.get(i + 1).unwrap_or(&0) as usize;
            i += 3 + len;
            continue;
        }
        let len = match prefix & 0x03 {
            3 => 4,
            n => n as usize,
        };
        let mut v: u32 = 0;
        for k in 0..len {
            v |= (*desc.get(i + 1 + k).unwrap_or(&0) as u32) << (8 * k);
        }
        match prefix & 0xFC {
            0x84 => id = v as u8,           // Report ID
            0x74 => size = v as usize,      // Report Size
            0x94 => count = v as usize,     // Report Count
            0xA4 => stack.push((id, size, count)),
            0xB4 => {
                if let Some(s) = stack.pop() {
                    (id, size, count) = s;
                }
            }
            0x80 => add(id, 0, size * count), // Input
            0x90 => add(id, 1, size * count), // Output
            0xB0 => add(id, 2, size * count), // Feature
            _ => {}
        }
        i += 1 + len;
    }

    let bytes = |b: usize| b.div_ceil(8);
    bits.into_iter()
        .map(|(id, b)| Report { id, input: bytes(b[0]), output: bytes(b[1]), feature: bytes(b[2]) })
        .collect()
}

/// The report with this id, if declared.
#[cfg(test)]
pub fn find(desc: &[u8], id: u8) -> Option<Report> {
    reports(desc).into_iter().find(|r| r.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    // The KA15's whole descriptor, from ioreg: media keys on 3, a Walkplay report on 0x4B and
    // FiiO's on 7, both 63 bytes each way.
    #[test]
    fn ka15_declares_both_protocols() {
        let d = hex("050c0901a1018503150025037501950209e909ea8102950409cd09cf09b509b68102950281010601ff854b7508953f09018103953f0902910285077508953f09038103953f09049102c0");
        let r = reports(&d);
        assert_eq!(find(&d, 7), Some(Report { id: 7, input: 63, output: 63, feature: 0 }));
        assert_eq!(find(&d, 0x4B), Some(Report { id: 0x4B, input: 63, output: 63, feature: 0 }));
        assert_eq!(find(&d, 3).map(|r| (r.input, r.output)), Some((1, 0)));
        assert_eq!(r.len(), 3);
    }

    #[test]
    fn push_and_pop_restore_globals() {
        // id 1: 8 bits x 4 out; push, id 2: 16 x 2 out; pop; another 4 bytes on id 1.
        let d = [0x85, 1, 0x75, 8, 0x95, 4, 0x91, 2, 0xA4, 0x85, 2, 0x75, 16, 0x95, 2, 0x91, 2, 0xB4, 0x91, 2];
        assert_eq!(find(&d, 1).unwrap().output, 8);
        assert_eq!(find(&d, 2).unwrap().output, 4);
    }

    #[test]
    fn a_descriptor_without_ids_is_report_zero() {
        let d = [0x06, 0x00, 0xFF, 0x09, 1, 0xA1, 1, 0x75, 8, 0x95, 64, 0x81, 2, 0x91, 2, 0xC0];
        assert_eq!(reports(&d), vec![Report { id: 0, input: 64, output: 64, feature: 0 }]);
    }
}
