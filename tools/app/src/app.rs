// The app's state and what happens to it: the editor model, the connected device or offline
// target, the curves, bank sets and dialogs. Drawing is in ui.rs and dialogs.rs; this file is the
// port of the HTML version's logic, function for function where it can be.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use eframe::egui;
use indexmap::IndexMap;

use crate::device::{self, Reply, Worker};
use crate::eq::{self, clamp, fixed, Band, Caps, FType, Target};
use crate::fit::{self, FitCfg, FitMode};
use crate::fr::{self, Smoothed};
use crate::hid;
use crate::store::{self, FrStore, SavedEqs, Settings, SetsDb, Theme};
use crate::theme::{self, Pal};
use crate::treble;

pub fn run() -> Result<(), String> {
    let mut vp = egui::ViewportBuilder::default()
        .with_title("FreeDSP Studio")
        .with_app_id("freedsp-studio")
        .with_inner_size([1280.0, 820.0])
        .with_min_inner_size([1000.0, 700.0])
        .with_icon(icon());
    // macOS keeps its real traffic lights over our header; elsewhere the header is the title bar.
    if cfg!(target_os = "macos") {
        vp = vp.with_fullsize_content_view(true).with_titlebar_shown(false).with_title_shown(false);
    } else {
        vp = vp.with_decorations(false);
    }
    let opts = eframe::NativeOptions { viewport: vp, centered: true, ..Default::default() };
    eframe::run_native("FreeDSP Studio", opts, Box::new(|cc| Ok(Box::new(App::new(cc))))).map_err(|e| e.to_string())
}

fn icon() -> egui::IconData {
    // 128x128 RGBA, converted from assets/icons/128x128.png at build time would need an image
    // decoder; the raw pixels are small enough to keep beside it.
    egui::IconData { rgba: include_bytes!("../assets/icons/icon-128.rgba").to_vec(), width: 128, height: 128 }
}

