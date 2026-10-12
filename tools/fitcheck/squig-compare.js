// Auto-fit head-to-head: FreeDSP Studio's autoFit against squig.link's and graph.hangout.audio's
// own Auto EQ, on identical curves, scored the same way.
//
//   node tools/fitcheck/squig-compare.js
//   node tools/fitcheck/squig-compare.js "Xenns Top Pro" "Moondrop Dusk 2 S2 USB Default" 6000,10000 10
//
// Arguments: squig.link measurement file name, target file name (both as in squig.link/data, without
// the " L.txt" / " R.txt"), the fit's upper bounds, and the band count. Their equalizer.js and the
// curves are fetched from the sites each run, so the comparison is always against what they serve
// today; nothing of theirs is kept in this repo. Our fit is taken from the app's index.html.
//
// Like squig, the measurement is its L/R average (of amplitude) and the target is its first channel.
// Note the sites' "range" only limits where filters may go, while the error they minimise covers
// 20 Hz-20 kHz; ours fits up to the bound and keeps the EQ flat above it. So the column above the
// bound isn't a like-for-like comparison.
//
// The sites' equalizer.js is downloaded code, and Node's vm module is not a security boundary, so
// it only runs if its SHA-256 matches a version that has been read and pinned below. When a site
// changes the file the check stops and prints the new hash: read the new file, then add the hash.
// (The vm context just keeps it from touching this script's variables.) Our own fit is evaluated
// from the app's index.html. This is a developer check, not part of the app.

const fs = require('fs');
const path = require('path');
const vm = require('vm');
const crypto = require('crypto');

// Reviewed versions, 2026-10-12.
const PINNED = {
  'https://squig.link/equalizer.js': ['a8e630acee49b02355bfc794d7cb306ce47d7bdd6f9bf0c70042f9aef1fac124'],
  'https://graph.hangout.audio/equalizer.js': ['c261f39fb31c34af5f270bf3080c7529194b709ed5ef3763a01b2261d36cd4ce'],
};

const [, , MEAS = 'Xenns Top Pro', TGT = 'Moondrop Dusk 2 S2 USB Default', HIS = '6000,10000', BANDS = '10'] = process.argv;
const PAGE = path.join(__dirname, '..', 'app', 'renderer', 'index.html');
const SITES = [['squig.link', 'https://squig.link/equalizer.js'], ['graph.hangout', 'https://graph.hangout.audio/equalizer.js']];

async function get(url) {
  const r = await fetch(url);
  if (!r.ok) throw new Error(`${url}: HTTP ${r.status}`);
  return r.text();
}
const parseFR = (t) => t.split(/\r?\n/).map(l => l.trim().match(/^([\d.]+)[\s,;]+(-?[\d.eE+]+)/)).filter(Boolean).map(m => [+m[1], +m[2]]);
const at = (pts, f) => {
  if (f <= pts[0][0]) return pts[0][1];
  for (let i = 1; i < pts.length; i++) if (pts[i][0] >= f) {
    const [f0, d0] = pts[i - 1], [f1, d1] = pts[i];
    return d0 + (d1 - d0) * Math.log(f / f0) / Math.log(f1 / f0);
  }
  return pts[pts.length - 1][1];
};
const norm = (pts) => { const r = at(pts, 1000); return pts.map(([f, v]) => [f, v - r]); };
const avgAmp = (cs) => cs[0].map(([f], i) => [f, 20 * Math.log10(cs.reduce((s, c) => s + Math.pow(10, c[i][1] / 20), 0) / cs.length)]);

function peaking(b, f, fs = 48000) {
  const A = Math.pow(10, b.gain / 40), w = 2 * Math.PI * b.freq / fs, al = Math.sin(w) / (2 * b.q), cw = Math.cos(w);
  const B = [1 + al * A, -2 * cw, 1 - al * A], D = [1 + al / A, -2 * cw, 1 - al / A];
  const W = 2 * Math.PI * f / fs, c1 = Math.cos(W), s1 = Math.sin(W), c2 = Math.cos(2 * W), s2 = Math.sin(2 * W);
  const nr = B[0] + B[1] * c1 + B[2] * c2, ni = -(B[1] * s1 + B[2] * s2), dr = D[0] + D[1] * c1 + D[2] * c2, di = -(D[1] * s1 + D[2] * s2);
  return 10 * Math.log10((nr * nr + ni * ni) / (dr * dr + di * di));
}

