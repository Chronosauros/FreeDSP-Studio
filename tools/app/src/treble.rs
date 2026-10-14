// The treble listening test's bookkeeping: staircases, loudness pairs, and turning answers into a
// profile. Timing and sound live in the UI and audio modules; this part is pure and tested.
//
// "Stands out": pink noise through the cable. Each trial plays one band on its own as a cue, a
// short gap, then the full noise with a peaking filter of gain g at that band, and asks whether
// the band sticks out. An up/down staircase per band homes in on the g where it just starts to:
// lower where your ear already has a peak, higher where it has a dip.
//
// "Louder / quieter": two bursts of band-limited pink noise, the first (reference) at 0 dB and
// the second at d, compared until they sound the same.
//
// Either way the overall offset and tilt are removed afterwards; what's left are the
// band-to-band differences, i.e. your ear's peaks and dips.

use crate::eq::{clamp, combined_at, round1, Band};
use crate::fr::{detrend, tidy_profile, TT_BANDS};

pub mod rand {
    /// A small xorshift generator: the test only needs unbiased shuffles, not crypto.
    pub struct Rng(u64);
    impl Rng {
        pub fn new() -> Rng {
            let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0x9E37_79B9);
            Rng(seed | 1)
        }
        pub fn next_u64(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }
        /// Uniform in [0, 1).
        pub fn f64(&mut self) -> f64 {
            (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
        }
        pub fn below(&mut self, n: usize) -> usize {
            (self.f64() * n as f64) as usize
        }
    }
    impl Default for Rng {
        fn default() -> Self {
            Rng::new()
        }
    }
}

pub const TT_GMIN: f64 = -12.0;
pub const TT_GMAX: f64 = 15.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Method {
    #[default]
    Noise,
    Match,
}

impl Method {
    pub fn label(self) -> &'static str {
        match self {
            Method::Noise => "stands out",
            Method::Match => "loudness",
        }
    }
    pub fn code(self) -> &'static str {
        match self {
            Method::Noise => "noise",
            Method::Match => "match",
        }
    }
}

/// Up/down staircase for one band: start step 3 dB, halve on every reversal down to 0.5 dB; done
/// after 4 reversals or 10 trials. Threshold = mean of the reversal points after the first.
#[derive(Clone, Debug, PartialEq)]
pub struct Stair {
    pub f: f64,
    pub g: f64,
    pub step: f64,
    pub dir: i32,
    pub n: u32,
    pub revs: Vec<f64>,
    pub done: bool,
}

