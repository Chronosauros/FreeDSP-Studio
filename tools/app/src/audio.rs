// Sound for the treble listening test: pink noise through two paths, a band-pass "cue" and a
// peaking-filtered "full" signal, each with its own gain. The UI schedules changes against the
// sample clock (like Web Audio's setTargetAtTime, which the test was designed around); the audio
// callback applies them on time and glides each gain with a 12 ms time constant so nothing clicks.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

const MASTER: f32 = 0.35;
const TAU: f64 = 0.012;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Gain {
    Cue,
    Full,
}

#[derive(Clone, Copy, Debug)]
enum Cmd {
    /// Glide this gain towards `v` from sample `at`; drops anything later queued for it.
    Ramp { g: Gain, v: f32, at: u64 },
    /// Band-pass centre and Q, from sample `at`.
    Bp { f: f64, q: f64, at: u64 },
    /// Peaking filter, from sample `at`.
    Pk { f: f64, q: f64, gain: f64, at: u64 },
}

impl Cmd {
    fn at(&self) -> u64 {
        match *self {
            Cmd::Ramp { at, .. } | Cmd::Bp { at, .. } | Cmd::Pk { at, .. } => at,
        }
    }
}

pub struct Audio {
    tx: Sender<Cmd>,
    clock: Arc<AtomicU64>,
    rate: f64,
    _stream: cpal::Stream,
}