pub const BTR17_ID: &str = "fiio-btr17";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Built-in starting points, shown first in Saved EQs so FreeDSP (no firmware presets) can still
/// load the DUSK voicings. Kept independent of Moondrop's own data.
pub const STARTING: [(&str, &[(f64, f64, f64)]); 5] = [
    ("DUSK Default", &[(1400.0, -3.0, 0.8), (5400.0, -3.0, 2.0), (14000.0, -5.0, 2.0)]),
    ("DUSK V", &[(250.0, -3.0, 0.4), (1500.0, -6.0, 0.6), (5400.0, -3.0, 2.0), (14000.0, -5.0, 2.0)]),
    ("DUSK Harman", &[(80.0, -2.0, 0.2), (400.0, -3.0, 0.4), (1600.0, -4.0, 1.2), (14000.0, -10.0, 2.0)]),
    (
        "DUSK Bass+",
        &[(140.0, 1.0, 0.4), (200.0, -1.0, 3.0), (300.0, -6.0, 0.4), (1500.0, -7.0, 0.6), (5400.0, -3.0, 2.5), (8000.0, -5.0, 0.4), (14000.0, -7.0, 2.0)],
    ),
    ("DUSK Diffuse", &[(65.0, -7.0, 0.7), (1500.0, -5.0, 0.4), (2900.0, 3.0, 2.5), (6000.0, -2.0, 3.0), (14000.0, -10.0, 2.0)]),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Modal {
    EqText,
    Backup,
    Saved,
    Banks,
    Drop,
    Treble,
    Log,
    Clip,
    About,
    Fr,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GraphMode {
    Eq,
    Fr,
}

#[derive(Clone)]
struct Snap {
    model: Vec<Band>,
    preamp: f64,
    hi_gain: bool,
    g_ceil: f64,
}

pub struct Offline {
    pub target: Target,
    pub bands: usize,
}

/// A curve dropped on the window, waiting for a name, a rig and a role.
pub struct DropItem {
    pub name: String,
    pub file: String,
    pub points: fr::Curve,
    pub channels: usize,
    pub single: Option<char>,
    pub rig: String,
    pub meas: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TtView {
    Setup,
    Run,
    Done,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TtEv {
    Ask,
    PhaseClear,
    PhaseSecond,
    Next,
}

/// The treble test window's state.
pub struct TtUi {
    pub view: TtView,
    pub method: treble::Method,
    pub thorough: bool,
    pub tip: String,
    pub warn: String,
    pub warn_ok: bool,
    pub session: Option<treble::Session>,
    pub result: Option<treble::Result>,
    pub done_warn: String,
    pub done_ok: bool,
    pub timers: Vec<(Instant, TtEv)>,
    pub ask_text: String,
    pub phase: String,
    pub phase_ask: bool,
    pub freq: String,
    pub count: String,
    pub level_on: bool,
    pub audio: Option<crate::audio::Audio>,
    pub audio_err: Option<String>,
}

pub struct App {
    pub pal: Pal,
    pub settings: Settings,
    pub worker: Worker,
    pub log: Vec<String>,

    // device or offline target
    pub cable: Option<hid::CableInfo>,
    pub offline: Option<Offline>,
    pub cur_target: Option<Target>,
    pub targets: IndexMap<String, Target>,
    pub sel_bank: Option<u8>,
    pub active_bank: Option<i32>,
    pub caps: Caps,
    pub hi_gain: bool,
    pub g_ceil: f64,
    pub db_min: f64,
    pub cable_eq: Option<Vec<Band>>,

    // the editor
    pub model: Vec<Band>,
    pub preamp: f64,
    history: Vec<Snap>,
    pub graph_mode: GraphMode,
    pub drag_node: Option<usize>,
    pub edit: HashMap<(usize, eq::Field), String>,
    pub edit_undone: bool,
    pub preamp_text: Option<String>,

    // connect screen
    pub connected: bool,
    pub connect_title: String,
    pub device_list: Vec<hid::Listed>,
    pub off_target: String,
    pub off_bands: usize,

    // curves and fitting
    pub frs: FrStore,
    smooth: HashMap<(bool, String), (usize, Smoothed)>,
    pub fit: FitCfg,
    pub treble_view: bool,

    pub saved: SavedEqs,
    pub sets: SetsDb,

    // dialogs and notes
    pub modal: Option<Modal>,
    pub eq_text: String,
    pub eq_text_msg: String,
    pub backup_msg: Option<(String, Instant)>,
    pub save_name: String,
    pub fr_msg: String,
    pub bank_msg: (String, bool),
    pub bank_busy: bool,
    pub write_all_armed: bool,
    pub set_rename: Option<String>,
    pub del_armed: Option<(String, Instant)>,
    pub bank_drag: Option<u8>,
    pub drop_queue: VecDeque<DropItem>,
    pub drop_cur: Option<DropItem>,
    pub clip_info: (f64, f64),
    pub tt: TtUi,
    pub treble_msg: Option<(String, bool, Instant)>,
    pub write_note: Option<String>,
    pub write_label: Option<(String, Instant)>,
    pub writing: bool,
    pub toast: Option<(String, bool, Instant)>,
    pub read_note: bool,
    pub read_flash: Option<Instant>,
    pub copied: Option<Instant>,
    pub hovering_files: bool,
}

impl App {
    fn new(cc: &eframe::CreationContext) -> App {
        let ctx = &cc.egui_ctx;
        ctx.set_fonts(theme::fonts());
        let settings = Settings::load();
        let dark = match settings.theme {
            Theme::Dark => true,
            Theme::Light => false,
            Theme::System => ctx.global_style().visuals.dark_mode,
        };
        let pal = if dark { theme::DARK } else { theme::LIGHT };
        theme::apply(ctx, &pal);
        let caps = Caps::default();
        let mut targets = IndexMap::new();
        let btr = Target {
            id: BTR17_ID.into(),
            name: "FIIO BTR17".into(),
            gen: 3,
            bands: 10,
            g_min: -24.0,
            g_max: 12.0,
            q_min: 0.25,
            q_max: 8.0,
            types: vec![FType::PK, FType::LSQ, FType::HSQ],
            presets: (0..10).map(|i| (160 + i as u8, format!("USER{}", i + 1))).chain([(240u8, "Bypass".to_string())]).collect(),
            user_slots: (160..170).collect(),
            tested: true,
        };
        targets.insert(btr.id.clone(), btr);
        for x in hid::bank_targets() {
            let t = target_from_bank(&x);
            targets.entry(t.id.clone()).or_insert(t);
        }
        let generic = Target {
            id: "generic".into(),
            name: "Generic".into(),
            gen: 2,
            bands: 10,
            g_min: -12.0,
            g_max: 12.0,
            q_min: 0.1,
            q_max: 10.0,
            types: vec![FType::PK, FType::LSQ, FType::HSQ],
            presets: Default::default(),
            user_slots: vec![],
            tested: true,
        };
        targets.insert(generic.id.clone(), generic);
        let off_target = settings.offline_target.clone().filter(|t| targets.contains_key(t)).unwrap_or_else(|| BTR17_ID.into());
        let off_bands = settings.offline_bands.unwrap_or(10);
        let tt_method = if settings.tt_method == "match" { treble::Method::Match } else { treble::Method::Noise };
        let mut fit = settings.fit_cfg();
        fit.bands = fit.bands.clamp(1, caps.bands);
        let mut app = App {
            pal,
            worker: Worker::new(ctx.clone()),
            log: Vec::new(),
            cable: None,
            offline: None,
            cur_target: None,
            targets,
            sel_bank: None,
            active_bank: None,
            model: caps.blank_model(),
            caps,
            hi_gain: false,
            g_ceil: 6.0,
            db_min: -15.0,
            cable_eq: None,
            preamp: 0.0,
            history: Vec::new(),
            graph_mode: GraphMode::Eq,
            drag_node: None,
            edit: HashMap::new(),
            edit_undone: false,
            preamp_text: None,
            connected: false,
            connect_title: "Looking for a cable…".into(),
            device_list: Vec::new(),
            off_target,
            off_bands,
            frs: FrStore::load(),
            smooth: HashMap::new(),
            fit,
            treble_view: settings.treble_view,
            saved: store::load_saved(),
            sets: SetsDb::load(),
            modal: None,
            eq_text: String::new(),
            eq_text_msg: String::new(),
            backup_msg: None,
            save_name: String::new(),
            fr_msg: String::new(),
            bank_msg: (String::new(), false),
            bank_busy: false,
            write_all_armed: false,
            set_rename: None,
            del_armed: None,
            bank_drag: None,
            drop_queue: VecDeque::new(),
            drop_cur: None,
            clip_info: (0.0, 0.0),
            tt: TtUi {
                view: TtView::Setup,
                method: tt_method,
                thorough: true,
                tip: String::new(),
                warn: String::new(),
                warn_ok: false,
                session: None,
                result: None,
                done_warn: String::new(),
                done_ok: false,
                timers: Vec::new(),
                ask_text: String::new(),
                phase: String::new(),
                phase_ask: false,
                freq: "—".into(),
                count: String::new(),
                level_on: false,
                audio: None,
                audio_err: None,
            },
            treble_msg: None,
            write_note: None,
            write_label: None,
            writing: false,
            toast: None,
            read_note: false,
            read_flash: None,
            copied: None,
            hovering_files: false,
            settings,
        };
        app.set_db_range();
        app.detect();
        app
    }

    pub fn log(&mut self, m: impl Into<String>) {
        let m = m.into();
        self.log.push(m);
        if self.log.len() > 2000 {
            self.log.drain(..500);
        }
    }

    pub fn save_settings(&mut self) {
        self.settings.set_fit_cfg(&self.fit);
        self.settings.treble_view = self.treble_view;
        self.settings.tt_method = self.tt.method.code().into();
        if let Err(e) = self.settings.save() {
            self.log(format!("saving settings failed: {}", e));
        }
    }

    pub fn set_theme(&mut self, ctx: &egui::Context, dark: bool) {
        self.pal = if dark { theme::DARK } else { theme::LIGHT };
        theme::apply(ctx, &self.pal);
        self.settings.theme = if dark { Theme::Dark } else { Theme::Light };
        self.save_settings();
    }

    pub fn toast(&mut self, msg: impl Into<String>, warn: bool) {
        self.toast = Some((msg.into(), warn, Instant::now() + Duration::from_millis(if warn { 6000 } else { 3500 })));
    }

    // ---- limits ----

    pub fn cur_g_max(&self) -> f64 {
        if self.hi_gain {
            self.g_ceil
        } else {
            self.caps.g_max
        }
    }

    /// The graph floor follows the device: -15 dB suits the cables that stop at -12, but a FiiO
    /// band goes down to -24, so the floor drops to keep a few dB of room below the lowest gain.
    pub fn set_db_range(&mut self) {
        let g = self.caps.g_min;
        self.db_min = if g < -12.0 { ((g - 3.0) / 3.0).floor() * 3.0 } else { -15.0 };
    }

    pub fn gain_scale(&self) -> f64 {
        self.caps.g_min.abs().max(self.cur_g_max()).max(1.0)
    }

    /// Gen-1 cables accept more gain than the app offers by default, which is what "+6 dB mode"
    /// unlocks. Gen-2 cables publish a real ceiling, so there is nothing to raise.
    fn ensure_ceil_for(&mut self, max_gain: f64) {
        if self.caps.gen == 1 && max_gain > self.caps.g_max {
            self.hi_gain = true;
            self.g_ceil = clamp(self.g_ceil.max(max_gain.ceil()), 3.0, 12.0);
        }
    }

    /// Reshapes the editor around whatever device or target just opened.
    fn apply_caps(&mut self, c: Caps) {
        self.caps = c;
        if self.caps.gen != 1 {
            self.hi_gain = false;
        }
        self.set_db_range();
        self.fit.bands = if self.fit.bands == 0 { self.caps.bands } else { self.fit.bands }.clamp(1, self.caps.bands);
        // drop any band the new device doesn't have, and any filter type it can't run
        let types = self.caps.types.clone();
        let m: Vec<Band> = self.model.iter().map(|b| Band { typ: if types.contains(&b.typ) { b.typ } else { FType::PK }, ..*b }).collect();
        self.model = self.caps.fill(&m);
    }

    fn reset_caps(&mut self) {
        self.caps = Caps::default();
        self.set_db_range();
    }

    // ---- undo ----

    fn snap(&self) -> Snap {
        Snap { model: self.model.clone(), preamp: self.preamp, hi_gain: self.hi_gain, g_ceil: self.g_ceil }
    }

    pub fn push_undo(&mut self) {
        let s = self.snap();
        if let Some(l) = self.history.last() {
            if eq::same_eq(&l.model, &s.model) && l.preamp == s.preamp && l.hi_gain == s.hi_gain && l.g_ceil == s.g_ceil {
                return;
            }
        }
        self.history.push(s);
        if self.history.len() > 60 {
            self.history.remove(0);
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.history.is_empty()
    }

    pub fn undo(&mut self) {
        if let Some(s) = self.history.pop() {
            self.model = s.model;
            self.preamp = s.preamp;
            self.hi_gain = s.hi_gain;
            self.g_ceil = s.g_ceil;
            self.edit.clear();
        }
    }

    // ---- preamp and clipping ----

    pub fn set_preamp(&mut self, v: f64) {
        self.preamp = self.caps.pre_round(v);
    }

    pub fn auto_preamp_value(&self) -> f64 {
        self.caps.headroom_for(eq::peak(&self.model).0)
    }

    /// (effective peak, peak frequency)
    pub fn eff_peak(&self) -> (f64, f64) {
        let (p, pf) = eq::peak(&self.model);
        (p + self.preamp, pf)
    }

    pub fn clip_text(&self) -> (String, bool) {
        let (eff, pf) = self.eff_peak();
        let with = if self.preamp != 0.0 { format!(" (with {} dB preamp)", fixed(self.preamp, 1)) } else { String::new() };
        if eff > 0.1 {
            (format!("Peaks at +{} dB @ {} Hz{}, may clip.", fixed(eff, 1), eq::js_num(pf), with), true)
        } else {
            (format!("Peaks at {}{} dB{}, no clipping expected.", if eff > 0.0 { "+" } else { "" }, fixed(eff, 1), with), false)
        }
    }

    // ---- model changes ----

    pub fn blank_model(&self) -> Vec<Band> {
        self.caps.blank_model()
    }

    pub fn flatten(&mut self) {
        self.push_undo();
        self.model = self.blank_model();
        self.preamp = 0.0;
        self.edit.clear();
    }

    pub fn sort_bands(&mut self) {
        self.push_undo();
        self.model.sort_by(|a, b| a.freq.partial_cmp(&b.freq).unwrap_or(std::cmp::Ordering::Equal));
        self.edit.clear();
    }

    pub fn cycle_type(&mut self, i: usize) {
        self.push_undo();
        let t = &self.caps.types;
        let k = t.iter().position(|x| *x == self.model[i].typ).map(|k| (k + 1) % t.len()).unwrap_or(0);
        self.model[i].typ = t[k];
    }

    pub fn toggle_hi_gain(&mut self) {
        self.push_undo();
        self.hi_gain = !self.hi_gain;
        if !self.hi_gain {
            let g = self.caps.g_max;
            for b in &mut self.model {
                if b.gain > g {
                    b.gain = g;
                }
            }
        }
    }

    fn apply_eq(&mut self, bands: &[Band], pre: f64, label: &str) {
        self.push_undo();
        let mx = bands.iter().map(|b| b.gain).fold(0.0, f64::max);
        self.ensure_ceil_for(mx);
        self.model = self.caps.fill(bands);
        self.preamp = self.caps.pre_round(pre);
        self.edit.clear();
        self.log(format!("loaded {}", label));
    }

    pub fn apply_parsed(&mut self, p: &eq::Parsed) -> String {
        let mx = p.filters.iter().map(|b| b.gain).fold(0.0, f64::max);
        self.ensure_ceil_for(mx);
        let (m, pre, msg) = eq::apply_filters(p, &self.caps, self.cur_g_max());
        self.push_undo();
        self.model = m;
        self.preamp = pre;
        self.edit.clear();
        msg
    }

    pub fn eq_text_now(&self) -> String {
        eq::eq_to_text(&self.model, self.preamp)
    }

    // ---- saved EQs ----

    pub fn persist_saved(&mut self) {
        if let Err(e) = store::save_saved(&self.saved) {
            self.log(format!("save failed: {}", e));
        }
    }

    pub fn save_current(&mut self) -> bool {
        let name = self.save_name.trim().to_string();
        if name.is_empty() {
            return false;
        }
        self.saved.insert(name.clone(), store::SavedEq { bands: self.model.clone(), preamp: self.preamp });
        self.persist_saved();
        self.save_name.clear();
        self.log(format!("saved EQ \"{}\"", name));
        true
    }

    pub fn load_starting(&mut self, i: usize) {
        let (n, bands) = STARTING[i];
        let b: Vec<Band> = bands.iter().map(|&(f, g, q)| Band::pk(f, g, q)).collect();
        self.apply_eq(&b, 0.0, &format!("built-in \"{}\"", n));
    }

    pub fn load_saved(&mut self, name: &str) {
        if let Some(e) = self.saved.get(name).cloned() {
            self.apply_eq(&e.bands, e.preamp, &format!("saved EQ \"{}\"", name));
        }
    }

    // ---- curves ----

    pub fn persist_frs(&mut self) {
        if let Err(e) = self.frs.save() {
            self.log(format!("FR save failed: {}", e));
        }
    }

    /// A curve with its smoothed copy, cached until the curve changes.
    pub fn smoothed(&mut self, meas: bool, name: &str) -> Option<Smoothed> {
        let rec = if meas { self.frs.meas.get(name)? } else { self.frs.targets.get(name)? };
        let key = (meas, name.to_string());
        let sig = rec.points.len() ^ (rec.points.first().map(|p| p[1].to_bits() as usize).unwrap_or(0));
        if let Some((s, sm)) = self.smooth.get(&key) {
            if *s == sig {
                return Some(sm.clone());
            }
        }
        let sm = Smoothed::new(&rec.points);
        self.smooth.insert(key, (sig, sm.clone()));
        Some(sm)
    }

    pub fn cur_meas_sm(&mut self) -> Option<Smoothed> {
        let n = self.frs.cur_meas.clone()?;
        self.smoothed(true, &n)
    }

    pub fn cur_tgt_sm(&mut self) -> Option<Smoothed> {
        let t = self.frs.cur_tgt.clone()?;
        if let Some(n) = t.strip_prefix("m:") {
            self.smoothed(true, n)
        } else {
            self.smoothed(false, t.strip_prefix("t:").unwrap_or(&t))
        }
    }

    pub fn active_profile(&self) -> Option<fr::Curve> {
        self.frs.active_profile().map(|(_, p)| p.bands.clone())
    }

    pub fn set_fr_rig(&mut self, rig: &str) {
        if rig == self.frs.rig {
            return;
        }
        self.frs.rig = rig.into();
        if let Some(m) = &self.frs.cur_meas {
            if self.frs.meas.get(m).map(|r| r.rig != rig).unwrap_or(true) {
                self.frs.cur_meas = None;
            }
        }
        if self.frs.tgt_rec().map(|r| r.rig != rig).unwrap_or(true) {
            self.frs.cur_tgt = None;
        }
        self.persist_frs();
        if self.graph_mode == GraphMode::Fr && self.frs.cur_meas.is_none() {
            self.graph_mode = GraphMode::Eq;
        }
    }

    pub fn set_graph_mode(&mut self, m: GraphMode) {
        if m == GraphMode::Fr && self.frs.cur_meas.is_none() {
            self.open_modal(Modal::Fr); // need a measurement first
            return;
        }
        self.graph_mode = m;
    }

    pub fn import_fr(&mut self, meas: bool, paths: Vec<std::path::PathBuf>) {
        let (mut ok, mut last) = (0, String::new());
        for p in paths {
            let Ok(text) = std::fs::read_to_string(&p) else { continue };
            let pts = fr::normalize(&fr::parse_fr(&text));
            if pts.len() >= 4 {
                let name = fr::name_from_file(&p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default());
                let rec = store::FrRec { rig: self.frs.rig.clone(), points: pts, treble: None, extra: Default::default() };
                if meas {
                    self.frs.meas.insert(name.clone(), rec);
                    self.frs.cur_meas = Some(name.clone());
                } else {
                    self.frs.targets.insert(name.clone(), rec);
                    self.frs.cur_tgt = Some(format!("t:{}", name));
                }
                ok += 1;
                last = name;
            }
        }
        let rig = self.frs.rig.clone();
        self.fr_msg = if ok == 1 {
            format!("Loaded \"{}\" ({}).", last, rig)
        } else if ok > 1 {
            format!("Imported {} {}(s) ({}).", ok, if meas { "measurement" } else { "target" }, rig)
        } else {
            "No valid freq/dB points found.".into()
        };
        self.persist_frs();
    }

    // ---- auto-fit ----

    pub fn auto_fit_clicked(&mut self) {
        if self.frs.cur_meas.is_none() || self.frs.tgt_rec().is_none() {
            self.open_modal(Modal::Fr);
            self.fr_msg = "Pick a measurement and a target, then Auto-fit.".into();
            return;
        }
        let (Some(meas), Some(tgt)) = (self.cur_meas_sm(), self.cur_tgt_sm()) else { return };
        let prof = self.active_profile();
        let inp = fit::FitInput { meas: &meas, tgt: &tgt, prof: prof.as_deref(), cfg: self.fit, caps: &self.caps, g_max: self.cur_g_max() };
        match fit::auto_fit(&inp) {
            fit::FitOutcome::NoProfile => {
                self.open_treble();
                self.tt.warn = "Auto-fit is set to \"whole target + treble correction\", which needs a treble profile for this measurement. Run the test, or switch the mode in Auto-fit settings.".into();
                self.tt.warn_ok = false;
            }
            fit::FitOutcome::Bands(b) => {
                self.push_undo();
                self.model = self.caps.fill(&b);
                self.preamp = self.caps.headroom_for(eq::peak(&self.model).0); // headroom
                self.edit.clear();
                if self.fit.mode == FitMode::Treble && !self.treble_view {
                    self.treble_view = true; // show it the way it was fitted
                    self.save_settings();
                }
                self.graph_mode = GraphMode::Fr;
            }
        }
    }

    pub fn apply_treble_profile(&mut self, prof: &[[f64; 2]]) -> (String, bool) {
        match fit::apply_treble(prof, &self.model, self.preamp, &self.caps, self.cur_g_max(), self.fit.top) {
            fit::TrebleApply::Unchanged { ok, msg } => (msg, ok),
            fit::TrebleApply::Changed { model, preamp, msg } => {
                self.push_undo();
                self.model = model;
                self.preamp = preamp;
                self.edit.clear();
                (msg, true)
            }
        }
    }

    // ---- connect / offline ----

    pub fn detect(&mut self) {
        self.worker.list();
    }

    pub fn open_cable(&mut self, d: Option<(u16, u16)>) {
        self.worker.open(d);
    }

    pub fn device_key(&self) -> Option<String> {
        self.cable.as_ref().map(|c| format!("{:x}:{:x}", c.vid, c.pid))
    }

    pub fn presets_map(&self) -> std::collections::BTreeMap<u8, String> {
        if let Some(c) = &self.cable {
            c.presets.clone()
        } else if let Some(o) = &self.offline {
            o.target.presets.clone()
        } else {
            Default::default()
        }
    }

    /// Devices that hold several writable banks (FiiO, Fosi) rather than one editable EQ.
    pub fn bank_device(&self) -> bool {
        self.cable.as_ref().map(|c| !c.user_slots.is_empty()).unwrap_or(false)
    }

    pub fn user_slots_now(&self) -> Vec<u8> {
        if let Some(c) = &self.cable {
            c.user_slots.clone()
        } else if let Some(o) = &self.offline {
            o.target.user_slots.clone()
        } else {
            vec![]
        }
    }

    pub fn user_label(&self, s: u8) -> String {
        self.presets_map().get(&s).cloned().unwrap_or_else(|| format!("bank {}", s))
    }

    fn render_cable(&mut self, info: hid::CableInfo) {
        self.offline = None;
        self.sel_bank = None;
        self.cur_target = target_for_cable(&info);
        if let Some(t) = &self.cur_target {
            self.targets.entry(t.id.clone()).or_insert_with(|| t.clone());
        }
        let c = Caps {
            gen: info.gen,
            bands: info.bands,
            g_min: info.gain_min,
            g_max: info.gain_max,
            q_min: info.q_min,
            q_max: info.q_max,
            types: if info.types.is_empty() { vec![FType::PK, FType::LSQ, FType::HSQ] } else { info.types.iter().map(|t| FType::from_code(t)).collect() },
            preamp_step: if info.preamp_step > 0.0 { info.preamp_step } else { 0.1 },
        };
        self.apply_caps(c);
        self.cable = Some(info);
    }

    pub fn enter_offline(&mut self) {
        let t = self.targets.get(&self.off_target).or_else(|| self.targets.get("generic")).cloned().unwrap();
        let nb = self.off_bands.clamp(1, t.bands);
        self.settings.offline_target = Some(t.id.clone());
        self.settings.offline_bands = Some(nb);
        self.save_settings();
        self.cable = None;
        self.cable_eq = None;
        self.apply_caps(Caps { gen: t.gen, bands: nb, g_min: t.g_min, g_max: t.g_max, q_min: t.q_min, q_max: t.q_max, types: t.types.clone(), preamp_step: 0.1 });
        let banks = !t.user_slots.is_empty();
        self.sel_bank = if banks { Some(t.user_slots[0]) } else { None };
        self.log(format!("offline · {} · {} bands", t.name, nb));
        self.cur_target = Some(t.clone());
        self.offline = Some(Offline { target: t, bands: nb });
        self.write_note = None;
        self.connected = true;
        self.device_list.clear();
        if let Some(s) = self.sel_bank {
            if self.cur_set().map(|x| x.get(s).is_some()).unwrap_or(false) {
                self.load_bank_into_editor(s);
            }
        }
        self.active_bank = self.sel_bank.map(|s| s as i32);
    }

    pub fn select_offline_bank(&mut self, s: u8) {
        self.sel_bank = Some(s);
        self.active_bank = Some(s as i32);
        if self.cur_set().map(|x| x.get(s).is_some()).unwrap_or(false) {
            self.load_bank_into_editor(s);
        }
    }

    pub fn disconnect(&mut self) {
        if self.cable.is_some() {
            self.worker.close();
        }
        self.worker.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        self.cable = None;
        self.cable_eq = None;
        self.offline = None;
        self.cur_target = None;
        self.sel_bank = None;
        self.active_bank = None;
        self.reset_caps();
        self.write_note = None;
        self.close_modal();
        self.connected = false;
        self.device_list.clear();
        self.connect_title = "Looking for a cable…".into();
    }

    pub fn read_from_cable(&mut self) {
        let gen = self.caps.gen;
        self.worker.read(gen >= 3);
    }

    pub fn switch_preset(&mut self, idx: i32) {
        self.log(format!("switch -> preset {}", idx));
        self.write_note = None;
        let gen = self.caps.gen;
        self.worker.switch(idx, gen >= 3);
    }

    pub fn is_cable_issue(m: &str) -> bool {
        let l = m.to_lowercase();
        ["wrong cable", "no moondrop cable", "no longer connected", "no cable at", "does not speak"].iter().any(|k| l.contains(k))
    }

    pub fn report_cable_issue(&mut self, m: &str) {
        let l = m.to_lowercase();
        let msg = if ["no moondrop cable", "no longer connected", "no cable at"].iter().any(|k| l.contains(k)) {
            "No cable detected, nothing was written. Reconnect to continue.".to_string()
        } else {
            format!("{} Reconnect to continue.", m.trim_end())
        };
        self.log(format!("BLOCKED: {}", msg));
        self.write_note = Some(msg);
    }

    // ---- writing ----

    pub fn write_label(&self) -> String {
        if let Some((l, until)) = &self.write_label {
            if Instant::now() < *until {
                return l.clone();
            }
        }
        if self.writing {
            return "Writing…".into();
        }
        if let Some(o) = &self.offline {
            return if !o.target.user_slots.is_empty() {
                match self.sel_bank {
                    Some(s) => format!("Store in {}", self.user_label(s)),
                    None => "Store in bank".into(),
                }
            } else {
                "Save EQ…".into()
            };
        }
        if self.bank_device() {
            let c = self.cable.as_ref().unwrap();
            return match self.active_bank {
                Some(a) if a >= 0 && c.user_slots.contains(&(a as u8)) => format!("Write to {}", c.presets.get(&(a as u8)).cloned().unwrap_or_default()),
                _ => "Pick a bank to write".into(),
            };
        }
        "Write to cable".into()
    }

    fn flash_write(&mut self, l: &str, ms: u64) {
        self.write_label = Some((l.into(), Instant::now() + Duration::from_millis(ms)));
    }

    /// Write entry point: clipping is checked first. In +6 dB mode a clipping EQ is blocked
    /// (reduce level); otherwise the user gets a choice (Auto headroom / write anyway).
    pub fn write_clicked(&mut self) {
        self.write_note = None;
        if let Some(o) = &self.offline {
            if o.target.user_slots.is_empty() {
                self.open_modal(Modal::Saved);
                return;
            }
            let Some(s) = self.sel_bank else { return };
            self.store_into_bank(s);
            self.flash_write("Stored ✔", 1200);
            return;
        }
        let (eff, pf) = self.eff_peak();
        if eff > 0.1 {
            self.clip_info = (eff, pf);
            self.open_modal(Modal::Clip);
            return;
        }
        self.do_write();
    }

    pub fn do_write(&mut self) {
        if self.cable.is_none() {
            return;
        }
        self.writing = true;
        self.worker.write(self.model.clone(), self.preamp);
    }

    // ---- bank sets ----

    pub fn persist_sets(&mut self) {
        if let Err(e) = self.sets.save() {
            self.log(format!("saving bank sets failed: {}", e));
        }
    }

    pub fn sets_for(&self, tid: &str) -> Vec<String> {
        self.sets.sets.iter().filter(|(_, v)| v.target == tid).map(|(k, _)| k.clone()).collect()
    }

    pub fn create_set(&mut self, tid: &str, name: &str, banks: IndexMap<String, store::Bank>, current: bool) -> String {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        let mut rng = treble::rand::Rng::new();
        let id = format!("s{}{:04x}", radix36(t as u64), rng.next_u64() & 0xffff);
        self.sets.sets.insert(id.clone(), store::BankSet { name: name.into(), target: tid.into(), banks });
        if current {
            self.sets.current.insert(tid.into(), id.clone());
        }
        self.persist_sets();
        id
    }

    /// The current set for the target in use, made if there is none.
    pub fn cur_set_id(&mut self) -> Option<String> {
        let t = self.cur_target.clone()?;
        if t.user_slots.is_empty() {
            return None;
        }
        let id = self.sets.current.get(&t.id).cloned();
        let ok = id.as_ref().map(|i| self.sets.sets.get(i).map(|s| s.target == t.id).unwrap_or(false)).unwrap_or(false);
        if ok {
            return id;
        }
        let id = match self.sets_for(&t.id).first() {
            Some(f) => f.clone(),
            None => self.create_set(&t.id, "My banks", IndexMap::new(), false),
        };
        self.sets.current.insert(t.id.clone(), id.clone());
        self.persist_sets();
        Some(id)
    }

    pub fn cur_set(&mut self) -> Option<&store::BankSet> {
        let id = self.cur_set_id()?;
        self.sets.sets.get(&id)
    }

    pub fn cur_set_mut(&mut self) -> Option<&mut store::BankSet> {
        let id = self.cur_set_id()?;
        self.sets.sets.get_mut(&id)
    }

    pub fn device_names(&mut self) -> IndexMap<String, String> {
        let Some(k) = self.device_key() else { return IndexMap::new() };
        self.sets.device_names.entry(k).or_default().clone()
    }

    pub fn bank_name(&mut self, s: u8) -> String {
        if self.offline.is_some() {
            return self.cur_set().and_then(|x| x.get(s)).map(|b| b.name.clone()).unwrap_or_default();
        }
        self.device_names().get(&s.to_string()).cloned().unwrap_or_default()
    }

    pub fn bank_msg(&mut self, t: impl Into<String>, warn: bool) {
        let t = t.into();
        if !t.is_empty() {
            self.log(t.clone());
        }
        self.bank_msg = (t, warn);
    }

    pub fn load_bank_into_editor(&mut self, s: u8) {
        let Some(bank) = self.cur_set().and_then(|x| x.get(s)).cloned() else { return };
        if self.offline.is_some() {
            self.sel_bank = Some(s);
        }
        let over = eq::active_count(&bank.bands) as i64 - self.caps.bands as i64;
        let label = format!("{}{}", self.user_label(s), if bank.name.is_empty() { String::new() } else { format!(" \"{}\"", bank.name) });
        self.apply_eq(&bank.bands, bank.preamp, &label);
        if over > 0 {
            self.write_note = Some(format!(
                "{} has {} more band(s) than the {} you're designing with; they were left off. Storing back would drop them.",
                self.user_label(s),
                over,
                self.caps.bands
            ));
        }
    }

    pub fn store_into_bank(&mut self, s: u8) {
        let Some(t) = self.cur_target.clone() else { return };
        let f = eq::fit_to_target(&self.model, self.preamp, &t);
        let label = self.user_label(s);
        let Some(set) = self.cur_set_mut() else { return };
        let name = set.get(s).map(|b| b.name.clone()).unwrap_or_default();
        set.put(s, store::Bank { name, bands: f.bands, preamp: f.preamp });
        self.persist_sets();
        let issues = if f.issues.is_empty() { String::new() } else { format!(" To fit the {}: {}.", t.name, f.issues.join("; ")) };
        self.bank_msg(format!("Stored the editor's EQ in {}.{}", label, issues), !f.issues.is_empty());
    }

    pub fn fill_from_saved(&mut self, s: u8, name: &str) {
        let (Some(e), Some(t)) = (self.saved.get(name).cloned(), self.cur_target.clone()) else { return };
        let f = eq::fit_to_target(&e.bands, e.preamp, &t);
        let label = self.user_label(s);
        if let Some(set) = self.cur_set_mut() {
            set.put(s, store::Bank { name: name.into(), bands: f.bands, preamp: f.preamp });
        }
        self.persist_sets();
        let issues = if f.issues.is_empty() { String::new() } else { format!(" To fit the {}: {}.", t.name, f.issues.join("; ")) };
        self.bank_msg(format!("Put saved EQ \"{}\" in {}.{}", name, label, issues), !f.issues.is_empty());
    }

    pub fn move_bank(&mut self, from: u8, to: u8) {
        let Some(t) = self.cur_target.clone() else { return };
        let slots = t.user_slots.clone();
        let Some(set) = self.cur_set_mut() else { return };
        let mut arr: Vec<Option<store::Bank>> = slots.iter().map(|x| set.get(*x).cloned()).collect();
        let (fi, ti) = (slots.iter().position(|x| *x == from).unwrap_or(0), slots.iter().position(|x| *x == to).unwrap_or(0));
        let m = arr.remove(fi);
        arr.insert(ti, m);
        set.banks.clear();
        for (i, x) in slots.iter().enumerate() {
            if let Some(b) = arr[i].take() {
                set.put(*x, b);
            }
        }
        self.persist_sets();
        let (a, b) = (self.user_label(from), self.user_label(to));
        self.bank_msg(format!("Moved {} to {}.", a, b), false);
    }

    pub fn sort_banks(&mut self) {
        let Some(t) = self.cur_target.clone() else { return };
        let slots = t.user_slots.clone();
        let Some(set) = self.cur_set_mut() else { return };
        let mut filled: Vec<store::Bank> = slots.iter().filter_map(|x| set.get(*x).cloned()).collect();
        filled.sort_by(|a, b| (a.name.is_empty() as u8).cmp(&(b.name.is_empty() as u8)).then_with(|| natural_cmp(&a.name, &b.name)));
        set.banks.clear();
        for (i, b) in filled.into_iter().enumerate() {
            set.put(slots[i], b);
        }
        self.persist_sets();
        self.bank_msg("Sorted by name; empty banks are last.", false);
    }

    /// Writes every filled bank of the current set (or just `only`), skipping banks the device
    /// already holds and keeping what it overwrote as a set of its own. A whole-set write goes
    /// back to the bank the device started on; sending one bank leaves the device on it, so you
    /// hear it.
    pub fn write_banks(&mut self, only: Option<u8>) {
        let (Some(t), true) = (self.cur_target.clone(), self.cable.is_some()) else { return };
        let Some(set) = self.cur_set().cloned() else { return };
        let slots: Vec<u8> = match only {
            Some(s) => vec![s],
            None => t.user_slots.clone(),
        }
        .into_iter()
        .filter(|s| set.get(*s).is_some())
        .collect();
        if slots.is_empty() {
            self.bank_msg("Nothing to write: every bank in this set is empty.", false);
            return;
        }
        let clipping: Vec<u8> = slots.iter().copied().filter(|s| clips(set.get(*s))).collect();
        if !clipping.is_empty() && !self.write_all_armed {
            self.write_all_armed = true;
            let names: Vec<String> = clipping.iter().map(|s| self.user_label(*s)).collect();
            self.bank_msg(format!("{} will clip. Click again to write anyway, or lower their preamp first.", names.join(", ")), true);
            return;
        }
        self.write_all_armed = false;
        self.bank_busy = true;
        let banks = slots.iter().map(|s| (*s, self.user_label(*s), set.get(*s).cloned().unwrap())).collect();
        let job = device::BankJob { banks, return_to: if only.is_none() { self.active_bank } else { None }, caps: self.caps.clone() };
        self.worker.write_banks(job);
    }

    pub fn read_all_banks(&mut self) {
        let (Some(t), true) = (self.cur_target.clone(), self.cable.is_some()) else { return };
        self.bank_busy = true;
        let slots = t.user_slots.iter().map(|s| (*s, self.user_label(*s))).collect();
        self.worker.read_banks(slots, self.active_bank);
    }

    pub fn delete_set(&mut self) {
        let Some(t) = self.cur_target.clone() else { return };
        let Some(id) = self.sets.current.get(&t.id).cloned() else { return };
        let armed = matches!(&self.del_armed, Some((i, until)) if *i == id && Instant::now() < *until);
        if !armed {
            self.del_armed = Some((id, Instant::now() + Duration::from_secs(3)));
            return;
        }
        self.del_armed = None;
        let name = self.sets.sets.shift_remove(&id).map(|s| s.name).unwrap_or_default();
        self.sets.current.shift_remove(&t.id);
        self.persist_sets();
        self.cur_set_id();
        self.bank_msg(format!("Deleted set \"{}\".", name), false);
    }

    // ---- dialogs ----

    pub fn open_modal(&mut self, m: Modal) {
        if m != Modal::Treble {
            self.tt_halt();
        }
        match m {
            Modal::EqText => {
                self.eq_text = self.eq_text_now();
                self.eq_text_msg.clear();
            }
            Modal::Backup => self.backup_msg = None,
            Modal::Fr => self.fr_msg.clear(),
            Modal::Banks => {
                self.write_all_armed = false;
                self.bank_msg.0.clear();
                self.set_rename = None;
            }
            _ => {}
        }
        self.modal = Some(m);
    }

    pub fn close_modal(&mut self) {
        self.tt_halt();
        self.tt.result = None;
        self.drop_queue.clear();
        self.drop_cur = None;
        self.modal = None;
        self.set_rename = None;
    }

    pub fn open_treble(&mut self) {
        self.open_modal(Modal::Treble);
        self.tt.view = TtView::Setup;
        self.tt.warn = self.tt_setup_warn();
        self.tt.warn_ok = false;
    }

    /// The test plays through whatever is on the cable, so it has to know what that is.
    pub fn tt_setup_warn(&self) -> String {
        match &self.cable_eq {
            None => "Not sure what EQ is on the cable. Write your EQ first so the test knows what it is playing through.".into(),
            Some(c) if !eq::same_eq(c, &self.model) => {
                "The EQ on screen is not what is on the cable. The test plays through the cable's EQ, so write first if you meant to test with this one.".into()
            }
            Some(c) if c.iter().any(|b| b.gain != 0.0 && b.freq > 5000.0 && b.typ == FType::PK && b.gain.abs() >= 1.0) => {
                "The cable EQ has bands above 5 kHz. That works, but the result is cleanest with the treble left flat.".into()
            }
            _ => String::new(),
        }
    }

    // ---- backup ----

    pub fn export_backup(&mut self) {
        let data = store::backup_json(&self.saved, &self.frs, &self.sets);
        let Some(path) = rfd::FileDialog::new().set_file_name("freedsp-backup.json").add_filter("JSON", &["json"]).save_file() else { return };
        match std::fs::write(&path, data) {
            Ok(_) => {
                self.log(format!("backup written: {}", path.display()));
                self.backup_msg = Some(("Backup saved".into(), Instant::now() + Duration::from_millis(2600)));
            }
            Err(e) => {
                self.log(format!("backup failed: {}", e));
                self.backup_msg = Some(("Backup failed".into(), Instant::now() + Duration::from_millis(2600)));
            }
        }
    }

    pub fn restore_backup(&mut self) {
        let Some(path) = rfd::FileDialog::new().add_filter("JSON", &["json"]).pick_file() else { return };
        let msg = match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| store::restore(&t, &mut self.saved, &mut self.frs, &mut self.sets)) {
            Ok(r) => {
                self.persist_saved();
                self.persist_frs();
                self.persist_sets();
                format!("Restored {} EQ(s), {} FR(s){}", r.eqs, r.frs, if r.sets > 0 { format!(", {} bank set(s)", r.sets) } else { String::new() })
            }
            Err(e) => {
                self.log(format!("restore failed: {}", e));
                "Restore failed, not a valid backup".into()
            }
        };
        self.backup_msg = Some((msg, Instant::now() + Duration::from_millis(2600)));
    }

    pub fn export_name(&mut self) -> String {
        let bank = if self.offline.is_some() {
            self.sel_bank.map(|s| self.bank_name(s)).unwrap_or_default()
        } else if self.cable.is_some() {
            self.active_bank.filter(|a| *a >= 0).map(|a| self.bank_name(a as u8)).unwrap_or_default()
        } else {
            String::new()
        };
        let base = if !bank.is_empty() { bank } else { self.frs.cur_meas.clone().unwrap_or_else(|| "EQ".into()) };
        let clean: String = base.chars().map(|c| if "\\/:*?\"<>|".contains(c) { ' ' } else { c }).collect();
        let clean = clean.trim();
        format!("{} ParametricEQ.txt", if clean.is_empty() { "EQ" } else { clean })
    }

    pub fn export_eq(&mut self) {
        let name = self.export_name();
        let Some(path) = rfd::FileDialog::new().set_file_name(&name).add_filter("Text", &["txt"]).save_file() else { return };
        match std::fs::write(&path, self.eq_text_now() + "\n") {
            Ok(_) => {
                self.log(format!("EQ exported: {}", path.display()));
                self.toast("EQ exported", false);
            }
            Err(e) => {
                self.log(format!("export failed: {}", e));
                self.toast("Export failed, see the log", true);
            }
        }
    }

    // ---- dropped files ----
    // A dropped text file is either a parametric EQ (squig / AutoEQ / APO "Filter …" lines),
    // which is applied to the current EQ straight away, or a frequency response (freq/dB pairs),
    // which needs a name, a rig and a role before it's added.

    pub fn handle_drop(&mut self, files: Vec<(String, String)>) {
        let mut eq_done = false;
        let mut frs: Vec<(String, Option<(String, char)>, fr::Curve)> = Vec::new();
        let mut bad = Vec::new();
        for (name, text) in files {
            let p = eq::parse_squig(&text);
            if !p.filters.is_empty() {
                if eq_done {
                    bad.push(format!("{} (only one EQ is applied at a time)", name));
                    continue;
                }
                if !self.connected {
                    bad.push(format!("{} (connect a device or start offline first)", name));
                    continue;
                }
                eq_done = true;
                let msg = self.apply_parsed(&p);
                self.toast(format!("{}: {}", name, msg), false);
                self.log(format!("dropped EQ {}", name));
                continue;
            }
            let pts = fr::parse_fr(&text);
            if pts.len() >= 4 {
                frs.push((name.clone(), fr::split_channel(&name), pts));
                continue;
            }
            bad.push(name);
        }
        if !bad.is_empty() {
            self.toast(format!("Not used: {}. Expected squig / AutoEQ \"Filter …\" lines or freq/dB pairs.", bad.join(", ")), true);
        }
        // Channels of the same measurement dropped together become one averaged curve.
        let mut groups: IndexMap<String, Vec<(String, Option<(String, char)>, fr::Curve)>> = IndexMap::new();
        for c in frs {
            let key = match &c.1 {
                Some((b, _)) => format!("ch:{}", b.to_lowercase()),
                None => format!("f:{}", c.0),
            };
            groups.entry(key).or_default().push(c);
        }
        for (_, g) in groups {
            let name = match &g[0].1 {
                Some((b, _)) => b.clone(),
                None => fr::name_from_file(&g[0].0),
            };
            if g.len() > 1 {
                let curves: Vec<fr::Curve> = g.iter().map(|c| c.2.clone()).collect();
                self.drop_queue.push_back(DropItem {
                    name,
                    file: g.iter().map(|c| c.0.clone()).collect::<Vec<_>>().join(" + "),
                    points: fr::normalize(&fr::average(&curves)),
                    channels: g.len(),
                    single: None,
                    rig: "711".into(),
                    meas: true,
                });
            } else {
                let c = &g[0];
                self.drop_queue.push_back(DropItem { name, file: c.0.clone(), points: fr::normalize(&c.2), channels: 1, single: c.1.as_ref().map(|x| x.1), rig: "711".into(), meas: true });
            }
        }
        if self.drop_cur.is_none() {
            self.next_drop();
        }
    }

    pub fn next_drop(&mut self) {
        self.drop_cur = self.drop_queue.pop_front();
        if self.drop_cur.is_none() {
            if self.modal == Some(Modal::Drop) {
                self.modal = None;
            }
            return;
        }
        self.modal = Some(Modal::Drop);
    }

    pub fn add_dropped(&mut self) {
        let Some(c) = self.drop_cur.take() else { return };
        let name = c.name.trim().to_string();
        if name.is_empty() {
            self.drop_cur = Some(c);
            return;
        }
        let rec = store::FrRec { rig: c.rig.clone(), points: c.points.clone(), treble: None, extra: Default::default() };
        if c.meas {
            self.frs.meas.insert(name.clone(), rec);
        } else {
            self.frs.targets.insert(name.clone(), rec);
        }
        // Show it: switch to its rig and select it, as an import from the FR window does.
        if self.frs.rig != c.rig {
            self.set_fr_rig(&c.rig);
        }
        if c.meas {
            self.frs.cur_meas = Some(name.clone());
            if self.frs.cur_tgt.as_deref() == Some(&format!("m:{}", name)) {
                self.frs.cur_tgt = None;
            }
        } else {
            self.frs.cur_tgt = Some(format!("t:{}", name));
        }
        self.persist_frs();
        let kind = if c.meas { "source" } else { "target" };
        self.log(format!("added {} \"{}\" ({})", kind, name, c.rig));
        self.toast(format!("Added {} \"{}\" for the {}.", kind, name, c.rig), false);
        self.next_drop();
    }

    // ---- device replies ----

    pub fn poll(&mut self) {
        while let Ok(r) = self.worker.rx.try_recv() {
            self.on_reply(r);
        }
    }

    fn on_reply(&mut self, r: Reply) {
        match r {
            Reply::Listed(Ok(found)) => {
                self.device_list.clear();
                if found.is_empty() {
                    self.connect_title = "No cable found, plug one in and click Detect".into();
                } else if found.len() == 1 {
                    let d = &found[0];
                    self.open_cable(Some((d.vid as u16, d.pid as u16)));
                } else {
                    self.connect_title = "More than one cable found, pick one".into();
                    self.device_list = found;
                }
            }
            Reply::Listed(Err(e)) => {
                self.log(format!("list failed: {}", e));
                self.connect_title = "Error looking for cables, see log".into();
            }
            Reply::Opened(Ok(info)) => {
                let (name, fam, gen, bands, gmin, gmax, tested) =
                    (info.name.clone(), info.family.clone(), info.gen, info.bands, info.gain_min, info.gain_max, info.tested);
                self.render_cable(info);
                self.write_note = None;
                self.connected = true;
                self.device_list.clear();
                self.log(format!("{} · {} · {} bands · gain {}..{} dB", name, if fam.is_empty() { format!("gen {}", gen) } else { fam }, bands, gmin, gmax));
                if !tested {
                    self.log(format!(
                        "{} hasn't been tested with this app yet: its settings come from the maker's own app or devicePEQ. Writes are still read back and checked.",
                        name
                    ));
                }
                self.worker.read_mode();
                self.read_from_cable();
            }
            Reply::Opened(Err(e)) => {
                self.log(format!("open failed: {}", e));
                self.connect_title = "Error opening cable, see log".into();
            }
            Reply::Mode(m) => {
                self.active_bank = m;
            }
            Reply::Read(Ok((bands, pre))) => {
                let n = bands.iter().filter(|b| b.is_some()).count();
                let types = self.caps.types.clone();
                let mdl: Vec<Band> = bands
                    .iter()
                    .enumerate()
                    .map(|(i, b)| match b {
                        Some(b) => Band { gain: self.caps.round_gain(b.gain), typ: if types.contains(&b.typ) { b.typ } else { FType::PK }, ..*b },
                        None => Band::blank(self.caps.default_freq(i)),
                    })
                    .collect();
                self.push_undo();
                let mx = mdl.iter().map(|b| b.gain).fold(0.0, f64::max);
                self.ensure_ceil_for(mx);
                self.model = self.caps.fill(&mdl);
                if self.caps.gen >= 3 {
                    if let Some(p) = pre {
                        self.preamp = self.caps.pre_round(p);
                    }
                }
                self.cable_eq = Some(self.model.clone());
                self.edit.clear();
                self.log(format!("read {} bands", n));
            }
            Reply::Read(Err(m)) => {
                self.log(format!("read failed: {}", m));
                self.write_note = Some(if self.caps.gen >= 3 { m } else { "Could not read the EQ from the cable, see the log.".into() });
            }
            Reply::Switched(Ok(())) => {}
            Reply::Switched(Err(m)) => {
                self.log(format!("setPreset failed: {}", m));
                if Self::is_cable_issue(&m) {
                    self.report_cable_issue(&m);
                }
            }
            Reply::Wrote(res, sent) => {
                self.writing = false;
                match res {
                    Ok(r) => {
                        let nb = self.caps.bands;
                        if self.caps.gen >= 2 {
                            // Gen-2 verifies by reading the bank back, so `acked` counts bands
                            // the cable is actually holding. Gen-1 can only count frames it
                            // believes were acknowledged.
                            self.log(format!("wrote {}/{} bands, verified by read-back; slot={:?}", r.acked, nb, r.active));
                            if (r.acked as usize) < nb {
                                self.write_note = Some(format!("Only {} of {} bands read back as written. The EQ on the cable may not match what you see.", r.acked, nb));
                                self.flash_write("Partly written", 1800);
                                self.worker.read_mode();
                                return;
                            }
                        } else {
                            self.log(format!("wrote {}/54 frames; active={:?}", r.acked, r.active));
                        }
                        self.cable_eq = Some(sent);
                        self.worker.read_mode();
                        self.flash_write("Written ✔", 1400);
                        if self.modal == Some(Modal::Treble) {
                            self.tt.warn = self.tt_setup_warn();
                        }
                    }
                    Err(m) => {
                        self.log(format!("writeBank failed: {}", m));
                        if Self::is_cable_issue(&m) {
                            self.report_cable_issue(&m);
                            self.flash_write("Blocked", 1600);
                        } else {
                            if self.caps.gen >= 3 {
                                self.write_note = Some(m);
                            }
                            self.flash_write("Write failed", 1600);
                        }
                    }
                }
            }
            Reply::BankMsg(t, w) => self.bank_msg(t, w),
            Reply::BankWritten(o) => self.on_banks_written(o),
            Reply::BankRead(o) => self.on_banks_read(o),
        }
    }

    fn on_banks_written(&mut self, o: device::BankWriteOutcome) {
        self.bank_busy = false;
        let Some(t) = self.cur_target.clone() else { return };
        let mut names = self.device_names();
        let mut before = IndexMap::new();
        for (s, b) in o.before {
            let prev = names.get(&s.to_string()).cloned().unwrap_or_default();
            before.insert(s.to_string(), store::Bank { name: if prev.is_empty() { String::new() } else { format!("{} (before)", prev) }, ..b });
        }
        for (s, n) in &o.names {
            names.insert(s.to_string(), n.clone());
        }
        if let Some(k) = self.device_key() {
            self.sets.device_names.insert(k, names);
        }
        let mut backup = String::new();
        if !before.is_empty() {
            backup = format!("Before write · {}", stamp());
            self.create_set(&t.id, &backup, before, false);
        }
        self.persist_sets();
        let done = if o.only_one {
            let s = o.names.first().map(|x| x.0).unwrap_or(0);
            format!("{} {}", self.user_label(s), if o.wrote > 0 { "written" } else { "already matched" })
        } else {
            format!("Wrote {}, {} already matched", o.wrote, o.same)
        };
        let tail = format!(
            "{}{}",
            if backup.is_empty() { String::new() } else { format!(" What was overwritten is saved as set \"{}\".", backup) },
            if o.only_one { "" } else { " Empty banks were left as they are." }
        );
        match o.err {
            Some(e) => {
                self.bank_msg(format!("Stopped: {} {} before that.{}", e, done, tail), true);
                if Self::is_cable_issue(&e) {
                    self.report_cable_issue(&e);
                }
            }
            None => self.bank_msg(format!("{}{}.{}", if o.stopped { "Stopped. " } else { "" }, done, tail), false),
        }
        // The editor keeps whatever you were working on; it just no longer matches the device.
        self.cable_eq = None;
    }

    fn on_banks_read(&mut self, o: device::BankReadOutcome) {
        self.bank_busy = false;
        let (Some(t), Some(c)) = (self.cur_target.clone(), self.cable.clone()) else { return };
        if o.err.is_none() && !o.stopped {
            let names = self.device_names();
            let mut banks = IndexMap::new();
            for (s, b) in o.banks {
                banks.insert(s.to_string(), store::Bank { name: names.get(&s.to_string()).cloned().unwrap_or_default(), ..b });
            }
            let n = banks.len();
            let name = format!("{} · read {}", c.name, stamp());
            self.create_set(&t.id, &name, banks, true);
            self.bank_msg(format!("Read {} banks into new set \"{}\" ({} hold an EQ).", t.user_slots.len(), name, n), false);
        } else {
            match o.err {
                Some(e) => self.bank_msg(format!("Stopped: {}", e), true),
                None => self.bank_msg("Stopped; nothing was saved.", false),
            }
        }
    }

    // ---- the treble test's sound and timing ----

    fn audio(&mut self) -> Option<&crate::audio::Audio> {
        if self.tt.audio.is_none() && self.tt.audio_err.is_none() {
            match crate::audio::Audio::open() {
                Ok(a) => self.tt.audio = Some(a),
                Err(e) => {
                    self.log(format!("audio: {}", e));
                    self.tt.audio_err = Some(e);
                }
            }
        }
        self.tt.audio.as_ref()
    }

    pub fn tt_silence(&mut self) {
        self.tt.timers.clear();
        if let Some(a) = &self.tt.audio {
            a.silence();
        }
    }

    /// Closing the window or the dialog: stop sound, drop any test in progress.
    pub fn tt_halt(&mut self) {
        self.tt_silence();
        self.tt.session = None;
        self.tt.level_on = false;
    }

    /// Level check: the full noise, no band, until clicked again.
    pub fn tt_level_toggle(&mut self) {
        let on = !self.tt.level_on;
        let Some(a) = self.audio() else {
            self.tt.warn = "No audio output found, so the test can't play.".into();
            return;
        };
        let t = a.now();
        a.peaking(8000.0, 3.5, 0.0, t);
        a.ramp(crate::audio::Gain::Cue, 0.0, t);
        a.ramp(crate::audio::Gain::Full, if on { 1.0 } else { 0.0 }, t);
        self.tt.level_on = on;
    }

    pub fn tt_start(&mut self) {
        let tip = self.tt.tip.trim().to_string();
        if tip.is_empty() {
            self.tt.warn = "Name the tip you are using, so the profile can be kept per tip.".into();
            self.tt.warn_ok = false;
            return;
        }
        if self.tt.level_on {
            self.tt_level_toggle();
        }
        if self.audio().is_none() {
            self.tt.warn = format!("No audio output: {}", self.tt.audio_err.clone().unwrap_or_default());
            return;
        }
        let eq_during = self.cable_eq.clone().unwrap_or_else(|| self.model.clone());
        self.tt.session = Some(treble::Session::new(tip, self.tt.method, self.tt.thorough, eq_during));
        self.tt.view = TtView::Run;
        self.tt_next();
    }

    pub fn tt_next(&mut self) {
        let Some(s) = self.tt.session.as_mut() else { return };
        match s.next() {
            treble::Next::Play(_) => self.tt_play(None),
            treble::Next::Reseat => {
                self.tt_silence();
                self.tt.freq = "—".into();
                self.tt.phase.clear();
                self.tt.phase_ask = false;
                self.tt.count.clear();
            }
            treble::Next::Finished => self.tt_finish(),
        }
    }

    /// only: None = the whole trial, Some(true) / Some(false) = the first / second burst again.
    pub fn tt_play(&mut self, only: Option<bool>) {
        const CUE: f64 = 1.2;
        const GAP: f64 = 0.6;
        const FULL: f64 = 5.0;
        const CUE_LVL: f32 = 2.5;
        const BURST: f64 = 0.55;
        const BURST_GAP: f64 = 0.3;
        const MQ: f64 = 4.32;
        const TQ: f64 = 3.5;
        self.tt.timers.clear();
        let Some(item) = self.tt.session.as_ref().and_then(|s| s.current().cloned()) else { return };
        let passes = self.tt.session.as_ref().map(|s| s.passes.len()).unwrap_or(1);
        let pass = self.tt.session.as_ref().map(|s| s.pass).unwrap_or(0);
        if let Some(s) = self.tt.session.as_mut() {
            s.asking = false;
        }
        self.tt.count = if passes > 1 { format!("Pass {} of 2", pass + 1) } else { "Quick test".into() };
        self.tt.phase_ask = false;
        let Some(a) = self.audio() else { return };
        let t = a.now() + 0.05;
        let now = Instant::now();
        let at = |s: f64| now + Duration::from_secs_f64(s + 0.05);
        let mut timers = Vec::new();
        let phase;
        let ask_text;
        let freq;
        match &item {
            treble::Item::Pair(p) => {
                let kh = |f: f64| fixed(f / 1000.0, 1);
                freq = format!("{} → {} kHz", kh(fr::TT_BANDS[p.reference]), kh(fr::TT_BANDS[p.vari]));
                let t2 = t + BURST + BURST_GAP;
                a.ramp(crate::audio::Gain::Full, 0.0, t);
                a.ramp(crate::audio::Gain::Cue, 0.0, t - 0.04);
                let burst = |first: bool, t: f64| {
                    let f = fr::TT_BANDS[if first { p.reference } else { p.vari }];
                    let lvl = CUE_LVL * if first { 1.0 } else { 10f64.powf(p.d / 20.0) as f32 };
                    a.bandpass(f, MQ, t - 0.04);
                    a.ramp(crate::audio::Gain::Cue, lvl, t);
                    a.ramp(crate::audio::Gain::Cue, 0.0, t + BURST);
                };
                let mut ph = String::new();
                if only != Some(false) {
                    burst(true, t);
                    ph = "First…".into();
                }
                if only != Some(true) {
                    let at2 = if only == Some(false) { t } else { t2 };
                    burst(false, at2);
                    if only == Some(false) {
                        ph = "…second".into();
                    } else {
                        timers.push((at(BURST + BURST_GAP), TtEv::PhaseSecond));
                    }
                }
                phase = ph;
                timers.push((at(if only.is_some() { BURST } else { 2.0 * BURST + BURST_GAP }), TtEv::Ask));
                ask_text = "Was the second quieter, the same, or louder?".to_string();
            }
            treble::Item::Stair(s) => {
                freq = format!("{} kHz", fixed(s.f / 1000.0, 1));
                phase = "Listen: the band on its own".into();
                a.bandpass(s.f, TQ, t);
                a.peaking(s.f, TQ, s.g, t);
                a.ramp(crate::audio::Gain::Full, 0.0, t);
                a.ramp(crate::audio::Gain::Cue, CUE_LVL, t);
                a.ramp(crate::audio::Gain::Cue, 0.0, t + CUE);
                a.ramp(crate::audio::Gain::Full, 1.0, t + CUE + GAP);
                a.ramp(crate::audio::Gain::Full, 0.0, t + CUE + GAP + FULL);
                timers.push((at(CUE), TtEv::PhaseClear));
                timers.push((at(CUE + GAP), TtEv::Ask));
                ask_text = "Does that band stick out of the noise?".to_string();
            }
        }
        self.tt.freq = freq;
        self.tt.phase = phase;
        self.tt.ask_text = ask_text;
        self.tt.timers = timers;
    }

    /// Fires due timers; returns how long until the next one.
    pub fn tt_tick(&mut self) -> Option<Duration> {
        let now = Instant::now();
        let mut due: Vec<TtEv> = Vec::new();
        self.tt.timers.retain(|(t, e)| {
            if *t <= now {
                due.push(*e);
                false
            } else {
                true
            }
        });
        for e in due {
            match e {
                TtEv::Ask => {
                    if let Some(s) = self.tt.session.as_mut() {
                        s.asking = true;
                    }
                    self.tt.phase = self.tt.ask_text.clone();
                    self.tt.phase_ask = true;
                }
                TtEv::PhaseClear => self.tt.phase.clear(),
                TtEv::PhaseSecond => self.tt.phase = "…second".into(),
                TtEv::Next => self.tt_next(),
            }
        }
        self.tt.timers.iter().map(|(t, _)| t.saturating_duration_since(now)).min()
    }

    pub fn tt_answer_stair(&mut self, heard: bool) {
        let Some(s) = self.tt.session.as_mut() else { return };
        if !s.asking || s.paused || s.method != treble::Method::Noise {
            return;
        }
        s.asking = false;
        s.answer_stair(heard);
        self.tt_after_answer(0.7);
    }

    pub fn tt_answer_pair(&mut self, a: treble::Loudness) {
        let Some(s) = self.tt.session.as_mut() else { return };
        if !s.asking || s.paused || s.method != treble::Method::Match {
            return;
        }
        s.asking = false;
        s.answer_pair(a);
        self.tt_after_answer(0.45);
    }

    fn tt_after_answer(&mut self, gap: f64) {
        self.tt_silence();
        self.tt.phase.clear();
        self.tt.phase_ask = false;
        // a moment of silence between trials
        self.tt.timers.push((Instant::now() + Duration::from_secs_f64(gap), TtEv::Next));
    }

    pub fn tt_replay(&mut self, only: Option<bool>) {
        if let Some(s) = &self.tt.session {
            if !s.paused && !s.wait_reseat {
                self.tt_play(only);
            }
        }
    }

    /// Takes back the last answer and plays that trial again, even across the re-seat break.
    pub fn tt_undo(&mut self) {
        let Some(s) = self.tt.session.as_mut() else { return };
        if s.paused || !s.undo() {
            return;
        }
        self.tt_play(None);
    }

    pub fn tt_pause(&mut self) {
        let Some(s) = self.tt.session.as_mut() else { return };
        if s.wait_reseat {
            return;
        }
        s.paused = !s.paused;
        if s.paused {
            s.asking = false;
            self.tt_silence();
            self.tt.phase = "Paused".into();
            self.tt.phase_ask = false;
        } else {
            self.tt_play(None);
        }
    }

    pub fn tt_continue(&mut self) {
        if let Some(s) = self.tt.session.as_mut() {
            s.wait_reseat = false;
        }
        self.tt_next();
    }

    pub fn tt_stop(&mut self) {
        self.tt_halt();
        self.tt.view = TtView::Setup;
        self.tt.warn = self.tt_setup_warn();
    }

    fn tt_finish(&mut self) {
        self.tt_silence();
        let Some(s) = self.tt.session.take() else { return };
        let r = s.finish(today());
        self.tt.done_warn = r.warn.clone();
        self.tt.done_ok = false;
        self.tt.result = Some(r);
        self.tt.view = TtView::Done;
    }

    pub fn tt_save(&mut self) {
        let Some(r) = self.tt.result.take() else { return };
        let Some(m) = self.frs.cur_meas.clone() else { return };
        let Some(rec) = self.frs.meas.get_mut(&m) else { return };
        let tr = rec.treble.get_or_insert_with(Default::default);
        let mut extra = serde_json::Map::new();
        extra.insert("method".into(), r.method.code().into());
        if let Some(t) = &r.thresholds {
            extra.insert("thresholds".into(), serde_json::to_value(t).unwrap_or_default());
        }
        if let Some(p) = &r.pairs {
            extra.insert("pairs".into(), serde_json::to_value(p).unwrap_or_default());
        }
        extra.insert("eqDuring".into(), serde_json::to_value(&r.eq_during).unwrap_or_default());
        tr.profiles.insert(r.name.clone(), store::Profile { bands: r.bands.clone(), date: r.date.clone(), passes: r.passes as u32, extra });
        tr.sel = Some(r.name.clone());
        self.treble_view = true;
        self.save_settings();
        self.persist_frs();
        self.tt.view = TtView::Setup;
        self.tt.warn.clear();
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll();
        if let Some(d) = self.tt_tick() {
            ctx.request_repaint_after(d);
        }
        // timed notes
        let now = Instant::now();
        for until in [self.toast.as_ref().map(|t| t.2), self.write_label.as_ref().map(|t| t.1), self.backup_msg.as_ref().map(|t| t.1), self.treble_msg.as_ref().map(|t| t.2)]
            .into_iter()
            .flatten()
        {
            if until > now {
                ctx.request_repaint_after(until - now);
            }
        }
        if self.worker.busy() || self.writing {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        self.files(&ctx);
        crate::ui::draw(self, ui);
        crate::dialogs::draw(self, &ctx);
        if ctx.input(|i| i.viewport().close_requested()) {
            self.tt_halt();
        }
    }
}

impl App {
    /// Files dragged over or dropped on the window.
    fn files(&mut self, ctx: &egui::Context) {
        self.hovering_files = ctx.input(|i| !i.raw.hovered_files.is_empty());
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if dropped.is_empty() {
            return;
        }
        let mut files = Vec::new();
        for d in dropped {
            let name = d.path().file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let text = d.bytes().map(|b| String::from_utf8_lossy(&b).to_string()).unwrap_or_default();
            files.push((name, text));
        }
        self.handle_drop(files);
    }
}

fn target_from_bank(x: &hid::BankTarget) -> Target {
    Target {
        id: eq::target_id(&x.name),
        name: x.name.clone(),
        gen: if x.family == "Fosi Audio" { 5 } else { 3 },
        bands: x.bands,
        g_min: x.gain_min,
        g_max: x.gain_max,
        q_min: x.q_min,
        q_max: x.q_max,
        types: x.types.iter().map(|t| FType::from_code(t)).collect(),
        presets: x.presets.clone(),
        user_slots: x.user_slots.clone(),
        tested: x.tested,
    }
}

fn target_for_cable(info: &hid::CableInfo) -> Option<Target> {
    if info.user_slots.is_empty() {
        return None;
    }
    Some(Target {
        id: eq::target_id(&info.name),
        name: info.name.clone(),
        gen: if info.gen > 0 { info.gen } else { 3 },
        bands: info.bands,
        g_min: info.gain_min,
        g_max: info.gain_max,
        q_min: info.q_min,
        q_max: info.q_max,
        types: info.types.iter().map(|t| FType::from_code(t)).collect(),
        presets: info.presets.clone(),
        user_slots: info.user_slots.clone(),
        tested: info.tested,
    })
}

pub fn clips(b: Option<&store::Bank>) -> bool {
    b.map(|b| eq::peak_of(&b.bands) + b.preamp > 0.1).unwrap_or(false)
}

pub fn bank_summary(b: Option<&store::Bank>) -> String {
    match b {
        None => "empty".into(),
        Some(b) => {
            let n = eq::active_count(&b.bands);
            format!("{} band{} · {} dB", n, if n == 1 { "" } else { "s" }, fixed(b.preamp, 1))
        }
    }
}

pub fn bank_short(name: &str) -> String {
    name.strip_prefix("USER").or_else(|| name.strip_prefix("Custom ")).unwrap_or(name).to_string()
}

pub fn is_off_slot(name: &str) -> bool {
    let l = name.to_lowercase();
    l == "bypass" || l == "close eq"
}

fn radix36(mut n: u64) -> String {
    const D: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut s = Vec::new();
    loop {
        s.push(D[(n % 36) as usize]);
        n /= 36;
        if n == 0 {
            break;
        }
    }
    s.reverse();
    String::from_utf8(s).unwrap()
}

/// Names that hold numbers sort by value: "EQ 2" before "EQ 10". Case is ignored.
fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let (a, b) = (a.to_lowercase(), b.to_lowercase());
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, _) => return std::cmp::Ordering::Less,
            (_, None) => return std::cmp::Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let mut n1 = String::new();
                while let Some(c) = x.peek().copied().filter(|c| c.is_ascii_digit()) {
                    n1.push(c);
                    x.next();
                }
                let mut n2 = String::new();
                while let Some(c) = y.peek().copied().filter(|c| c.is_ascii_digit()) {
                    n2.push(c);
                    y.next();
                }
                let o = n1.trim_start_matches('0').len().cmp(&n2.trim_start_matches('0').len()).then(n1.trim_start_matches('0').cmp(n2.trim_start_matches('0')));
                if o != std::cmp::Ordering::Equal {
                    return o;
                }
            }
            (Some(c), Some(d)) => {
                if c != d {
                    return c.cmp(&d);
                }
                x.next();
                y.next();
            }
        }
    }
}

