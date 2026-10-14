// Auto-fit: the bands fitted to (target - measurement).
//
// Ports AutoEq's approach (Jaakko Pasanen, MIT): matching-pursuit init + MSE loss with a
// sharpness penalty and "only average energy above 10 kHz", adapted to a fixed band count and
// the device's gain limits (SLSQP -> per-parameter golden-section coordinate descent).
// Golden-section assumes one minimum, so each band's frequency is only searched within an octave
// of where it sits (a global search teleports bands and leaves them cancelling each other).
// Like squig.link's Auto EQ, bands the fit doesn't need are dropped (left flat), and outside the
// configured range the EQ is pushed towards flat rather than fitted to an untrusted measurement.

use crate::eq::{clamp, jround, mag_db, round1, round2, Band, Caps, FType, F_MAX, F_MIN};
use crate::fr::{ear_meas_at, fit_tgt_at, interp, Smoothed, TT_BANDS};

const FIT_N: usize = 180;
const SPILL_W: f64 = 0.1; // weight of the "stay flat outside the fit range" term
const STACK_OCT: f64 = 1.0 / 12.0; // two bands closer than this are one filter's job
const PRUNE_DB: f64 = 0.02; // a band must improve the fit by at least this much (dB RMS) to keep its slot

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum FitMode {
    /// Fit target vs raw measurement between the bounds, leave the rest flat.
    #[default]
    Range,
    /// Fit the whole target with every band, through the treble correction above ~6 kHz.
    Treble,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FitCfg {
    pub f_low: f64,
    pub f_high: f64,
    pub bands: usize,
    pub mode: FitMode,
    pub top: f64,
}

impl Default for FitCfg {
    fn default() -> Self {
        FitCfg { f_low: 20.0, f_high: 20000.0, bands: 9, mode: FitMode::Range, top: 16000.0 }
    }
}

/// Where the loss looks: the fit range, and above which only average energy counts.
#[derive(Clone, Copy)]
struct Span {
    lo: f64,
    hi: f64,
    avg_above: f64,
}

fn resp_except(bands: &[Band], k: Option<usize>, fs: &[f64]) -> Vec<f64> {
    fs.iter()
        .map(|&f| {
            let mut s = 0.0;
            for (j, b) in bands.iter().enumerate() {
                if Some(j) != k && b.gain != 0.0 {
                    s += mag_db(b, f);
                }
            }
            s
        })
        .collect()
}

fn sharp_penalty(b: &Band) -> f64 {
    if b.q > 4.0 {
        0.0006 * (b.q - 4.0) * (b.q - 4.0) * (b.gain * b.gain + 1.0)
    } else {
        0.0
    }
}

fn fit_loss(band_k: &Band, others: &[f64], fs: &[f64], err: &[f64], sp_: Span) -> f64 {
    let (mut se, mut cnt, mut m_e, mut m_c, mut hc, mut sp, mut sc) = (0.0, 0usize, 0.0, 0.0, 0usize, 0.0, 0usize);
    for i in 0..fs.len() {
        let f = fs[i];
        let comb = others[i] + if band_k.gain != 0.0 { mag_db(band_k, f) } else { 0.0 };
        if f < sp_.lo || f > sp_.hi {
            sp += comb * comb; // outside the range: the EQ should leave it alone
            sc += 1;
        } else if f > sp_.avg_above {
            m_e += err[i]; // up top: average energy only
            m_c += comb;
            hc += 1;
        } else {
            let d = err[i] - comb;
            se += d * d;
            cnt += 1;
        }
    }
    let mut loss = if cnt > 0 { se / cnt as f64 } else { 0.0 };
    if hc > 0 {
        let d = (m_e - m_c) / hc as f64;
        loss += d * d;
    }
    if sc > 0 {
        loss += SPILL_W * sp / sc as f64;
    }
    loss + sharp_penalty(band_k)
}

/// Total loss of a whole band set (used when pruning).
fn fit_loss_all(bands: &[Band], fs: &[f64], err: &[f64], sp: Span) -> f64 {
    let pen: f64 = bands.iter().map(sharp_penalty).sum();
    fit_loss(&Band::pk(1000.0, 0.0, 1.0), &resp_except(bands, None, fs), fs, err, sp) + pen
}

#[derive(Clone, Copy, PartialEq)]
enum Param {
    Freq,
    Gain,
    Q,
}

fn get(b: &Band, p: Param) -> f64 {
    match p {
        Param::Freq => b.freq,
        Param::Gain => b.gain,
        Param::Q => b.q,
    }
}

fn put(b: &mut Band, p: Param, v: f64, lo: f64, hi: f64) {
    match p {
        Param::Freq => b.freq = jround(clamp(v, lo, hi)),
        Param::Gain => b.gain = clamp(round1(v), lo, hi),
        Param::Q => b.q = clamp(round2(v), lo, hi),
    }
}

#[allow(clippy::too_many_arguments)]
fn opt_param(bands: &mut [Band], k: usize, param: Param, lo: f64, hi: f64, fs: &[f64], err: &[f64], log: bool, sp: Span) {
    let others = resp_except(bands, Some(k), fs);
    let b = &mut bands[k];
    let (mut a, mut c) = if log { (lo.ln(), hi.ln()) } else { (lo, hi) };
    let to_v = |x: f64| if log { x.exp() } else { x };
    let ev = |b: &mut Band, x: f64| {
        put(b, param, to_v(x), lo, hi);
        fit_loss(b, &others, fs, err, sp)
    };
    // The loss is bumpy along frequency, so golden-section can settle in a worse dip than the one
    // the band started in. Only move if it actually helps; otherwise a well-placed band gets
    // thrown elsewhere and neighbours pile in to compensate (cancelling pairs).
    let keep = get(b, param);
    let before = fit_loss(b, &others, fs, err, sp);
    let gr = (5f64.sqrt() - 1.0) / 2.0;
    let mut x1 = c - gr * (c - a);
    let mut x2 = a + gr * (c - a);
    let mut f1 = ev(b, x1);
    let mut f2 = ev(b, x2);
    for _ in 0..16 {
        if f1 < f2 {
            c = x2;
            x2 = x1;
            f2 = f1;
            x1 = c - gr * (c - a);
            f1 = ev(b, x1);
        } else {
            a = x1;
            x1 = x2;
            f1 = f2;
            x2 = a + gr * (c - a);
            f2 = ev(b, x2);
        }
    }
    put(b, param, to_v((a + c) / 2.0), lo, hi);
    if fit_loss(b, &others, fs, err, sp) > before {
        match param {
            Param::Freq => b.freq = keep,
            Param::Gain => b.gain = keep,
            Param::Q => b.q = keep,
        }
    }
}

pub struct FitInput<'a> {
    pub meas: &'a Smoothed,
    pub tgt: &'a Smoothed,
    /// The selected treble profile, when the mode is Treble.
    pub prof: Option<&'a [[f64; 2]]>,
    pub cfg: FitCfg,
    pub caps: &'a Caps,
    /// The gain ceiling in force (raised by "+6 dB mode" on gen-1).
    pub g_max: f64,
}

