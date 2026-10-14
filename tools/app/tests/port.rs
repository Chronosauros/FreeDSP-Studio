// The Rust port against the JavaScript it replaced. tests/reference.json was produced by running
// the last HTML version's own functions (index.html, v0.7.2) headless on these inputs; every
// case here must come out the same.

use freedsp_studio::eq::{self, Band, Caps, FType, Field};
use freedsp_studio::fit::{self, FitCfg, FitMode, FitOutcome};
use freedsp_studio::fr::{self, Smoothed};
use freedsp_studio::treble::{self, Loudness, Pair, Stair};
use serde_json::Value;

fn reference() -> Value {
    serde_json::from_str(include_str!("reference.json")).unwrap()
}

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol * (1.0 + b.abs())
}

fn band(v: &Value) -> Band {
    Band {
        freq: v["freq"].as_f64().unwrap(),
        gain: v["gain"].as_f64().unwrap(),
        q: v["q"].as_f64().unwrap(),
        typ: FType::from_code(v["type"].as_str().unwrap_or("PK")),
    }
}

fn curve(v: &Value) -> Vec<[f64; 2]> {
    v.as_array().unwrap().iter().map(|p| [p[0].as_f64().unwrap(), p[1].as_f64().unwrap()]).collect()
}

fn caps(v: &Value) -> Caps {
    Caps {
        gen: v["gen"].as_u64().unwrap() as u8,
        bands: v["bands"].as_u64().unwrap() as usize,
        g_min: v["gainMin"].as_f64().unwrap(),
        g_max: v["gainMax"].as_f64().unwrap(),
        q_min: v["qMin"].as_f64().unwrap(),
        q_max: v["qMax"].as_f64().unwrap(),
        types: v["types"].as_array().unwrap().iter().map(|t| FType::from_code(t.as_str().unwrap())).collect(),
        preamp_step: v["preampStep"].as_f64().unwrap(),
    }
}

fn model_from(v: &Value) -> Vec<Band> {
    v.as_array().unwrap().iter().map(band).collect()
}

fn assert_bands(got: &[Band], want: &[Band], what: &str) {
    assert_eq!(got.len(), want.len(), "{}: band count", what);
    for (i, (g, w)) in got.iter().zip(want).enumerate() {
        assert!(
            close(g.freq, w.freq, 1e-9) && close(g.gain, w.gain, 1e-9) && close(g.q, w.q, 1e-9) && g.typ == w.typ,
            "{}: band {} is {:?}, want {:?}",
            what,
            i,
            g,
            w
        );
    }
}

#[test]
fn magnitude_and_peak() {
    let r = reference();
    for m in r["mag"].as_array().unwrap() {
        let (b, f, v) = (band(&m[0]), m[1].as_f64().unwrap(), m[2].as_f64().unwrap());
        assert!(close(eq::mag_db(&b, f), v, 1e-9), "{:?} at {}", b, f);
    }
    let model: Vec<Band> = {
        let mut m: Vec<Band> = r["mag"].as_array().unwrap().iter().step_by(5).map(|m| band(&m[0])).collect();
        m.extend((0..4).map(|_| Band::pk(1000.0, 0.0, 1.0)));
        m
    };
    for c in r["combined"].as_array().unwrap() {
        assert!(close(eq::combined_at(&model, c[0].as_f64().unwrap()), c[1].as_f64().unwrap(), 1e-9));
    }
    let (p, pf) = eq::peak(&model);
    assert!(close(p, r["peak"]["p"].as_f64().unwrap(), 1e-9));
    assert_eq!(pf, r["peak"]["pf"].as_f64().unwrap());
    assert_eq!(eq::eq_to_text(&model, -1.5), r["eqText"].as_str().unwrap());
}