impl Audio {
    /// Opens the default output. Fails if there is no audio device.
    pub fn open() -> Result<Audio, String> {
        let host = cpal::default_host();
        let dev = host.default_output_device().ok_or("no audio output device")?;
        let cfg = dev.default_output_config().map_err(|e| e.to_string())?;
        let rate = cfg.sample_rate() as f64;
        let ch = cfg.channels() as usize;
        let (tx, rx) = channel();
        let clock = Arc::new(AtomicU64::new(0));
        let mut eng = Engine::new(rate, rx, clock.clone());
        let err = |e| eprintln!("audio: {}", e);
        let sc: cpal::StreamConfig = cfg.clone().into();
        let stream = match cfg.sample_format() {
            cpal::SampleFormat::F32 => dev.build_output_stream(
                sc.clone(),
                move |out: &mut [f32], _| {
                    for fr in out.chunks_mut(ch) {
                        let s = eng.tick();
                        fr.fill(s);
                    }
                },
                err,
                None,
            ),
            cpal::SampleFormat::I16 => dev.build_output_stream(
                sc.clone(),
                move |out: &mut [i16], _| {
                    for fr in out.chunks_mut(ch) {
                        let s = (eng.tick().clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                        fr.fill(s);
                    }
                },
                err,
                None,
            ),
            cpal::SampleFormat::U16 => dev.build_output_stream(
                sc.clone(),
                move |out: &mut [u16], _| {
                    for fr in out.chunks_mut(ch) {
                        let s = ((eng.tick().clamp(-1.0, 1.0) * 0.5 + 0.5) * u16::MAX as f32) as u16;
                        fr.fill(s);
                    }
                },
                err,
                None,
            ),
            f => return Err(format!("unsupported sample format {:?}", f)),
        }
        .map_err(|e| e.to_string())?;
        stream.play().map_err(|e| e.to_string())?;
        Ok(Audio { tx, clock, rate, _stream: stream })
    }

    /// The current time in seconds, on the audio clock.
    pub fn now(&self) -> f64 {
        self.clock.load(Ordering::Relaxed) as f64 / self.rate
    }

    fn at(&self, t: f64) -> u64 {
        (t.max(0.0) * self.rate) as u64
    }

    pub fn ramp(&self, g: Gain, v: f32, t: f64) {
        let _ = self.tx.send(Cmd::Ramp { g, v, at: self.at(t) });
    }

    pub fn bandpass(&self, f: f64, q: f64, t: f64) {
        let _ = self.tx.send(Cmd::Bp { f, q, at: self.at(t) });
    }

    pub fn peaking(&self, f: f64, q: f64, gain: f64, t: f64) {
        let _ = self.tx.send(Cmd::Pk { f, q, gain, at: self.at(t) });
    }

    /// Both paths to silence, now.
    pub fn silence(&self) {
        let t = self.now();
        self.ramp(Gain::Cue, 0.0, t);
        self.ramp(Gain::Full, 0.0, t);
    }
}

/// Transposed direct form II biquad.
#[derive(Default, Clone, Copy)]
struct Bq {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl Bq {
    fn set(&mut self, b0: f64, b1: f64, b2: f64, a0: f64, a1: f64, a2: f64) {
        self.b0 = b0 / a0;
        self.b1 = b1 / a0;
        self.b2 = b2 / a0;
        self.a1 = a1 / a0;
        self.a2 = a2 / a0;
    }
    /// Constant 0 dB peak gain band-pass (what Web Audio's "bandpass" is).
    fn bandpass(&mut self, f: f64, q: f64, fs: f64) {
        let w = 2.0 * std::f64::consts::PI * f / fs;
        let al = w.sin() / (2.0 * q);
        self.set(al, 0.0, -al, 1.0 + al, -2.0 * w.cos(), 1.0 - al);
    }
    fn peaking(&mut self, f: f64, q: f64, g: f64, fs: f64) {
        let c = crate::eq::rbj(crate::eq::FType::PK, f, g, q, fs);
        self.set(c.b0, c.b1, c.b2, c.a0, c.a1, c.a2);
    }
    fn run(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

struct Engine {
    rate: f64,
    rx: Receiver<Cmd>,
    queue: Vec<Cmd>,
    clock: Arc<AtomicU64>,
    t: u64,
    pink: [f64; 7],
    seed: u64,
    bp: Bq,
    pk: Bq,
    cue: (f64, f64), // (current, target)
    full: (f64, f64),
    k: f64,
}

impl Engine {
    fn new(rate: f64, rx: Receiver<Cmd>, clock: Arc<AtomicU64>) -> Engine {
        let mut e = Engine {
            rate,
            rx,
            queue: Vec::new(),
            clock,
            t: 0,
            pink: [0.0; 7],
            seed: 0x2545_F491_4F6C_DD1D,
            bp: Bq::default(),
            pk: Bq::default(),
            cue: (0.0, 0.0),
            full: (0.0, 0.0),
            k: 1.0 - (-1.0 / (TAU * rate)).exp(),
        };
        e.bp.bandpass(8000.0, 3.5, rate);
        e.pk.peaking(8000.0, 3.5, 0.0, rate);
        e
    }

    fn white(&mut self) -> f64 {
        let mut x = self.seed;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.seed = x;
        (x >> 11) as f64 / (1u64 << 52) as f64 - 1.0
    }

    /// Paul Kellet's refined pink filter on white noise.
    fn pink(&mut self) -> f64 {
        let w = self.white();
        let b = &mut self.pink;
        b[0] = 0.99886 * b[0] + w * 0.0555179;
        b[1] = 0.99332 * b[1] + w * 0.0750759;
        b[2] = 0.96900 * b[2] + w * 0.1538520;
        b[3] = 0.86650 * b[3] + w * 0.3104856;
        b[4] = 0.55000 * b[4] + w * 0.5329522;
        b[5] = -0.7616 * b[5] - w * 0.0168980;
        let out = (b[0] + b[1] + b[2] + b[3] + b[4] + b[5] + b[6] + w * 0.5362) * 0.11;
        b[6] = w * 0.115926;
        out
    }

    fn tick(&mut self) -> f32 {
        if self.t % 64 == 0 {
            while let Ok(c) = self.rx.try_recv() {
                if let Cmd::Ramp { g, at, .. } = c {
                    self.queue.retain(|q| !matches!(q, Cmd::Ramp { g: g2, at: a2, .. } if *g2 == g && *a2 >= at));
                }
                self.queue.push(c);
            }
            self.clock.store(self.t, Ordering::Relaxed);
        }
        if !self.queue.is_empty() {
            let now = self.t;
            let mut due: Vec<Cmd> = Vec::new();
            self.queue.retain(|c| {
                if c.at() <= now {
                    due.push(*c);
                    false
                } else {
                    true
                }
            });
            due.sort_by_key(|c| c.at());
            for c in due {
                match c {
                    Cmd::Ramp { g: Gain::Cue, v, .. } => self.cue.1 = v as f64,
                    Cmd::Ramp { g: Gain::Full, v, .. } => self.full.1 = v as f64,
                    Cmd::Bp { f, q, .. } => self.bp.bandpass(f, q, self.rate),
                    Cmd::Pk { f, q, gain, .. } => self.pk.peaking(f, q, gain, self.rate),
                }
            }
        }
        self.cue.0 += (self.cue.1 - self.cue.0) * self.k;
        self.full.0 += (self.full.1 - self.full.0) * self.k;
        let n = self.pink();
        let mut s = 0.0;
        if self.cue.0.abs() > 1e-6 || self.cue.1 != 0.0 {
            s += self.bp.run(n) * self.cue.0;
        } else {
            self.bp.run(n);
        }
        if self.full.0.abs() > 1e-6 || self.full.1 != 0.0 {
            s += self.pk.run(n) * self.full.0;
        } else {
            self.pk.run(n);
        }
        self.t += 1;
        (s as f32) * MASTER
    }
}