pub enum FitOutcome {
    /// Whole-target mode without a treble profile for this measurement.
    NoProfile,
    /// The fitted bands, sorted by frequency (only the ones in use).
    Bands(Vec<Band>),
}

pub fn auto_fit(inp: &FitInput) -> FitOutcome {
    let tm = inp.cfg.mode == FitMode::Treble;
    let prof = if tm { inp.prof } else { None };
    if tm && prof.is_none() {
        return FitOutcome::NoProfile;
    }
    let caps = inp.caps;
    let nb = caps.bands;
    let fs: Vec<f64> = (0..=FIT_N).map(|i| 20.0 * 1000f64.powf(i as f64 / FIT_N as f64)).collect();
    let err: Vec<f64> = fs
        .iter()
        .map(|&f| match prof {
            Some(p) => fit_tgt_at(inp.tgt, f) - ear_meas_at(inp.meas, p, f),
            None => interp(&inp.tgt.raw, f) - interp(&inp.meas.raw, f),
        })
        .collect();
    let g_max = inp.g_max;
    let n_b = if inp.cfg.bands == 0 { nb } else { inp.cfg.bands }.clamp(1, nb);
    let top = F_MAX.min(if inp.cfg.top > 0.0 { inp.cfg.top } else { 16000.0 }); // whole-target mode: no bands, and no fitting, above this
    let (mut f_lo, mut f_hi) = if tm { (F_MIN, top) } else { (F_MIN.max(inp.cfg.f_low), F_MAX.min(inp.cfg.f_high)) };
    if f_hi <= f_lo {
        f_lo = F_MIN;
        f_hi = F_MAX;
    }
    let avg_above = match prof {
        Some(p) => top.min(p[p.len() - 1][0] * 1.06),
        None => 10000.0,
    };
    let sp = Span { lo: f_lo, hi: f_hi, avg_above };
    let (g_min, q_min, q_max) = (caps.g_min, caps.q_min, caps.q_max);

    // Matching-pursuit init: place each band on the biggest remaining error (within range),
    // subtract, repeat. The residual is reduced by the correction the spot *needs*, not by the
    // gain-capped band, so a capped band can't leave a residual that drags every following band
    // onto the same frequency. Above the average-energy threshold only the average matters (see
    // fit_loss), so bands start at or below it; refinement can still move them up.
    let mut resid = err.clone();
    let init_hi = if f_lo < avg_above && f_hi > avg_above { avg_above } else { f_hi };
    // Whole-target mode spans the full range, where a narrow treble feature can out-peak a broad
    // bass error that matters more, and the octave-limited refinement can't carry a band back
    // down. There, each band starts where it removes the most squared error (true matching
    // pursuit) rather than where the residual peaks. Range mode keeps the peak pick, which is
    // what was checked against squig.
    let in_range: Vec<bool> = fs.iter().map(|&f| f >= f_lo && f <= f_hi).collect();
    let atoms: Option<Vec<Option<Vec<f64>>>> = if tm {
        Some(
            fs.iter()
                .map(|&fc| {
                    if fc < f_lo || fc > init_hi {
                        None
                    } else {
                        let a = Band::pk(fc, 6.0, 1.4);
                        Some(fs.iter().map(|&f| mag_db(&a, f) / 6.0).collect())
                    }
                })
                .collect(),
        )
    } else {
        None
    };
    // A start is never within STACK_OCT of an existing band: a narrow feature the wide starting
    // band only half covers is that band's to narrow into, not a reason to put a second band on
    // top of it.
    let near = |bands: &[Band], i: usize| bands.iter().any(|b| (fs[i] / b.freq).log2().abs() < STACK_OCT);
    let pick = |bands: &[Band], r: &[f64]| -> (Option<usize>, f64) {
        let (mut bi, mut bv) = (None, 0.0);
        if let Some(atoms) = &atoms {
            let mut best = 0.0;
            for i in 0..fs.len() {
                let Some(a) = &atoms[i] else { continue };
                if near(bands, i) {
                    continue;
                }
                let (mut ra, mut aa) = (0.0, 0.0);
                for j in 0..fs.len() {
                    if !in_range[j] {
                        continue;
                    }
                    ra += r[j] * a[j];
                    aa += a[j] * a[j];
                }
                if aa != 0.0 && ra * ra / aa > best {
                    best = ra * ra / aa;
                    bi = Some(i);
                    bv = ra / aa;
                }
            }
        } else {
            for i in 0..fs.len() {
                if fs[i] < f_lo || fs[i] > init_hi || near(bands, i) {
                    continue;
                }
                if r[i].abs() > bv.abs() {
                    bv = r[i];
                    bi = Some(i);
                }
            }
        }
        (bi, bv)
    };
    let start_band =
        |bi: usize, bv: f64| Band { freq: jround(clamp(fs[bi], f_lo, f_hi)), gain: clamp(round1(bv), g_min, g_max), q: 1.4, typ: FType::PK };
    let mut bands: Vec<Band> = Vec::new();
    for _ in 0..n_b {
        let (bi, bv) = pick(&bands, &resid);
        let Some(bi) = bi else { break };
        if bv.abs() < 0.1 {
            break; // nothing left worth a band
        }
        let band = start_band(bi, bv);
        bands.push(band);
        let want = Band { gain: round1(bv), ..band };
        for i in 0..fs.len() {
            resid[i] -= mag_db(&want, fs[i]);
        }
    }
    // coordinate-descent refinement; a band's frequency only moves within an octave of where it sits
    let refine = |bands: &mut Vec<Band>, rounds: usize| {
        for _ in 0..rounds {
            for k in 0..bands.len() {
                opt_param(bands, k, Param::Gain, g_min, g_max, &fs, &err, false, sp);
                opt_param(bands, k, Param::Q, q_min, q_max, &fs, &err, false, sp);
                let (lo, hi) = (f_lo.max(bands[k].freq / 2.0), f_hi.min(bands[k].freq * 2.0));
                opt_param(bands, k, Param::Freq, lo, hi, &fs, &err, true, sp);
            }
        }
    };
    // Two bands within STACK_OCT of each other are doing one filter's job. Fold each such pair into
    // one band (|gain|-weighted centre and width, gains summed, which is what two coincident peaks
    // add up to), then start the freed band wherever the most error is left and refine again.
    let merge_stacked = |bands: &mut Vec<Band>| -> usize {
        let mut n = 0;
        let mut a = 0;
        while a < bands.len() {
            let mut b = bands.len() - 1;
            while b > a {
                let (ba, bb) = (bands[a], bands[b]);
                if (ba.freq / bb.freq).log2().abs() < STACK_OCT {
                    let wa = if ba.gain.abs() != 0.0 { ba.gain.abs() } else { 1e-6 };
                    let wb = if bb.gain.abs() != 0.0 { bb.gain.abs() } else { 1e-6 };
                    let m = &mut bands[a];
                    m.freq = jround(((wa * ba.freq.ln() + wb * bb.freq.ln()) / (wa + wb)).exp());
                    m.q = clamp(round2((wa * ba.q + wb * bb.q) / (wa + wb)), q_min, q_max);
                    m.gain = clamp(round1(ba.gain + bb.gain), g_min, g_max);
                    bands.remove(b);
                    n += 1;
                }
                b -= 1;
            }
            a += 1;
        }
        n
    };
    let leftover = |bands: &[Band]| -> Vec<f64> {
        err.iter()
            .enumerate()
            .map(|(i, e)| {
                let mut c = 0.0;
                for b in bands {
                    if b.gain != 0.0 {
                        c += mag_db(b, fs[i]);
                    }
                }
                e - c
            })
            .collect()
    };
    // One finishing run from the starting bands: refine, optionally fold stacked pairs mid-fit and
    // re-place the freed bands, prune, then fold any pair that is still stacked.
    let init_bands = bands.clone();
    let finish = |mid_merge: bool| -> (Vec<Band>, f64) {
        let mut bands = init_bands.clone();
        refine(&mut bands, 6);
        let mut pass = 0;
        while mid_merge && pass < 3 && merge_stacked(&mut bands) > 0 {
            while bands.len() < n_b {
                let (bi, bv) = pick(&bands, &leftover(&bands));
                let Some(bi) = bi else { break };
                if bv.abs() < 0.1 {
                    break;
                }
                bands.push(start_band(bi, bv));
            }
            refine(&mut bands, 3);
            pass += 1;
        }
        // Drop bands that do nothing: near-flat ones, then, one at a time, the band whose removal
        // costs the least, as long as that cost is under PRUNE_DB. Kept low on purpose: the error
        // is averaged over the whole fit range, so a band fixing a real 1 dB wiggle only moves it
        // a few hundredths of a dB, and a higher bar threw half the bands away. Spare bands for
        // hand use are what the "Bands" count is for.
        bands.retain(|b| b.gain.abs() > 0.1);
        while bands.len() > 1 {
            let base = fit_loss_all(&bands, &fs, &err, sp).sqrt();
            let (mut bk, mut bc) = (0usize, f64::INFINITY);
            for k in 0..bands.len() {
                let without: Vec<Band> = bands.iter().enumerate().filter(|(j, _)| *j != k).map(|(_, b)| *b).collect();
                let c = fit_loss_all(&without, &fs, &err, sp).sqrt() - base;
                if c < bc {
                    bc = c;
                    bk = k;
                }
            }
            if bc >= PRUNE_DB {
                break;
            }
            bands.remove(bk);
            refine(&mut bands, 1);
        }
        if merge_stacked(&mut bands) > 0 {
            refine(&mut bands, 1); // refinement can pull two together again; last word on it
        }
        merge_stacked(&mut bands);
        let loss = fit_loss_all(&bands, &fs, &err, sp);
        (bands, loss)
    };
    // Both modes also try folding stacked pairs mid-fit, re-placing the freed bands where the most
    // error is left, and keep whichever run fits better.
    let mut best = finish(false);
    let alt = finish(true);
    if alt.1 < best.1 {
        best = alt;
    }
    let mut out = best.0;
    out.sort_by(|a, b| a.freq.partial_cmp(&b.freq).unwrap_or(std::cmp::Ordering::Equal));
    FitOutcome::Bands(out)
}