#[test]
fn squig_text() {
    let r = reference();
    let p = eq::parse_squig("Preamp: -3.8 dB\nFilter 1: ON PK Fc 105 Hz Gain 3.0 dB Q 0.70\nFilter 2: ON LSC Fc 60 Hz Gain 4.0 dB Q 0.70\nFilter 3: OFF PK Fc 1000 Hz Gain 1 dB Q 1\nFilter: ON HSC Fc 8000 Hz Gain -2.5 dB Q 0.707\nFilter 5: ON LPQ Fc 15000 Hz Gain 0 dB Q 0.5\ngarbage");
    let s = &r["squig"];
    assert_eq!(p.preamp, s["preamp"].as_f64().unwrap());
    assert_eq!(p.skipped as u64, s["skipped"].as_u64().unwrap());
    assert_bands(&p.filters, &model_from(&s["filters"]), "squig");

    let c = caps(&r["caps"]["capsGen2"]);
    let pa = eq::Parsed {
        preamp: -3.84,
        filters: vec![
            Band { freq: 10.0, gain: 20.0, q: 20.0, typ: FType::PK },
            Band { freq: 1000.0, gain: 2.0, q: 1.0, typ: FType::LP },
            Band { freq: 80.0, gain: -3.0, q: 0.7, typ: FType::LSQ },
        ],
        skipped: 1,
    };
    let (m, pre, msg) = eq::apply_filters(&pa, &c, c.g_max);
    let a = &r["applyFilters"];
    assert_eq!(msg, a["msg"].as_str().unwrap());
    assert_eq!(pre, a["preamp"].as_f64().unwrap());
    assert_bands(&m, &model_from(&a["model"]), "applyFilters");
}

#[test]
fn fr_parsing_and_smoothing() {
    let r = reference();
    let got = fr::parse_fr("Frequency,dB\n20, 1.5\n1000\t0\n# comment\n500;-2.5e0\n-5 3\n20000 4.25\nabc 1");
    assert_eq!(got, curve(&r["parseFR"]));
    let a = curve(&r["curves"]["iemA"]);
    let sm = fr::smoothed(&a);
    for (i, w) in r["smooth"].as_array().unwrap().iter().enumerate() {
        let g = sm[i * 20];
        assert!(close(g[0], w[0].as_f64().unwrap(), 1e-12) && close(g[1], w[1].as_f64().unwrap(), 1e-9), "smooth {}", i);
    }
    let avg = fr::average(&[vec![[100.0, 0.0], [1000.0, 3.0], [10000.0, -6.0]], vec![[100.0, 2.0], [1000.0, 1.0], [10000.0, -2.0]]]);
    for (g, w) in avg.iter().zip(curve(&r["avg"])) {
        assert!(close(g[1], w[1], 1e-12));
    }
    for s in r["split"].as_array().unwrap() {
        let got = fr::split_channel(s[0].as_str().unwrap());
        match &s[1] {
            Value::Null => assert!(got.is_none(), "{} should not split", s[0]),
            v => {
                let (b, c) = got.unwrap_or_else(|| panic!("{} should split", s[0]));
                assert_eq!(b, v["base"].as_str().unwrap());
                assert_eq!(c.to_string(), v["ch"].as_str().unwrap());
            }
        }
    }
    let prof = curve(&r["profile"]["bands"]);
    let meas = Smoothed::new(&a);
    let tgt = Smoothed::new(&curve(&r["curves"]["tgtH"]));
    for e in r["ear"].as_array().unwrap() {
        let f = e[0].as_f64().unwrap();
        assert!(close(fr::ear_meas_at(&meas, &prof, f), e[1].as_f64().unwrap(), 1e-9), "ear at {}", f);
        assert!(close(fr::fit_tgt_at(&tgt, f), e[2].as_f64().unwrap(), 1e-9), "tgt at {}", f);
        assert!(close(fr::treble_corr_at(&prof, f), e[3].as_f64().unwrap(), 1e-9), "corr at {}", f);
    }
}

