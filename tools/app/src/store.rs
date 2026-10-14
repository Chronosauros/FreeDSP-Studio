// What the app keeps between runs. The files and their JSON are the same ones the earlier
// (Tauri) builds wrote, in the same folder, so saved EQs, curves and bank sets carry over:
//
//   saved-eqs.json   name -> { bands, preamp }  (or a bare band list, from early versions)
//   saved-frs.json   { meas, targets, curMeas, curTgt, rig }
//   bank-sets.json   { sets, current, deviceNames }
//   settings.json    theme, auto-fit settings and other small preferences (new: these used to
//                    live in the webview's local storage)
//
// Fields this version doesn't know about are kept, so a file written by a newer version
// survives a round trip through an older one.

use std::path::PathBuf;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::eq::Band;
use crate::fit::{FitCfg, FitMode};

/// %APPDATA%\com.freedsp.studio, ~/Library/Application Support/com.freedsp.studio,
/// ~/.config/com.freedsp.studio: where Tauri put them.
pub fn config_dir() -> Option<PathBuf> {
    let d = dirs::config_dir()?.join("com.freedsp.studio");
    std::fs::create_dir_all(&d).ok()?;
    Some(d)
}

fn read(name: &str) -> Option<String> {
    std::fs::read_to_string(config_dir()?.join(name)).ok()
}

/// Writes through a temporary file, so a crash mid-write can't leave half a file.
fn write(name: &str, data: &str) -> Result<(), String> {
    let dir = config_dir().ok_or("no config folder")?;
    let tmp = dir.join(format!(".{}.tmp", name));
    std::fs::write(&tmp, data).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, dir.join(name)).map_err(|e| e.to_string())
}

fn load<T: for<'de> Deserialize<'de> + Default>(name: &str) -> T {
    read(name).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn save<T: Serialize>(name: &str, v: &T) -> Result<(), String> {
    write(name, &serde_json::to_string(v).map_err(|e| e.to_string())?)
}

// ---- saved EQs ----

#[derive(Clone, Debug, Default)]
pub struct SavedEq {
    pub bands: Vec<Band>,
    pub preamp: f64,
}

impl SavedEq {
    fn from_value(v: &Value) -> Option<SavedEq> {
        match v {
            Value::Array(_) => Some(SavedEq { bands: serde_json::from_value(v.clone()).ok()?, preamp: 0.0 }),
            Value::Object(o) => Some(SavedEq {
                bands: serde_json::from_value(o.get("bands")?.clone()).ok()?,
                preamp: o.get("preamp").and_then(Value::as_f64).unwrap_or(0.0),
            }),
            _ => None,
        }
    }
    fn to_value(&self) -> Value {
        serde_json::json!({ "bands": self.bands, "preamp": self.preamp })
    }
}

pub type SavedEqs = IndexMap<String, SavedEq>;

pub fn saved_from_value(v: &Value) -> SavedEqs {
    let mut out = SavedEqs::new();
    if let Value::Object(o) = v {
        for (k, e) in o {
            if let Some(s) = SavedEq::from_value(e) {
                out.insert(k.clone(), s);
            }
        }
    }
    out
}

pub fn saved_to_value(s: &SavedEqs) -> Value {
    Value::Object(s.iter().map(|(k, v)| (k.clone(), v.to_value())).collect())
}

pub fn load_saved() -> SavedEqs {
    saved_from_value(&read("saved-eqs.json").and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null))
}

pub fn save_saved(s: &SavedEqs) -> Result<(), String> {
    write("saved-eqs.json", &saved_to_value(s).to_string())
}

