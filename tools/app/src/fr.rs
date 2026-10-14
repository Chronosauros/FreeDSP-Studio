// Frequency-response curves: reading measurement and target files, interpolation, smoothing,
// and the treble correction a listening-test profile applies above ~6 kHz.

use crate::eq::clamp;

pub type Curve = Vec<[f64; 2]>;

/// "freq dB" per line (squig / AutoEQ style), separated by spaces, tabs, commas or semicolons.
/// Header lines and anything else that isn't a pair of numbers is skipped.
pub fn parse_fr(text: &str) -> Curve {
    let mut pts: Curve = Vec::new();
    for ln in text.lines() {
        let t = ln.trim();
        let Some((f, rest)) = take_num(t, false) else { continue };
        let sep = rest.chars().take_while(|c| c.is_whitespace() || *c == ',' || *c == ';').count();
        if sep == 0 {
            continue;
        }
        let rest = &rest[rest.char_indices().nth(sep).map(|(i, _)| i).unwrap_or(rest.len())..];
        let Some((d, _)) = take_num(rest, true) else { continue };
        if f > 0.0 && f.is_finite() && d.is_finite() {
            pts.push([f, d]);
        }
    }
    pts.sort_by(|a, b| a[0].partial_cmp(&b[0]).unwrap_or(std::cmp::Ordering::Equal));
    pts
}

/// A leading number: -?[\d.]+ for the frequency, -?[\d.eE+]+ for the level (as the app always
/// read them, so "1e3" is a level but not a frequency).
fn take_num(s: &str, exp: bool) -> Option<(f64, &str)> {
    let mut end = 0;
    for (i, c) in s.char_indices() {
        let ok = c.is_ascii_digit() || c == '.' || (i == 0 && c == '-') || (exp && (c == 'e' || c == 'E' || c == '+'));
        if ok {
            end = i + 1;
        } else {
            break;
        }
    }
    if end == 0 {
        return None;
    }
    // parseFloat reads the longest valid prefix
    let mut v = None;
    for k in (1..=end).rev() {
        if let Ok(x) = s[..k].parse::<f64>() {
            v = Some(x);
            break;
        }
    }
    v.map(|x| (x, &s[end..]))
}

