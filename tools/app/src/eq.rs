// The EQ model and its maths: bands, the RBJ biquads behind them, the device limits they are
// held to, and the squig / Equalizer APO text format. Pure functions, no UI and no device I/O.

use serde::{Deserialize, Serialize};

/// JavaScript's Math.round: halves go up (towards +inf), not away from zero. The app was written
/// against it, and saved EQs and auto-fit results depend on it landing the same way.
pub fn jround(x: f64) -> f64 {
    (x + 0.5).floor()
}
pub fn round1(x: f64) -> f64 {
    jround(x * 10.0) / 10.0
}
pub fn round2(x: f64) -> f64 {
    jround(x * 100.0) / 100.0
}
pub fn clamp(v: f64, lo: f64, hi: f64) -> f64 {
    hi.min(lo.max(v))
}
/// `x.toFixed(n)` as JavaScript prints it: rounded on the number's exact decimal value, with a
/// true half going away from zero (Rust's own formatting rounds halves to even).
pub fn fixed(x: f64, n: usize) -> String {
    // 40 extra digits is past where any f64's exact expansion stops mattering for a tie
    let long = format!("{:.*}", n + 40, x.abs());
    let (int, frac) = long.split_once('.').unwrap_or((&long, ""));
    let mut digits: Vec<u8> = int.bytes().chain(frac.bytes().take(n)).map(|b| b - b'0').collect();
    if frac.as_bytes().get(n).map(|&d| d >= b'5').unwrap_or(false) {
        let mut i = digits.len();
        loop {
            if i == 0 {
                digits.insert(0, 1);
                break;
            }
            i -= 1;
            if digits[i] == 9 {
                digits[i] = 0;
            } else {
                digits[i] += 1;
                break;
            }
        }
    }
    let k = digits.len() - n;
    let mut s: String = digits[..k].iter().map(|d| (d + b'0') as char).collect();
    if n > 0 {
        s.push('.');
        s.extend(digits[k..].iter().map(|d| (d + b'0') as char));
    }
    if x < 0.0 {
        s.insert(0, '-');
    }
    s
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, Default, Hash)]
pub enum FType {
    #[default]
    PK,
    LSQ,
    HSQ,
    LP,
    HP,
}