// ---- apply a treble result straight to the EQ (what the SoundEQ site does) ----
// Turns a result into treble bands in the EQ's flat slots, leaving the other bands alone. The
// profile describes the bare IEM, so what the current EQ already does up there is taken off
// first: applying twice doesn't double up.
// The test only knows 7 points, so the fit is to those points, not to a curve drawn between
// them: one filter at each of the biggest corrections (up to 4), free to shift a twelfth of an
// octave, gains and widths solved together so overlapping filters don't add up. Two fixed
// points at 4 and 5 kHz keep the filters from leaking into what the measurement fit covers.
const TT_APPLY_MAX: usize = 4;
const TT_APPLY_QMAX: f64 = 6.0; // Q 6 ≈ quarter octave; narrower would chase insertion depth

pub enum TrebleApply {
    /// Nothing changed; the message says why.
    Unchanged { ok: bool, msg: String },
    /// The new model and preamp.
    Changed { model: Vec<Band>, preamp: f64, msg: String },
}

pub fn apply_treble(prof: &[[f64; 2]], model: &[Band], preamp: f64, caps: &Caps, g_max: f64, top_cfg: f64) -> TrebleApply {
    let free: Vec<usize> = model.iter().enumerate().filter(|(_, b)| b.gain == 0.0).map(|(i, _)| i).collect();
    if free.is_empty() {
        return TrebleApply::Unchanged {
            ok: false,
            msg: "Every band is in use. Flatten some (or auto-fit with fewer bands) to leave room for the treble.".into(),
        };
    }
    let need: Vec<f64> = crate::fr::detrend(&prof.iter().map(|p| [p[0], p[1] - crate::eq::combined_at(model, p[0])]).collect::<Vec<_>>())
        .iter()
        .map(|p| p[1])
        .collect(); // still to add, local shape only
    let top = F_MAX.min(if top_cfg > 0.0 { top_cfg } else { 16000.0 });
    let step = 2f64.powf(1.0 / 12.0);
    let mut fs = vec![4000.0, 5000.0];
    fs.extend(TT_BANDS);
    let mut err = vec![0.0, 0.0];
    err.extend(&need);
    let sp = Span { lo: 0.0, hi: f64::INFINITY, avg_above: f64::INFINITY }; // plain squared error at the points
    let mut cand: Vec<(f64, f64)> = need.iter().enumerate().map(|(i, &v)| (v, TT_BANDS[i])).filter(|&(v, f)| v.abs() >= 1.5 && f <= top * step).collect();
    cand.sort_by(|x, y| y.0.abs().partial_cmp(&x.0.abs()).unwrap_or(std::cmp::Ordering::Equal));
    cand.truncate(free.len().min(TT_APPLY_MAX));
    let homes: Vec<f64> = cand.iter().map(|c| c.1).collect();
    let mut bands: Vec<Band> = cand.iter().map(|&(v, f)| Band { freq: f, gain: clamp(round1(v), caps.g_min, g_max), q: 4.0, typ: FType::PK }).collect();
    for _ in 0..10 {
        for k in 0..bands.len() {
            let h = homes[k];
            opt_param(&mut bands, k, Param::Gain, caps.g_min, g_max, &fs, &err, false, sp);
            opt_param(&mut bands, k, Param::Q, caps.q_min.max(1.0), caps.q_max.min(TT_APPLY_QMAX), &fs, &err, false, sp);
            opt_param(&mut bands, k, Param::Freq, h / step, top.min(h * step), &fs, &err, true, sp);
        }
    }
    let mut keep: Vec<Band> = bands.into_iter().filter(|b| b.gain.abs() >= 0.5).collect();
    keep.sort_by(|x, y| x.freq.partial_cmp(&y.freq).unwrap_or(std::cmp::Ordering::Equal));
    if keep.is_empty() {
        return TrebleApply::Unchanged { ok: true, msg: "Nothing to add: the current EQ already matches this result in the treble.".into() };
    }
    let mut m = model.to_vec();
    for (k, b) in keep.iter().enumerate() {
        m[free[k]] = *b;
    }
    let (p, _) = crate::eq::peak(&m);
    let preamp = if p + preamp > 0.0 { caps.headroom_for(p) } else { preamp }; // keep headroom
    let left = free.len() - keep.len();
    let msg = format!(
        "Added {} treble band{}{}. Write to the cable to hear it.",
        keep.len(),
        if keep.len() > 1 { "s" } else { "" },
        if left > 0 { format!(" ({} still flat)", left) } else { String::new() }
    );
    TrebleApply::Changed { model: m, preamp, msg }
}