/// Log-frequency linear interpolation; flat beyond the ends.
pub fn interp(pts: &[[f64; 2]], f: f64) -> f64 {
    if pts.is_empty() {
        return 0.0;
    }
    if f <= pts[0][0] {
        return pts[0][1];
    }
    let last = pts[pts.len() - 1];
    if f >= last[0] {
        return last[1];
    }
    let (mut lo, mut hi) = (0usize, pts.len() - 1);
    while hi - lo > 1 {
        let mid = (lo + hi) >> 1;
        if pts[mid][0] < f {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let ([f0, d0], [f1, d1]) = (pts[lo], pts[hi]);
    let t = (f.ln() - f0.ln()) / (f1.ln() - f0.ln());
    d0 + (d1 - d0) * t
}

/// Shifted so it reads 0 dB at 1 kHz.
pub fn normalize(pts: &[[f64; 2]]) -> Curve {
    let at = interp(pts, 1000.0);
    pts.iter().map(|p| [p[0], p[1] - at]).collect()
}

/// Broad version of a curve: running median over an octave (a narrow coupler peak is under half
/// the window, so it barely moves the median), then a 1/3-octave mean to round off the steps.
pub fn smoothed(pts: &[[f64; 2]]) -> Curve {
    const N: usize = 480;
    let per = N as f64 / 1000f64.log2();
    let hw = crate::eq::jround(per / 2.0) as usize;
    let hm = crate::eq::jround(per / 6.0) as usize;
    let mut fs = Vec::with_capacity(N + 1);
    let mut raw = Vec::with_capacity(N + 1);
    for i in 0..=N {
        let f = 20.0 * 1000f64.powf(i as f64 / N as f64);
        fs.push(f);
        raw.push(interp(pts, f));
    }
    let med: Vec<f64> = (0..=N)
        .map(|i| {
            let mut w: Vec<f64> = raw[i.saturating_sub(hw)..=(i + hw).min(N)].to_vec();
            w.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            w[w.len() >> 1]
        })
        .collect();
    fs.iter()
        .enumerate()
        .map(|(i, &f)| {
            let (a, b) = (i.saturating_sub(hm), (i + hm).min(N));
            let s: f64 = med[a..=b].iter().sum();
            [f, s / (b - a + 1) as f64]
        })
        .collect()
}

/// One channel of a stereo measurement, named the way squig downloads them ("Name [1].txt" /
/// "Name [2].txt") or the usual alternatives: L/R, (1)/(2), _L/-R, left/right. Returns the name
/// without the channel part and which channel it was ('L' or 'R').
pub fn split_channel(file_name: &str) -> Option<(String, char)> {
    let base = strip_ext(file_name).trim().to_string();
    let lower = base.to_lowercase();
    const CH: [&str; 6] = ["1", "2", "l", "r", "left", "right"];
    let ch_of = |c: &str| if c == "1" || c == "l" || c == "left" { 'L' } else { 'R' };
    // bracketed: "[1]", "(R)"
    if lower.ends_with(']') || lower.ends_with(')') {
        let open = if lower.ends_with(']') { '[' } else { '(' };
        if let Some(i) = lower.rfind(open) {
            let inner = lower[i + 1..lower.len() - 1].trim();
            if CH.contains(&inner) {
                let name = base[..i].trim();
                if !name.is_empty() {
                    return Some((name.to_string(), ch_of(inner)));
                }
            }
        }
        // a mismatched bracket pair, e.g. "[1)", is accepted too
        for o in ['[', '('] {
            if let Some(i) = lower.rfind(o) {
                let inner = lower[i + 1..lower.len() - 1].trim();
                if CH.contains(&inner) && !base[..i].trim().is_empty() {
                    return Some((base[..i].trim().to_string(), ch_of(inner)));
                }
            }
        }
    }
    // separated: " L", "_R", "-left"
    for w in ["left", "right", "l", "r"] {
        if lower.ends_with(w) {
            let cut = lower.len() - w.len();
            let pre = &lower[..cut];
            let seps = pre.chars().rev().take_while(|c| c.is_whitespace() || *c == '_' || *c == '-').count();
            if seps > 0 {
                let name = base[..cut - seps].trim();
                if !name.is_empty() {
                    return Some((name.to_string(), ch_of(w)));
                }
            }
        }
    }
    None
}

pub fn strip_ext(name: &str) -> &str {
    match name.rfind('.') {
        Some(i) if i > 0 => &name[..i],
        _ => name,
    }
}

/// A curve's display name from its file name: no extension, no trailing "[...]" tag.
pub fn name_from_file(file: &str) -> String {
    let mut n = strip_ext(file).trim().to_string();
    if n.ends_with(']') {
        if let Some(i) = n.rfind('[') {
            n = n[..i].trim().to_string();
        }
    }
    if n.is_empty() {
        "imported".into()
    } else {
        n
    }
}

/// Squig plots the left/right average (of amplitude, then back to dB), so dropping both channels
/// gives the same curve. Averaged on the first curve's frequency points.
pub fn average(curves: &[Curve]) -> Curve {
    curves[0]
        .iter()
        .map(|p| {
            let f = p[0];
            let s: f64 = curves.iter().map(|c| 10f64.powf(interp(c, f) / 20.0)).sum();
            [f, 20.0 * (s / curves.len() as f64).log10()]
        })
        .collect()
}

// ---- treble correction (listening-test profile) ----
// Above ~6 kHz a coupler's fine detail doesn't match anyone's ear (the 711's 8 kHz peak is the
// classic case), so with a profile active the fit stops trusting it there:
//   - the measurement and target are both heavily smoothed, so only their broad level and shape
//     (the target's "gist") survive and coupler peaks drop out, and
//   - the listening test supplies the narrow peaks and dips as your ear hears this IEM + tip.
// Below TT_X0 nothing changes; between TT_X0 and TT_X1 it crossfades.

pub const TT_BANDS: [f64; 7] = [6300.0, 7100.0, 8000.0, 9000.0, 10000.0, 11200.0, 12500.0];
pub const TT_X0: f64 = 5000.0;
pub const TT_X1: f64 = 6300.0;
pub const TT_HI: f64 = 16000.0;

pub fn log_ramp(f: f64, a: f64, b: f64) -> f64 {
    if f <= a {
        0.0
    } else if f >= b {
        1.0
    } else {
        (f / a).ln() / (b / a).ln()
    }
}

/// What the EQ should add at f according to the profile, tapered to 0 outside the tested bands.
pub fn treble_corr_at(prof: &[[f64; 2]], f: f64) -> f64 {
    let (f0, f1) = (prof[0][0], prof[prof.len() - 1][0]);
    if f <= TT_X0 || f >= TT_HI {
        return 0.0;
    }
    if f < f0 {
        return prof[0][1] * log_ramp(f, TT_X0, f0);
    }
    if f > f1 {
        return prof[prof.len() - 1][1] * (1.0 - log_ramp(f, f1, TT_HI));
    }
    interp(prof, f)
}

/// A curve with its smoothed copy kept beside it, since the treble maths asks for it often.
#[derive(Clone, Debug, Default)]
pub struct Smoothed {
    pub raw: Curve,
    pub smooth: Curve,
}

impl Smoothed {
    pub fn new(raw: &[[f64; 2]]) -> Smoothed {
        Smoothed { raw: raw.to_vec(), smooth: smoothed(raw) }
    }
}

/// The measurement as your ear has it: raw below ~5 kHz, smoothed rig level minus your
/// correction above ~6 kHz (a dip where the test asked for a boost).
pub fn ear_meas_at(meas: &Smoothed, prof: &[[f64; 2]], f: f64) -> f64 {
    let raw = interp(&meas.raw, f);
    let w = log_ramp(f, TT_X0, TT_X1);
    if w <= 0.0 {
        raw
    } else {
        (1.0 - w) * raw + w * (interp(&meas.smooth, f) - treble_corr_at(prof, f))
    }
}

/// The target as the treble fit aims for it: only its broad shape above ~6 kHz.
pub fn fit_tgt_at(tgt: &Smoothed, f: f64) -> f64 {
    let raw = interp(&tgt.raw, f);
    let w = log_ramp(f, TT_X0, TT_X1);
    if w <= 0.0 {
        raw
    } else {
        (1.0 - w) * raw + w * interp(&tgt.smooth, f)
    }
}

/// Robust line fit (Theil–Sen) in log-frequency, subtracted. Robust so one real peak at the end
/// of the range doesn't get read as a tilt and smeared across the other bands.
pub fn detrend(pts: &[[f64; 2]]) -> Curve {
    let xs: Vec<f64> = pts.iter().map(|p| p[0].log2()).collect();
    let ys: Vec<f64> = pts.iter().map(|p| p[1]).collect();
    let mut sl = Vec::new();
    for i in 0..xs.len() {
        for j in i + 1..xs.len() {
            sl.push((ys[j] - ys[i]) / (xs[j] - xs[i]));
        }
    }
    let med = |a: &[f64]| -> f64 {
        let mut s = a.to_vec();
        s.sort_by(|p, q| p.partial_cmp(q).unwrap_or(std::cmp::Ordering::Equal));
        let m = s.len() >> 1;
        if s.len() % 2 == 1 {
            s[m]
        } else {
            (s[m - 1] + s[m]) / 2.0
        }
    };
    let b = if sl.is_empty() { 0.0 } else { med(&sl) };
    let r: Vec<f64> = ys.iter().zip(&xs).map(|(y, x)| y - b * x).collect();
    let a = med(&r);
    pts.iter().enumerate().map(|(i, p)| [p[0], ys[i] - a - b * xs[i]]).collect()
}

/// Clamped-and-rounded correction values, as a finished test stores them.
pub fn tidy_profile(pts: &[[f64; 2]]) -> Curve {
    pts.iter().map(|p| [p[0], crate::eq::round1(clamp(p[1], -9.0, 6.0))]).collect()
}