/// Local date and time, "2026-10-14 09:05".
pub fn stamp() -> String {
    let (y, mo, d, h, mi) = local_now();
    format!("{:04}-{:02}-{:02} {:02}:{:02}", y, mo, d, h, mi)
}

/// "2026-10-14" (the date the HTML version stored was UTC; so is this).
pub fn today() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (y, m, d) = civil(secs / 86400);
    format!("{:04}-{:02}-{:02}", y, m, d)
}

/// Days since 1970-01-01 to a calendar date (Howard Hinnant's algorithm).
fn civil(days: u64) -> (i64, u32, u32) {
    let z = days as i64 + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// The local time, without a time-zone library: the offset comes from the C library on Unix and
/// from the system on Windows; if neither is available, UTC.
fn local_now() -> (i64, u32, u32, u32, u32) {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let off = utc_offset_secs(secs);
    let t = (secs + off).max(0) as u64;
    let (y, m, d) = civil(t / 86400);
    let s = t % 86400;
    (y, m, d, (s / 3600) as u32, ((s % 3600) / 60) as u32)
}

#[cfg(unix)]
fn utc_offset_secs(t: i64) -> i64 {
    #[repr(C)]
    struct Tm {
        tm_sec: i32,
        tm_min: i32,
        tm_hour: i32,
        tm_mday: i32,
        tm_mon: i32,
        tm_year: i32,
        tm_wday: i32,
        tm_yday: i32,
        tm_isdst: i32,
        tm_gmtoff: std::ffi::c_long,
        tm_zone: *const std::ffi::c_char,
    }
    extern "C" {
        fn localtime_r(t: *const i64, out: *mut Tm) -> *mut Tm;
    }
    let mut tm: Tm = unsafe { std::mem::zeroed() };
    let r = unsafe { localtime_r(&t, &mut tm) };
    if r.is_null() {
        0
    } else {
        tm.tm_gmtoff as i64
    }
}

#[cfg(windows)]
fn utc_offset_secs(_t: i64) -> i64 {
    #[repr(C)]
    struct SystemTime {
        w: [u16; 8],
    }
    #[repr(C)]
    struct Tzi {
        bias: i32,
        sn: [u16; 32],
        sd: SystemTime,
        sb: i32,
        dn: [u16; 32],
        dd: SystemTime,
        db: i32,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetTimeZoneInformation(out: *mut Tzi) -> u32;
    }
    let mut z: Tzi = unsafe { std::mem::zeroed() };
    let r = unsafe { GetTimeZoneInformation(&mut z) };
    let bias = z.bias + if r == 2 { z.db } else { z.sb };
    -(bias as i64) * 60
}

#[cfg(not(any(unix, windows)))]
fn utc_offset_secs(_t: i64) -> i64 {
    0
}