// ---- frequency responses ----

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Profile {
    /// [frequency, dB the EQ should add] at each tested band.
    pub bands: Vec<[f64; 2]>,
    #[serde(default)]
    pub date: String,
    #[serde(default)]
    pub passes: u32,
    /// method, thresholds, pairs, eqDuring: kept as they were written.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Treble {
    #[serde(default)]
    pub profiles: IndexMap<String, Profile>,
    #[serde(default)]
    pub sel: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct FrRec {
    pub rig: String,
    pub points: Vec<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub treble: Option<Treble>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn rig_5128() -> String {
    "5128".into()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FrStore {
    #[serde(default)]
    pub meas: IndexMap<String, FrRec>,
    #[serde(default)]
    pub targets: IndexMap<String, FrRec>,
    /// The selected measurement's name.
    #[serde(rename = "curMeas", default)]
    pub cur_meas: Option<String>,
    /// The selected target: "t:name" for a target, "m:name" for a measurement used as one.
    #[serde(rename = "curTgt", default)]
    pub cur_tgt: Option<String>,
    #[serde(default = "rig_5128")]
    pub rig: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for FrStore {
    fn default() -> Self {
        FrStore { meas: IndexMap::new(), targets: IndexMap::new(), cur_meas: None, cur_tgt: None, rig: rig_5128(), extra: Map::new() }
    }
}

impl FrStore {
    pub fn load() -> FrStore {
        let mut s: FrStore = load("saved-frs.json");
        // migrate old plain-name targets
        if let Some(t) = s.cur_tgt.clone() {
            if !t.starts_with("t:") && !t.starts_with("m:") {
                s.cur_tgt = if s.targets.contains_key(&t) { Some(format!("t:{}", t)) } else { None };
            }
        }
        let rig = s.rig.clone();
        if let Some(m) = &s.cur_meas {
            if s.meas.get(m).map(|r| r.rig != rig).unwrap_or(true) {
                s.cur_meas = None;
            }
        }
        if s.tgt_rec().map(|r| r.rig != rig).unwrap_or(true) {
            s.cur_tgt = None;
        }
        s
    }
    pub fn save(&self) -> Result<(), String> {
        save("saved-frs.json", self)
    }
    pub fn tgt_rec(&self) -> Option<&FrRec> {
        let t = self.cur_tgt.as_deref()?;
        if let Some(n) = t.strip_prefix("t:") {
            self.targets.get(n)
        } else if let Some(n) = t.strip_prefix("m:") {
            self.meas.get(n)
        } else {
            self.targets.get(t)
        }
    }
    pub fn meas_rec(&self) -> Option<&FrRec> {
        self.meas.get(self.cur_meas.as_deref()?)
    }
    /// The selected measurement's active treble profile.
    pub fn active_profile(&self) -> Option<(&str, &Profile)> {
        let t = self.meas_rec()?.treble.as_ref()?;
        let sel = t.sel.as_deref()?;
        Some((sel, t.profiles.get(sel)?))
    }
}

// ---- bank sets ----

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Bank {
    #[serde(default)]
    pub name: String,
    pub bands: Vec<Band>,
    #[serde(default)]
    pub preamp: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct BankSet {
    pub name: String,
    pub target: String,
    /// Keyed by the bank's slot number, as a string (it is a JSON object).
    #[serde(default)]
    pub banks: IndexMap<String, Bank>,
}

impl BankSet {
    pub fn get(&self, slot: u8) -> Option<&Bank> {
        self.banks.get(&slot.to_string())
    }
    pub fn get_mut(&mut self, slot: u8) -> Option<&mut Bank> {
        self.banks.get_mut(&slot.to_string())
    }
    pub fn put(&mut self, slot: u8, b: Bank) {
        self.banks.insert(slot.to_string(), b);
    }
    pub fn remove(&mut self, slot: u8) {
        self.banks.shift_remove(&slot.to_string());
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct SetsDb {
    #[serde(default)]
    pub sets: IndexMap<String, BankSet>,
    /// target id -> set id
    #[serde(default)]
    pub current: IndexMap<String, String>,
    /// "vid:pid" (lower-case hex) -> slot -> the name of what the app last wrote there. The
    /// BTR17 can't store bank names itself.
    #[serde(rename = "deviceNames", default)]
    pub device_names: IndexMap<String, IndexMap<String, String>>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl SetsDb {
    pub fn load() -> SetsDb {
        load("bank-sets.json")
    }
    pub fn save(&self) -> Result<(), String> {
        save("bank-sets.json", self)
    }
}

// ---- settings (formerly the webview's local storage) ----

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub theme: Theme,
    pub fit_low: f64,
    pub fit_high: f64,
    pub fit_bands: usize,
    pub fit_mode: String,
    pub fit_top: f64,
    pub treble_view: bool,
    pub tt_method: String,
    pub offline_target: Option<String>,
    pub offline_bands: Option<usize>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: Theme::System,
            fit_low: 20.0,
            fit_high: 20000.0,
            fit_bands: 9,
            fit_mode: "range".into(),
            fit_top: 16000.0,
            treble_view: true,
            tt_method: "noise".into(),
            offline_target: None,
            offline_bands: None,
            extra: Map::new(),
        }
    }
}

impl Settings {
    pub fn load() -> Settings {
        load("settings.json")
    }
    pub fn save(&self) -> Result<(), String> {
        save("settings.json", self)
    }
    pub fn fit_cfg(&self) -> FitCfg {
        FitCfg {
            f_low: self.fit_low,
            f_high: self.fit_high,
            bands: self.fit_bands,
            mode: if self.fit_mode == "treble" { FitMode::Treble } else { FitMode::Range },
            top: self.fit_top,
        }
    }
    pub fn set_fit_cfg(&mut self, c: &FitCfg) {
        self.fit_low = c.f_low;
        self.fit_high = c.f_high;
        self.fit_bands = c.bands;
        self.fit_mode = if c.mode == FitMode::Treble { "treble" } else { "range" }.into();
        self.fit_top = c.top;
    }
}

// ---- backup: everything in one file ----

pub fn backup_json(saved: &SavedEqs, frs: &FrStore, sets: &SetsDb) -> String {
    let v = serde_json::json!({
        "version": 1,
        "eqs": saved_to_value(saved),
        "frs": { "meas": frs.meas, "targets": frs.targets, "rig": frs.rig },
        "sets": sets,
    });
    serde_json::to_string_pretty(&v).unwrap_or_default()
}

pub struct Restored {
    pub eqs: usize,
    pub frs: usize,
    pub sets: usize,
}

/// Merges a backup in: entries with the same name are replaced, nothing else is touched.
pub fn restore(text: &str, saved: &mut SavedEqs, frs: &mut FrStore, sets: &mut SetsDb) -> Result<Restored, String> {
    let o: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if !o.is_object() {
        return Err("not a backup".into());
    }
    let mut r = Restored { eqs: 0, frs: 0, sets: 0 };
    if let Some(e) = o.get("eqs") {
        for (k, v) in saved_from_value(e) {
            saved.insert(k, v);
            r.eqs += 1;
        }
    }
    if let Some(f) = o.get("frs") {
        for (key, store) in [("meas", &mut frs.meas), ("targets", &mut frs.targets)] {
            if let Some(Value::Object(m)) = f.get(key) {
                for (k, v) in m {
                    if let Ok(rec) = serde_json::from_value::<FrRec>(v.clone()) {
                        store.insert(k.clone(), rec);
                        r.frs += 1;
                    }
                }
            }
        }
    }
    if let Some(s) = o.get("sets") {
        if let Ok(db) = serde_json::from_value::<SetsDb>(s.clone()) {
            for (k, v) in db.sets {
                sets.sets.insert(k, v);
                r.sets += 1;
            }
            for (d, names) in db.device_names {
                let e = sets.device_names.entry(d).or_default();
                for (slot, n) in names {
                    e.insert(slot, n);
                }
            }
        }
    }
    Ok(r)
}