impl FType {
    pub fn from_code(s: &str) -> FType {
        match s {
            "LSQ" => FType::LSQ,
            "HSQ" => FType::HSQ,
            "LP" => FType::LP,
            "HP" => FType::HP,
            _ => FType::PK,
        }
    }
    pub fn code(self) -> &'static str {
        match self {
            FType::PK => "PK",
            FType::LSQ => "LSQ",
            FType::HSQ => "HSQ",
            FType::LP => "LP",
            FType::HP => "HP",
        }
    }
    /// What the band table shows.
    pub fn label(self) -> &'static str {
        match self {
            FType::PK => "PK",
            FType::LSQ => "LSC",
            FType::HSQ => "HSC",
            FType::LP => "LPF",
            FType::HP => "HPF",
        }
    }
    /// What squig / Equalizer APO call it.
    pub fn squig(self) -> &'static str {
        match self {
            FType::PK => "PK",
            FType::LSQ => "LSC",
            FType::HSQ => "HSC",
            FType::LP => "LPQ",
            FType::HP => "HPQ",
        }
    }
    fn from_squig(s: &str) -> FType {
        match s {
            "LSC" | "LS" | "LSQ" => FType::LSQ,
            "HSC" | "HS" | "HSQ" => FType::HSQ,
            "LP" | "LPQ" => FType::LP,
            "HP" | "HPQ" => FType::HP,
            _ => FType::PK,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct Band {
    pub freq: f64,
    pub gain: f64,
    pub q: f64,
    #[serde(rename = "type", default)]
    pub typ: FType,
}

impl Band {
    pub fn blank(freq: f64) -> Band {
        Band { freq, gain: 0.0, q: 1.0, typ: FType::PK }
    }
    pub fn pk(freq: f64, gain: f64, q: f64) -> Band {
        Band { freq, gain, q, typ: FType::PK }
    }
}

// ---- RBJ cookbook ----

pub struct Biquad {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a0: f64,
    pub a1: f64,
    pub a2: f64,
}

/// Coefficients as the app draws them. LP and HP are drawn as peaking, as they always were: the
/// graph's job is the boost/cut bands, and no device here does more than show those two types.
pub fn rbj(typ: FType, f: f64, g: f64, q: f64, fs: f64) -> Biquad {
    let a = 10f64.powf(g / 40.0);
    let w = 2.0 * std::f64::consts::PI * f / fs;
    let (cw, sw) = (w.cos(), w.sin());
    let al = sw / (2.0 * q);
    if typ == FType::LSQ || typ == FType::HSQ {
        let sa = (sw / 2.0) * ((a + 1.0 / a) * (1.0 / q - 1.0) + 2.0).sqrt();
        let t = 2.0 * a.sqrt() * sa;
        if typ == FType::LSQ {
            Biquad {
                b0: a * ((a + 1.0) - (a - 1.0) * cw + t),
                b1: 2.0 * a * ((a - 1.0) - (a + 1.0) * cw),
                b2: a * ((a + 1.0) - (a - 1.0) * cw - t),
                a0: (a + 1.0) + (a - 1.0) * cw + t,
                a1: -2.0 * ((a - 1.0) + (a + 1.0) * cw),
                a2: (a + 1.0) + (a - 1.0) * cw - t,
            }
        } else {
            Biquad {
                b0: a * ((a + 1.0) + (a - 1.0) * cw + t),
                b1: -2.0 * a * ((a - 1.0) + (a + 1.0) * cw),
                b2: a * ((a + 1.0) + (a - 1.0) * cw - t),
                a0: (a + 1.0) - (a - 1.0) * cw + t,
                a1: 2.0 * ((a - 1.0) - (a + 1.0) * cw),
                a2: (a + 1.0) - (a - 1.0) * cw - t,
            }
        }
    } else {
        Biquad { b0: 1.0 + al * a, b1: -2.0 * cw, b2: 1.0 - al * a, a0: 1.0 + al / a, a1: -2.0 * cw, a2: 1.0 - al / a }
    }
}

pub fn mag_db(b: &Band, at: f64) -> f64 {
    let c = rbj(b.typ, b.freq, b.gain, b.q, 48000.0);
    let w = 2.0 * std::f64::consts::PI * at / 48000.0;
    let (c1, s1, c2, s2) = (w.cos(), w.sin(), (2.0 * w).cos(), (2.0 * w).sin());
    let nr = c.b0 + c.b1 * c1 + c.b2 * c2;
    let ni = -(c.b1 * s1 + c.b2 * s2);
    let dr = c.a0 + c.a1 * c1 + c.a2 * c2;
    let di = -(c.a1 * s1 + c.a2 * s2);
    10.0 * ((nr * nr + ni * ni) / (dr * dr + di * di)).log10()
}

pub fn combined_at(bands: &[Band], f: f64) -> f64 {
    let mut s = 0.0;
    for b in bands {
        if b.gain != 0.0 {
            s += mag_db(b, f);
        }
    }
    s
}

/// Highest point of the combined response, and where.
pub fn peak(bands: &[Band]) -> (f64, f64) {
    let (mut p, mut pf) = (f64::NEG_INFINITY, 0.0);
    for i in 0..=240 {
        let f = 20.0 * 1000f64.powf(i as f64 / 240.0);
        let v = combined_at(bands, f);
        if v > p {
            p = v;
            pf = f;
        }
    }
    (p, jround(pf))
}

pub fn log_freq(t: f64) -> f64 {
    20.0 * 1000f64.powf(t)
}

// ---- the device's limits ----

/// What the connected device (or the offline target) allows. Holds the gen-1 values until a
/// device is opened, so the editor is sane before anything is connected.
#[derive(Clone, Debug, PartialEq)]
pub struct Caps {
    pub gen: u8,
    pub bands: usize,
    pub g_min: f64,
    pub g_max: f64,
    pub q_min: f64,
    pub q_max: f64,
    pub types: Vec<FType>,
    pub preamp_step: f64,
}

impl Default for Caps {
    fn default() -> Self {
        Caps {
            gen: 1,
            bands: 9,
            g_min: -12.0,
            g_max: 3.0,
            q_min: 0.2,
            q_max: 10.0,
            types: vec![FType::PK, FType::LSQ, FType::HSQ],
            preamp_step: 0.1,
        }
    }
}

pub const F_MIN: f64 = 20.0;
pub const F_MAX: f64 = 20000.0;

impl Caps {
    /// Placeholder frequencies for blank bands: octaves for nine, otherwise spread
    /// logarithmically over the same span and rounded to a tidy step.
    pub fn default_freqs(&self) -> Vec<f64> {
        let nb = self.bands;
        if nb == 9 {
            return vec![31.0, 62.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0];
        }
        let (lo, hi) = (31.0f64, 8000.0f64);
        (0..nb)
            .map(|i| {
                let t = if nb < 2 { 0.0 } else { i as f64 / (nb - 1) as f64 };
                let f = lo * (hi / lo).powf(t);
                let step = if f < 100.0 { 1.0 } else if f < 1000.0 { 5.0 } else { 50.0 };
                jround(f / step) * step
            })
            .collect()
    }
    pub fn default_freq(&self, i: usize) -> f64 {
        let d = self.default_freqs();
        *d.get(i).unwrap_or(d.last().unwrap_or(&1000.0))
    }
    pub fn blank_model(&self) -> Vec<Band> {
        self.default_freqs().into_iter().map(Band::blank).collect()
    }
    /// Takes up to `bands` of `list` and pads with blank bands.
    pub fn fill(&self, list: &[Band]) -> Vec<Band> {
        let mut out: Vec<Band> = list.iter().take(self.bands).copied().collect();
        for i in out.len()..self.bands {
            out.push(Band::blank(self.default_freq(i)));
        }
        out
    }
    /// Preamp is headroom, so it rounds down to the device's step: on a whole-dB device -4.2
    /// becomes -5, never -4.
    pub fn pre_round(&self, v: f64) -> f64 {
        let v = clamp(v, -20.0, 0.0);
        if self.preamp_step >= 1.0 {
            (v + 1e-9).floor()
        } else {
            round1(v)
        }
    }
    /// The preamp that brings a peak of `p` dB down to 0 or below.
    pub fn headroom_for(&self, p: f64) -> f64 {
        if p > 0.0 {
            if self.preamp_step >= 1.0 {
                -(p - 1e-9).ceil()
            } else {
                -((p * 10.0).ceil() / 10.0)
            }
        } else {
            0.0
        }
    }
    pub fn gain_step(&self) -> f64 {
        if self.gen >= 2 {
            0.1
        } else {
            1.0
        }
    }
    pub fn round_gain(&self, v: f64) -> f64 {
        if self.gen >= 2 {
            round1(v)
        } else {
            jround(v)
        }
    }
}

// ---- typed fields in the band table ----

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Field {
    Freq,
    Gain,
    Q,
}

pub fn fmt_field(k: Field, v: f64) -> String {
    match k {
        Field::Freq => format!("{}", jround(v) as i64),
        Field::Gain => format!("{}{}", if v > 0.0 { "+" } else { "" }, fixed(v, 1)),
        Field::Q => fixed(v, 2),
    }
}

/// Frequency takes "8000", "8k" or "8.5 kHz"; gain and Q take a number, with "dB" or a leading
/// "q" allowed. A comma works as the decimal point.
pub fn parse_field(k: Field, s: &str) -> Option<f64> {
    let s: String = s.trim().to_lowercase().replacen(',', ".", 1).chars().filter(|c| !c.is_whitespace()).collect();
    if k == Field::Freq {
        let (num, mul) = if let Some(n) = s.strip_suffix("khz") {
            (n, 1000.0)
        } else if let Some(n) = s.strip_suffix('k') {
            (n, 1000.0)
        } else if let Some(n) = s.strip_suffix("hz") {
            (n, 1.0)
        } else {
            (s.as_str(), 1.0)
        };
        if num.is_empty() || num.starts_with(['+', '-']) || !is_plain_number(num) {
            return None;
        }
        return num.parse::<f64>().ok().map(|v| v * mul);
    }
    let t = s.strip_suffix("db").unwrap_or(&s);
    let t = t.strip_prefix('q').unwrap_or(t);
    let body = t.strip_prefix(['+', '-']).unwrap_or(t);
    if body.is_empty() || !is_plain_number(body) {
        return None;
    }
    t.parse::<f64>().ok()
}

fn is_plain_number(s: &str) -> bool {
    // \d*\.?\d+
    let mut seen_dot = false;
    let mut digits_after = 0;
    for c in s.chars() {
        if c == '.' {
            if seen_dot {
                return false;
            }
            seen_dot = true;
            digits_after = 0;
        } else if c.is_ascii_digit() {
            digits_after += 1;
        } else {
            return false;
        }
    }
    digits_after > 0
}

/// Sets a field within the limits, rounded as the device stores it.
pub fn set_field(b: &mut Band, k: Field, v: f64, caps: &Caps, g_max: f64) {
    match k {
        Field::Freq => b.freq = jround(clamp(v, F_MIN, F_MAX)),
        Field::Gain => b.gain = clamp(round1(v), caps.g_min, g_max),
        Field::Q => b.q = clamp(round2(v), caps.q_min, caps.q_max),
    }
}

/// Up/Down in a field: a small step, or a big one with Shift.
pub fn nudge_field(b: &mut Band, k: Field, dir: f64, big: bool, caps: &Caps, g_max: f64) {
    let v = match k {
        Field::Freq => b.freq * 2f64.powf(dir * if big { 1.0 / 6.0 } else { 1.0 / 48.0 }) + dir * 0.5,
        Field::Gain => b.gain + dir * if big { 1.0 } else { 0.1 },
        Field::Q => b.q + dir * if big { 0.1 } else { 0.01 },
    };
    set_field(b, k, v, caps, g_max);
}

// ---- squig / AutoEQ / Equalizer APO text ----

pub struct Parsed {
    pub preamp: f64,
    pub filters: Vec<Band>,
    pub skipped: usize,
}

/// Reads "Preamp: -3.8 dB" and "Filter 1: ON PK Fc 105 Hz Gain 3.0 dB Q 0.70" lines, including
/// Equalizer APO's numberless "Filter: ON …". Disabled filters are counted, not kept.
pub fn parse_squig(text: &str) -> Parsed {
    let mut out = Parsed { preamp: 0.0, filters: Vec::new(), skipped: 0 };
    for ln in text.lines() {
        let lower = ln.to_ascii_lowercase();
        if let Some(i) = lower.find("preamp:") {
            let rest = ln[i + 7..].trim_start();
            if let Some(v) = leading_number(rest, true) {
                if rest[v.1..].trim_start().to_ascii_lowercase().starts_with("db") {
                    out.preamp = v.0;
                    continue;
                }
            }
        }
        if let Some(f) = parse_filter_line(ln) {
            match f {
                Some(b) => out.filters.push(b),
                None => out.skipped += 1,
            }
        }
    }
    out
}

/// A number at the start of `s` ([-]digits and dots), and how many bytes it took.
fn leading_number(s: &str, signed: bool) -> Option<(f64, usize)> {
    let mut end = 0;
    for (i, c) in s.char_indices() {
        if c.is_ascii_digit() || c == '.' || (signed && i == 0 && c == '-') {
            end = i + c.len_utf8();
        } else {
            break;
        }
    }
    if end == 0 {
        return None;
    }
    s[..end].parse::<f64>().ok().map(|v| (v, end))
}

/// Some(Some(band)) for an ON filter, Some(None) for an OFF one, None for anything else.
fn parse_filter_line(ln: &str) -> Option<Option<Band>> {
    let lower = ln.to_ascii_lowercase();
    let at = lower.find("filter")?;
    let mut rest = &ln[at + 6..];
    rest = rest.trim_start();
    // optional number, then ':'
    let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
    rest = rest[digits..].trim_start();
    rest = rest.strip_prefix(':')?;
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let toks: Vec<&str> = rest.split_whitespace().collect();
    // ON|OFF TYPE Fc <f> Hz Gain <g> dB Q <q>
    let on = toks.first()?.to_ascii_uppercase();
    if on != "ON" && on != "OFF" {
        return None;
    }
    let typ = toks.get(1)?;
    if !typ.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    let mut i = 2;
    // "<name> <number>[unit]"; returns the number and any unit glued to it
    fn want(toks: &[&str], i: &mut usize, name: &str) -> Option<(f64, Option<String>)> {
        if !toks.get(*i)?.eq_ignore_ascii_case(name) {
            return None;
        }
        let t = toks.get(*i + 1)?;
        *i += 2;
        let (v, n) = leading_number(t, name == "Gain")?;
        let tail = &t[n..];
        Some((v, if tail.is_empty() { None } else { Some(tail.to_string()) }))
    }
    // the unit, either glued to the number or as the next token
    fn unit(toks: &[&str], i: &mut usize, glued: Option<String>, u: &str) -> Option<()> {
        match glued {
            Some(g) if g.eq_ignore_ascii_case(u) => Some(()),
            Some(_) => None,
            None => {
                if !toks.get(*i)?.eq_ignore_ascii_case(u) {
                    return None;
                }
                *i += 1;
                Some(())
            }
        }
    }
    let (fc, fu) = want(&toks, &mut i, "Fc")?;
    unit(&toks, &mut i, fu, "Hz")?;
    let (g, gu) = want(&toks, &mut i, "Gain")?;
    unit(&toks, &mut i, gu, "dB")?;
    let (q, _) = want(&toks, &mut i, "Q")?;
    if on == "OFF" {
        return Some(None);
    }
    Some(Some(Band { freq: fc, gain: g, q, typ: FType::from_squig(&typ.to_ascii_uppercase()) }))
}

/// The EQ as squig / Equalizer APO parametric text: only the bands that do something.
pub fn eq_to_text(model: &[Band], preamp: f64) -> String {
    let mut lines = vec![format!("Preamp: {} dB", fixed(preamp, 1))];
    for (i, b) in model.iter().filter(|b| b.gain != 0.0).enumerate() {
        lines.push(format!(
            "Filter {}: ON {} Fc {} Hz Gain {} dB Q {}",
            i + 1,
            b.typ.squig(),
            js_num(b.freq),
            fixed(b.gain, 1),
            fixed(b.q, 3)
        ));
    }
    lines.join("\n")
}

/// A number as JavaScript prints it (no trailing ".0").
pub fn js_num(v: f64) -> String {
    if v == v.trunc() && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{}", v)
    }
}

/// Fits parsed filters to what the device can do and says what had to change. Returns the new
/// model, preamp and the message.
pub fn apply_filters(p: &Parsed, caps: &Caps, g_max: f64) -> (Vec<Band>, f64, String) {
    let nb = caps.bands;
    let mut warns: Vec<String> = Vec::new();
    if p.filters.len() > nb {
        warns.push(format!("had {} filters, kept first {}", p.filters.len(), nb));
    }
    let clamped: Vec<Band> = p
        .filters
        .iter()
        .take(nb)
        .map(|f| {
            let mut b = *f;
            if b.freq < F_MIN || b.freq > F_MAX {
                b.freq = clamp(b.freq, F_MIN, F_MAX);
                warns.push("freq clamped".into());
            }
            if b.q < caps.q_min || b.q > caps.q_max {
                b.q = clamp(b.q, caps.q_min, caps.q_max);
                warns.push("Q clamped".into());
            }
            if b.gain > g_max || b.gain < caps.g_min {
                b.gain = clamp(b.gain, caps.g_min, g_max);
                warns.push(format!("gain clamped to {}/+{}", js_num(caps.g_min), js_num(g_max)));
            }
            if !caps.types.contains(&b.typ) {
                warns.push(format!("{} isn't available here, made PK", b.typ.label()));
                b.typ = FType::PK;
            }
            b.gain = round1(b.gain);
            b
        })
        .collect();
    let model = caps.fill(&clamped);
    let preamp = caps.pre_round(p.preamp);
    let flat = nb - clamped.len();
    let mut msg = format!(
        "Applied {} filter(s){}. Preamp {} dB.",
        clamped.len(),
        if flat > 0 { format!(", {} slot(s) flat", flat) } else { String::new() },
        fixed(preamp, 1)
    );
    if p.skipped > 0 {
        msg += &format!(" {} disabled skipped.", p.skipped);
    }
    if !warns.is_empty() {
        let mut seen = Vec::new();
        for w in warns {
            if !seen.contains(&w) {
                seen.push(w);
            }
        }
        msg += &format!("  ⚠ {}", seen.join(", "));
    }
    (model, preamp, msg)
}

// ---- bank devices ----

/// A device an EQ can be designed for: the connected bank device, or one picked offline.
#[derive(Clone, Debug)]
pub struct Target {
    pub id: String,
    pub name: String,
    pub gen: u8,
    pub bands: usize,
    pub g_min: f64,
    pub g_max: f64,
    pub q_min: f64,
    pub q_max: f64,
    pub types: Vec<FType>,
    pub presets: std::collections::BTreeMap<u8, String>,
    pub user_slots: Vec<u8>,
    pub tested: bool,
}

/// "FIIO BTR17" -> "fiio-btr17", the id the BTR17's bank sets were first saved under.
pub fn target_id(name: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in name.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            dash = false;
        } else if !dash {
            out.push('-');
            dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

pub struct Fitted {
    pub bands: Vec<Band>,
    pub preamp: f64,
    pub issues: Vec<String>,
}

/// Makes an EQ fit a target: only the active bands are kept (at most the target's count, the
/// strongest first), gains and Q go into range, and filter types it can't run become peaking.
pub fn fit_to_target(bands: &[Band], pre: f64, t: &Target) -> Fitted {
    let mut issues = Vec::new();
    let mut act: Vec<Band> = bands.iter().filter(|b| b.gain != 0.0).copied().collect();
    if act.len() > t.bands {
        let mut idx: Vec<usize> = (0..act.len()).collect();
        idx.sort_by(|&a, &b| act[b].gain.abs().partial_cmp(&act[a].gain.abs()).unwrap_or(std::cmp::Ordering::Equal));
        let keep: Vec<usize> = idx.into_iter().take(t.bands).collect();
        issues.push(format!("dropped the {} weakest band(s)", act.len() - t.bands));
        act = act.iter().enumerate().filter(|(i, _)| keep.contains(i)).map(|(_, b)| *b).collect();
    }
    let act = act
        .into_iter()
        .map(|mut c| {
            if !t.types.contains(&c.typ) {
                issues.push(format!("{} at {} Hz became PK", c.typ.label(), js_num(c.freq)));
                c.typ = FType::PK;
            }
            let g = clamp(c.gain, t.g_min, t.g_max);
            if g != c.gain {
                issues.push(format!("gain at {} Hz clamped to {}", js_num(c.freq), js_num(g)));
                c.gain = g;
            }
            let q = clamp(c.q, t.q_min, t.q_max);
            if q != c.q {
                issues.push(format!("Q at {} Hz clamped to {}", js_num(c.freq), js_num(q)));
                c.q = q;
            }
            c.freq = clamp(jround(c.freq), 20.0, 20000.0);
            c.gain = round1(c.gain);
            c.q = round2(c.q);
            c
        })
        .collect();
    Fitted { bands: act, preamp: round1(clamp(pre, -20.0, 0.0)), issues }
}

pub fn peak_of(bands: &[Band]) -> f64 {
    let mut p = f64::NEG_INFINITY;
    for i in 0..=240 {
        let v = combined_at(bands, log_freq(i as f64 / 240.0));
        if v > p {
            p = v;
        }
    }
    p
}

pub fn active_count(bands: &[Band]) -> usize {
    bands.iter().filter(|b| b.gain != 0.0).count()
}

/// Whether two EQs are the same band for band.
pub fn same_eq(a: &[Band], b: &[Band]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.freq == y.freq && x.gain == y.gain && x.q == y.q && x.typ == y.typ)
}