#[test]
fn small_helpers() {
    let r = reference();
    let g2 = caps(&r["caps"]["capsGen2"]);
    for (n, v) in r["defaultFreqs"].as_object().unwrap() {
        let c = Caps { bands: n.parse().unwrap(), ..g2.clone() };
        let want: Vec<f64> = v.as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect();
        assert_eq!(c.default_freqs(), want, "{} bands", n);
    }
    let fi = caps(&r["caps"]["capsFiio"]);
    for p in r["preRound"].as_array().unwrap() {
        let v = p[0].as_f64().unwrap();
        assert_eq!(g2.pre_round(v), p[1].as_f64().unwrap(), "pre_round {}", v);
        assert_eq!(fi.pre_round(v), p[2].as_f64().unwrap(), "pre_round whole dB {}", v);
    }
    for p in r["headroom"].as_array().unwrap() {
        let v = p[0].as_f64().unwrap();
        assert_eq!(g2.headroom_for(v), p[1].as_f64().unwrap(), "headroom {}", v);
        assert_eq!(fi.headroom_for(v), p[2].as_f64().unwrap(), "headroom whole dB {}", v);
    }
    for p in r["parseField"].as_array().unwrap() {
        let k = match p[0].as_str().unwrap() {
            "freq" => Field::Freq,
            "gain" => Field::Gain,
            _ => Field::Q,
        };
        let got = eq::parse_field(k, p[1].as_str().unwrap());
        match p[2].as_f64() {
            Some(w) => assert_eq!(got, Some(w), "{}", p[1]),
            None => assert_eq!(got, None, "{}", p[1]),
        }
    }
    for p in r["fmt"].as_array().unwrap() {
        let k = match p[0].as_str().unwrap() {
            "freq" => Field::Freq,
            "gain" => Field::Gain,
            _ => Field::Q,
        };
        assert_eq!(eq::fmt_field(k, p[1].as_f64().unwrap()), p[2].as_str().unwrap());
    }
    let t = eq::Target {
        id: "x".into(),
        name: "FIIO BTR17".into(),
        gen: 3,
        bands: 3,
        g_min: -24.0,
        g_max: 12.0,
        q_min: 0.25,
        q_max: 8.0,
        types: vec![FType::PK, FType::LSQ],
        presets: Default::default(),
        user_slots: vec![],
        tested: true,
    };
    let inp = vec![
        Band { freq: 100.4, gain: 3.0, q: 0.1, typ: FType::HSQ },
        Band { freq: 2500.0, gain: -30.0, q: 2.0, typ: FType::PK },
        Band { freq: 9000.0, gain: 0.5, q: 9.0, typ: FType::PK },
        Band { freq: 40.0, gain: 14.0, q: 1.0, typ: FType::LSQ },
        Band { freq: 300.0, gain: 0.0, q: 1.0, typ: FType::PK },
    ];
    let f = eq::fit_to_target(&inp, -25.0, &t);
    let w = &r["fitToTarget"];
    assert_bands(&f.bands, &model_from(&w["bands"]), "fit_to_target");
    assert_eq!(f.preamp, w["preamp"].as_f64().unwrap());
    let wi: Vec<&str> = w["issues"].as_array().unwrap().iter().map(|x| x.as_str().unwrap()).collect();
    assert_eq!(f.issues, wi);
    assert_eq!(eq::target_id("FIIO BTR17"), "fiio-btr17");
}