// Our fit, lifted out of the page with just enough of the app around it to run.
function ourFit(meas, tgt, hi, n) {
  const src = fs.readFileSync(PAGE, 'utf8');
  const grab = (a, b) => { const i = src.indexOf(a), j = src.indexOf(b, i); if (i < 0 || j < 0) throw new Error(`index.html: can't find ${a}`); return src.slice(i, j); };
  const code = [grab('const clamp =', '// Band count'), grab('function rbj(', 'function combinedAt'), grab('function interpFR', 'function normalizeFR'),
    grab('const FIT_N', 'function loadFitCfg'), grab('const TT_BANDS', '// ---- backup')].join('\n').replace(/^let /gm, 'var ').replace(/^const /gm, 'var ');
  var LIM = { fMin: 20, fMax: 20000, gMin: -24, gMax: 12, qMin: 0.25, qMax: 8 }, NB = n, model = [], preamp = 0;
  var fitCfg = { fLow: 20, fHigh: hi, bands: n, mode: 'range', top: 16000 };
  var curMeas = 'm', curTgt = 't:t', frCache = { meas: { m: { rig: '711', points: meas } }, targets: { t: { rig: '711', points: tgt } } };
  function tgtRec() { return frCache.targets.t; }
  var curGMax = () => LIM.gMax, pushUndo = () => {}, renderAll = () => {}, fillBands = (b) => b, peak = () => ({ p: 0 }), syncTrebleUI = () => {}, saveTrebleView = () => {};
  eval(code);
  autoFit();
  return model;
}

(async () => {
  const data = (name, ch) => get(`https://squig.link/data/${encodeURIComponent(`${name} ${ch}.txt`)}`).then(parseFR);
  const [mL, mR, tL] = await Promise.all([data(MEAS, 'L'), data(MEAS, 'R'), data(TGT, 'L')]);
  const meas = norm(avgAmp([mL, mR])), tgt = norm(tL);
  const sites = await Promise.all(SITES.map(async ([label, url]) => {
    const code = await get(url);
    const hash = crypto.createHash('sha256').update(code).digest('hex');
    if (!(PINNED[url] || []).includes(hash)) {
      throw new Error(`${url} has changed (sha256 ${hash}). Read the new version before trusting it, then add the hash to PINNED.`);
    }
    return [label, new vm.Script(code, { filename: url })];
  }));
  // Runs one site's autoeq in a fresh, empty context; inputs and result cross as JSON text.
  const siteFit = (script, hi) => {
    const ctx = vm.createContext({ input: JSON.stringify({ meas, tgt, hi, n: +BANDS }) });
    script.runInContext(ctx, { timeout: 60000 });
    return JSON.parse(vm.runInContext(
      'const a = JSON.parse(input); Equalizer.config.AutoEQRange = [20, a.hi]; JSON.stringify(Equalizer.autoeq(a.meas, a.tgt, a.n))',
      ctx, { timeout: 60000 }));
  };

  const G = Array.from({ length: 481 }, (_, i) => 20 * Math.pow(1000, i / 480));
  const score = (filters, hi) => {
    const r = G.map(f => at(meas, f) + filters.reduce((s, b) => s + (b.gain ? peaking(b, f) : 0), 0) - at(tgt, f));
    const r0 = r[G.findIndex(f => f >= 1000)];
    const rms = (a, b) => { const v = r.filter((_, i) => G[i] >= a && G[i] <= b).map(x => x - r0); return Math.sqrt(v.reduce((s, x) => s + x * x, 0) / v.length).toFixed(2); };
    return `20-1k ${rms(20, 1000)} | 1k-${hi / 1000}k ${rms(1000, hi)} | ${hi / 1000}k-20k ${rms(hi, 20000)}`;
  };
  const fmt = (fl) => fl.filter(b => b.gain).sort((a, b) => a.freq - b.freq)
    .map(b => `${Math.round(b.freq)}/${b.gain > 0 ? '+' : ''}${(+b.gain).toFixed(1)}/${(+b.q).toFixed(2)}`).join('  ');

  console.log(`${MEAS} (L/R average) -> ${TGT} (L), ${BANDS} bands; RMS dB vs target, lower is better`);
  console.log(`no EQ            ${score([], 6000)}`);
  for (const hi of HIS.split(',').map(Number)) {
    console.log(`\nfilters up to ${hi} Hz`);
    const runs = sites.map(([label, script]) => [label, siteFit(script, hi)]);
    runs.push(['FreeDSP Studio', ourFit(meas, tgt, hi, +BANDS)]);
    for (const [label, fl] of runs) {
      console.log(`${label.padEnd(15)} ${String(fl.filter(b => b.gain).length).padStart(2)} bands  ${score(fl, hi)}`);
      console.log(`                ${fmt(fl)}`);
    }
  }
})().catch(e => { console.error(e.message); process.exit(1); });