impl Stair {
    pub fn new(f: f64, start: f64) -> Stair {
        Stair { f, g: start, step: 3.0, dir: 0, n: 0, revs: Vec::new(), done: false }
    }
    pub fn answer(&mut self, heard: bool) {
        let d = if heard { -1 } else { 1 };
        if self.dir != 0 && d != self.dir {
            self.revs.push(self.g);
            self.step = (self.step / 2.0).max(0.5);
        }
        self.dir = d;
        self.n += 1;
        self.g = clamp(self.g + d as f64 * self.step, TT_GMIN, TT_GMAX);
        if self.revs.len() >= 4 || self.n >= 10 {
            self.done = true;
        }
    }
    pub fn value(&self) -> f64 {
        if self.revs.len() >= 2 {
            let r = &self.revs[1..];
            r.iter().sum::<f64>() / r.len() as f64
        } else if !self.revs.is_empty() {
            (self.revs[0] + self.g) / 2.0
        } else {
            self.g
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Loudness {
    Louder,
    Quieter,
    Equal,
}

/// Loudness matching between two bands: "second louder" lowers d, "quieter" raises it, "same"
/// ends the pair. The step starts at 2 dB and halves on each reversal.
#[derive(Clone, Debug, PartialEq)]
pub struct Pair {
    pub reference: usize,
    pub vari: usize,
    pub d: f64,
    pub step: f64,
    pub dir: i32,
    pub n: u32,
    pub revs: Vec<f64>,
    pub eq: bool,
    pub done: bool,
}

impl Pair {
    /// Which band is the reference is random, so order can't bias one side.
    pub fn new(i: usize, j: usize, rng: &mut rand::Rng) -> Pair {
        let (r, v) = if rng.f64() < 0.5 { (i, j) } else { (j, i) };
        Pair::fixed(r, v)
    }
    pub fn fixed(reference: usize, vari: usize) -> Pair {
        Pair { reference, vari, d: 0.0, step: 2.0, dir: 0, n: 0, revs: Vec::new(), eq: false, done: false }
    }
    pub fn answer(&mut self, ans: Loudness) {
        self.n += 1;
        if ans == Loudness::Equal {
            self.eq = true;
            self.done = true;
            return;
        }
        let d = if ans == Loudness::Louder { -1 } else { 1 };
        if self.dir != 0 && d != self.dir {
            self.revs.push(self.d);
            self.step = (self.step / 2.0).max(0.5);
        }
        self.dir = d;
        self.d = clamp(self.d + d as f64 * self.step, -12.0, 12.0);
        if self.revs.len() >= 4 || self.n >= 10 {
            self.done = true;
        }
    }
    pub fn value(&self) -> f64 {
        if self.eq {
            return self.d;
        }
        if self.revs.len() >= 2 {
            let r = &self.revs[1..];
            r.iter().sum::<f64>() / r.len() as f64
        } else if !self.revs.is_empty() {
            (self.revs[0] + self.d) / 2.0
        } else {
            self.d
        }
    }
}

/// Each pair says corr[vari] - corr[ref] = d. Solve all of them at once by least squares (plus
/// sum = 0 to pin the level), so one shaky comparison is outvoted instead of carried along.
/// Returns the corrections and the RMS disagreement.
pub fn solve_pairs(pairs: &[(usize, usize, f64)], n: usize) -> (Vec<f64>, f64) {
    let mut m = vec![vec![0.0; n]; n];
    let mut v = vec![0.0; n];
    let mut add = |row: &[f64], val: f64| {
        for i in 0..n {
            if row[i] == 0.0 {
                continue;
            }
            v[i] += row[i] * val;
            for j in 0..n {
                m[i][j] += row[i] * row[j];
            }
        }
    };
    for &(r, x, d) in pairs {
        let mut row = vec![0.0; n];
        row[x] = 1.0;
        row[r] = -1.0;
        add(&row, d);
    }
    add(&vec![1.0; n], 0.0);
    for c in 0..n {
        // Gaussian elimination with partial pivoting
        let mut p = c;
        for r in c + 1..n {
            if m[r][c].abs() > m[p][c].abs() {
                p = r;
            }
        }
        m.swap(c, p);
        v.swap(c, p);
        for r in 0..n {
            if r == c || m[c][c] == 0.0 {
                continue;
            }
            let k = m[r][c] / m[c][c];
            for j in c..n {
                m[r][j] -= k * m[c][j];
            }
            v[r] -= k * v[c];
        }
    }
    let x: Vec<f64> = (0..n).map(|i| if m[i][i] != 0.0 { v[i] / m[i][i] } else { 0.0 }).collect();
    let res: Vec<f64> = pairs.iter().map(|&(r, xi, d)| d - (x[xi] - x[r])).collect();
    let rms = (res.iter().map(|e| e * e).sum::<f64>() / res.len().max(1) as f64).sqrt();
    (x, rms)
}

#[derive(Clone, Debug)]
pub enum Item {
    Stair(Stair),
    Pair(Pair),
}

impl Item {
    pub fn done(&self) -> bool {
        match self {
            Item::Stair(s) => s.done,
            Item::Pair(p) => p.done,
        }
    }
    pub fn n(&self) -> u32 {
        match self {
            Item::Stair(s) => s.n,
            Item::Pair(p) => p.n,
        }
    }
}

/// A finished test, before it is saved as a profile.
#[derive(Clone, Debug)]
pub struct Result {
    pub name: String,
    pub method: Method,
    pub bands: Vec<[f64; 2]>,
    pub passes: usize,
    pub thresholds: Option<Vec<[f64; 2]>>,
    pub pairs: Option<Vec<[f64; 3]>>,
    pub eq_during: Vec<Band>,
    pub date: String,
    pub warn: String,
}

pub fn profile_name(tip: &str, m: Method) -> String {
    format!("{} · {}", tip, m.label())
}

/// A running test: its passes, which item is up, and enough history to undo an answer.
pub struct Session {
    pub tip: String,
    pub method: Method,
    pub passes: Vec<Vec<Item>>,
    pub pass: usize,
    pub cur: Option<usize>,
    pub last: Option<usize>,
    pub eq_during: Vec<Band>,
    pub paused: bool,
    pub asking: bool,
    pub wait_reseat: bool,
    pub undo: Vec<(usize, usize, Item)>,
    rng: rand::Rng,
}

pub enum Next {
    /// Play this item of the current pass.
    Play(usize),
    /// Halfway: ask the listener to re-seat the IEMs.
    Reseat,
    /// All done.
    Finished,
}

impl Session {
    pub fn new(tip: String, method: Method, thorough: bool, eq_during: Vec<Band>) -> Session {
        let mut rng = rand::Rng::new();
        let n = TT_BANDS.len();
        let passes = match method {
            Method::Match => {
                let adj: Vec<Item> = (0..n - 1).map(|i| Item::Pair(Pair::new(i, i + 1, &mut rng))).collect(); // neighbours
                let skip: Vec<Item> = (0..n - 2).map(|i| Item::Pair(Pair::new(i, i + 2, &mut rng))).collect(); // one apart, as a cross-check
                if thorough {
                    vec![adj, skip]
                } else {
                    vec![adj]
                }
            }
            Method::Noise => {
                if thorough {
                    // from below, then from above
                    vec![TT_BANDS.iter().map(|&f| Item::Stair(Stair::new(f, -3.0))).collect(), TT_BANDS.iter().map(|&f| Item::Stair(Stair::new(f, 9.0))).collect()]
                } else {
                    vec![TT_BANDS.iter().map(|&f| Item::Stair(Stair::new(f, 3.0))).collect()]
                }
            }
        };
        Session { tip, method, passes, pass: 0, cur: None, last: None, eq_during, paused: false, asking: false, wait_reseat: false, undo: Vec::new(), rng }
    }

    pub fn progress(&self) -> f64 {
        let (mut done, mut tot) = (0u32, 0u32);
        for p in &self.passes {
            for s in p {
                tot += 10;
                done += if s.done() { 10 } else { s.n().min(9) };
            }
        }
        done as f64 / tot.max(1) as f64
    }

    /// Picks the next trial: random order, but not the same item twice running when there's a
    /// choice.
    pub fn next(&mut self) -> Next {
        let open: Vec<usize> = (0..self.passes[self.pass].len()).filter(|&i| !self.passes[self.pass][i].done()).collect();
        if open.is_empty() {
            if self.pass + 1 < self.passes.len() {
                self.pass += 1;
                self.wait_reseat = true;
                self.asking = false;
                return Next::Reseat;
            }
            return Next::Finished;
        }
        let mut sh = open.clone();
        for i in (1..sh.len()).rev() {
            let j = self.rng.below(i + 1);
            sh.swap(i, j);
        }
        let pick = sh.iter().copied().find(|&s| Some(s) != self.last).unwrap_or(open[0]);
        self.cur = Some(pick);
        self.last = Some(pick);
        Next::Play(pick)
    }

    pub fn current(&self) -> Option<&Item> {
        self.cur.map(|i| &self.passes[self.pass][i])
    }

    pub fn answer_stair(&mut self, heard: bool) {
        let Some(i) = self.cur else { return };
        let it = self.passes[self.pass][i].clone();
        if let Item::Stair(s) = &mut self.passes[self.pass][i] {
            self.undo.push((self.pass, i, it));
            s.answer(heard);
        }
    }

    pub fn answer_pair(&mut self, ans: Loudness) {
        let Some(i) = self.cur else { return };
        let it = self.passes[self.pass][i].clone();
        if let Item::Pair(p) = &mut self.passes[self.pass][i] {
            self.undo.push((self.pass, i, it));
            p.answer(ans);
        }
    }

    /// Takes back the last answer, even across the re-seat break. Returns whether it did.
    pub fn undo(&mut self) -> bool {
        let Some((pass, i, item)) = self.undo.pop() else { return false };
        self.passes[pass][i] = item;
        self.pass = pass;
        self.cur = Some(i);
        self.last = Some(i);
        self.wait_reseat = false;
        true
    }

    pub fn finish(&self, date: String) -> Result {
        let n = TT_BANDS.len();
        let mut warn = String::new();
        let (raw, thresholds, pairs): (Vec<f64>, Option<Vec<[f64; 2]>>, Option<Vec<[f64; 3]>>) = match self.method {
            Method::Match => {
                let pairs: Vec<(usize, usize, f64)> = self
                    .passes
                    .iter()
                    .flatten()
                    .filter_map(|it| if let Item::Pair(p) = it { Some((p.reference, p.vari, p.value())) } else { None })
                    .collect();
                let (x, rms) = solve_pairs(&pairs, n);
                // With the cross-check pass the comparisons over-determine the answer, so their
                // disagreement says how consistent the listening was.
                if self.passes.len() > 1 && rms > 2.0 {
                    warn = format!("Your comparisons disagreed with each other by about {} dB on average. Consider running it again.", crate::eq::fixed(rms, 1));
                }
                let pp = pairs.iter().map(|&(r, v, d)| [TT_BANDS[r], TT_BANDS[v], round1(d)]).collect();
                (x, None, Some(pp))
            }
            Method::Noise => {
                // threshold per band, averaged over passes; a lower threshold means a peak, so the EQ should cut
                let val = |p: &Vec<Item>, i: usize| if let Item::Stair(s) = &p[i] { s.value() } else { 0.0 };
                let thr: Vec<f64> = (0..n).map(|i| self.passes.iter().map(|p| val(p, i)).sum::<f64>() / self.passes.len() as f64).collect();
                // How far apart the two passes landed, as a rough reliability check.
                if self.passes.len() > 1 {
                    let spread = (0..n).map(|i| (val(&self.passes[0], i) - val(&self.passes[1], i)).abs()).fold(0.0, f64::max);
                    if spread > 4.0 {
                        warn = format!(
                            "The two passes differed by up to {} dB at one band. Fit or concentration may have changed between them; consider running it again.",
                            crate::eq::fixed(spread, 1)
                        );
                    }
                }
                let th = thr.iter().enumerate().map(|(i, &g)| [TT_BANDS[i], round1(g)]).collect();
                (thr, Some(th), None)
            }
        };
        // Add back the cable EQ that was playing: the profile describes the bare IEM in your ear,
        // so it stays valid whatever EQ ends up on the cable.
        let pts: Vec<[f64; 2]> = TT_BANDS.iter().enumerate().map(|(i, &f)| [f, raw[i] + combined_at(&self.eq_during, f)]).collect();
        let bands = tidy_profile(&detrend(&pts));
        Result {
            name: profile_name(&self.tip, self.method),
            method: self.method,
            bands,
            passes: self.passes.len(),
            thresholds,
            pairs,
            eq_during: self.eq_during.clone(),
            date,
            warn,
        }
    }
}