#[test]
fn listening_test_maths() {
    let r = reference();
    let prof = curve(&r["profile"]["bands"]);
    for (g, w) in fr::detrend(&prof).iter().zip(curve(&r["detrend"])) {
        assert!(close(g[1], w[1], 1e-9));
    }
    let pairs: Vec<(usize, usize, f64)> =
        vec![(0, 1, 2.0), (1, 2, -1.5), (2, 3, 0.5), (3, 4, 1.0), (4, 5, -2.0), (5, 6, 0.0), (0, 2, 1.0), (1, 3, -1.0), (2, 4, 1.5), (3, 5, -1.0), (4, 6, -2.0)];
    let (x, rms) = treble::solve_pairs(&pairs, 7);
    for (g, w) in x.iter().zip(r["solve"]["x"].as_array().unwrap()) {
        assert!(close(*g, w.as_f64().unwrap(), 1e-9));
    }
    assert!(close(rms, r["solve"]["rms"].as_f64().unwrap(), 1e-9));

    let mut s = Stair::new(8000.0, 3.0);
    for a in [true, true, false, true, false, false, true, false, true, true] {
        if !s.done {
            s.answer(a);
        }
    }
    let w = &r["stair"]["state"];
    assert_eq!(s.g, w["g"].as_f64().unwrap());
    assert_eq!(s.n as u64, w["n"].as_u64().unwrap());
    assert_eq!(s.done, w["done"].as_bool().unwrap());
    assert!(close(s.value(), r["stair"]["value"].as_f64().unwrap(), 1e-12));

    let mut p = Pair::fixed(0, 1);
    for a in [Loudness::Louder, Loudness::Louder, Loudness::Quieter, Loudness::Louder, Loudness::Quieter, Loudness::Quieter] {
        if !p.done {
            p.answer(a);
        }
    }
    assert_eq!(p.d, r["pair"]["state"]["d"].as_f64().unwrap());
    assert!(close(p.value(), r["pair"]["value"].as_f64().unwrap(), 1e-12));
}

#[test]
fn auto_fit_matches() {
    let r = reference();
    let prof = curve(&r["profile"]["bands"]);
    for (n, case) in r["fit"].as_array().unwrap().iter().enumerate() {
        let meas = Smoothed::new(&curve(&r["curves"][case["meas"].as_str().unwrap()]));
        let tgt = Smoothed::new(&curve(&r["curves"][case["tgt"].as_str().unwrap()]));
        let c = caps(&r["caps"][case["caps"].as_str().unwrap()]);
        let cfg = &case["cfg"];
        let fc = FitCfg {
            f_low: cfg["fLow"].as_f64().unwrap(),
            f_high: cfg["fHigh"].as_f64().unwrap(),
            bands: cfg["bands"].as_u64().unwrap() as usize,
            mode: if cfg["mode"] == "treble" { FitMode::Treble } else { FitMode::Range },
            top: cfg["top"].as_f64().unwrap(),
        };
        let inp = fit::FitInput { meas: &meas, tgt: &tgt, prof: Some(&prof), cfg: fc, caps: &c, g_max: c.g_max };
        let FitOutcome::Bands(b) = fit::auto_fit(&inp) else { panic!("case {}: no profile", n) };
        let model = c.fill(&b);
        let pre = c.headroom_for(eq::peak(&model).0);
        assert_bands(&model, &model_from(&case["model"]), &format!("fit case {}", n));
        assert_eq!(pre, case["preamp"].as_f64().unwrap(), "fit case {} preamp", n);
    }
}

#[test]
fn treble_apply_matches() {
    let r = reference();
    let prof = curve(&r["profile"]["bands"]);
    let c = caps(&r["caps"]["capsFiio"]);
    let mut model = vec![Band { freq: 100.0, gain: 3.0, q: 0.7, typ: FType::LSQ }, Band { freq: 3000.0, gain: -4.0, q: 1.2, typ: FType::PK }];
    model.extend([31.0, 62.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0].iter().map(|&f| Band::blank(f)));
    let w = &r["trebleApply"];
    match fit::apply_treble(&prof, &model, -2.0, &c, c.g_max, 16000.0) {
        fit::TrebleApply::Changed { model, preamp, msg } => {
            assert_eq!(msg, w["msg"].as_str().unwrap());
            assert_eq!(preamp, w["preamp"].as_f64().unwrap());
            assert_bands(&model, &model_from(&w["model"]), "treble apply");
        }
        fit::TrebleApply::Unchanged { msg, .. } => panic!("unchanged: {}", msg),
    }
}
