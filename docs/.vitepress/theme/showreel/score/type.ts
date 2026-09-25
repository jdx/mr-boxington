// Kinetic type: the dive through the monocle, the hero line typing on, the
// three slams of the lockup, and the names flying off to the flow scene.
// The iris whomp and the three slams are its loudest moments; they sit about
// 2 LU under the resolve so the end card stays the peak.

import { BEAT, type Section } from "../bible";
import { rng } from "../math";
import {
  HERO,
  KEY_T,
  MAIN_TIP,
  T_AND,
  T_BRANCH,
  T_CI,
  T_IRIS,
  T_LAND,
  T_MAIN,
  T_OUT,
  T_P,
  T_SNAP,
  T_W,
} from "../scenes/s3-type";
import type { Part } from ".";
import { ad, hz, line, type Mix, perc, swell, sweep } from "./mix";
import { bassBar, grooveBar, knock, pad, ping, pop, thump, tick, whoosh } from "./sounds";

function dive(m: Mix, t0: number, t1: number): void {
  m.duck(t1, 0.35, 0.15);
  // The lens catches the light as the camera dives in.
  ping(m, t0, hz(98), 0.07, 0.25, { pan: 0.25, send: 0.35 });
  tick(m, t0, 3000, 0.12, 0.25);
  whoosh(m, swell(t0, t0 + 0.05, 0.03, t1 - 0.012, 0.36, t1 + 0.07), sweep(t0, 300, t1, 4200), 1.3, { send: 0.2 });
  // Glass rising an octave as the lens fills the frame.
  const g = m.voice(swell(t0, t0 + 0.05, 0.01, t1 - 0.012, 0.07, t1 + 0.05), { send: 0.3, hold: true });
  if (g) g.osc("sine", sweep(t0, hz(74), t1, hz(86)));
  // The iris whomp.
  const w = m.voice(perc(t1, 0.39, 0.004, 0.45), { send: 0.15 });
  if (w) {
    w.osc("sine", sweep(t1, 95, t1 + 0.3, 38));
    w.noise("pink", perc(t1, 0.3, 0.002, 0.12), w.filter("lowpass", 700, 0), 1, t1 + 0.15);
  }
}

/** The hero line types on with scene 3's own uneven key rhythm (s3 KEY_T). */
function typing(m: Mix, s: Section): void {
  const text = HERO;
  const r = rng(303);
  for (let i = 0; i < text.length; i++) {
    const t = s.at(KEY_T[i]);
    const f = 1900 + 700 * r();
    const pan = (r() - 0.5) * 0.3;
    // Every other key, plus each word's first: a fast, even patter.
    const wordStart = i === 0 || text[i - 1] === " ";
    if (i % 2 && !wordStart) continue;
    tick(m, t, f, wordStart ? 0.4 : 0.24, pan);
  }
}

/** Slam 1, "projects,": a woody boom, then the other letters knock in. */
function slamDrop(m: Mix, t: number): void {
  m.duck(t, 0.6, 0.22);
  thump(m, t, 0.6, 125, 40, 0.6);
  const v = m.voice(perc(t, 0.2, 0.001, 0.4), { send: 0.18 });
  if (v) {
    const lp = v.filter("lowpass", sweep(t, 2400, t + 0.25, 500), 2);
    v.osc("triangle", hz(50), 1, lp);
    v.noise("white", perc(t, 0.7, 0.0005, 0.05), lp, 1, t + 0.08);
  }
  // The other letters land on the scene's 1/128-bar stagger.
  const stag = BEAT / 32;
  const climb = [0, 2, 4, 7, 9, 12, 14, 16];
  for (let k = 1; k <= 8; k++) {
    knock(m, t + k * stag, hz(62 + climb[k - 1]), 0.11 * (1 - k / 12), (k % 2 ? -1 : 1) * 0.3, 0.1);
  }
}

/**
 * The git graph under the lockup (s3 T_MAIN): main's first commit pops on
 * b1.25 as "projects," settles, and the line draws out left to right to its
 * second commit just before the branch forks on b1.75 (s3 MAIN_TIP).
 */
function gitMain(m: Mix, t: number, tip: number): void {
  pop(m, t, hz(74), 0.13, -0.35, 0.2);
  whoosh(m, ad(t + 0.02, tip - 0.02, 0.05, tip + 0.03), sweep(t + 0.02, 1400, tip, 3200), 3, {
    pan: line(t + 0.02, -0.4, tip, 0.35),
    send: 0.15,
  });
  pop(m, tip, hz(78), 0.08, 0.35, 0.2);
}

/** Slam 2, "worktrees,": a plucked Bm stab over a low body. */
function slamSlide(m: Mix, t: number, branch: number): void {
  // The git branch leaves the main line: a rising zip that meets the hit.
  m.duck(t, 0.55, 0.2);
  whoosh(m, ad(branch, t - 0.004, 0.2, t + 0.07), sweep(branch, 900, t, 4200), 1.6, {
    pan: line(branch, -0.6, t, 0.3),
    send: 0.1,
  });
  const z = m.voice(ad(branch, t - 0.004, 0.05, t + 0.03), { send: 0.2, hold: true, pan: 0.2 });
  if (z) z.osc("triangle", sweep(branch, hz(59), t, hz(83)));
  // Bm triad plucked through a closing filter, wide, over a B1 body.
  for (const side of [-1, 1]) {
    const v = m.voice(perc(t, 0.24, 0.002, 0.45), { pan: side * 0.5, send: 0.22 });
    if (!v) continue;
    const lp = v.filter("lowpass", sweep(t, 6000, t + 0.25, 700), 4);
    for (const n of [59, 62, 66, 71]) v.osc("sawtooth", hz(n), 0.3, lp, side * 9);
  }
  thump(m, t, 0.6, 95, 46, 0.4);
  const b = m.voice(perc(t, 0.16, 0.002, 0.35), { send: 0.08 });
  if (b) {
    b.osc("sine", hz(35));
    b.osc("sine", hz(47), 0.45);
  }
  const s = m.voice(perc(t, 0.45, 0.0005, 0.05), { send: 0.15 });
  if (s) s.noise("white", 1, s.filter("bandpass", 2600, 1.2), 1, t + 0.07);
}

/** "and" glides in on the sixteenth pickup (s3 T_AND). */
function glideAnd(m: Mix, t: number): void {
  whoosh(m, ad(t - 0.02, t + 0.05, 0.08, t + 0.2), sweep(t - 0.02, 2600, t + 0.2, 1100), 1.5, { send: 0.15, pan: 0.1 });
  tick(m, t + 0.05, 2300, 0.08, 0.1, 0.1);
}

/** "CI" stamps giant with an RGB split, then snaps to size: three detuned layers pulling into unison. */
function slamGlitch(m: Mix, t: number, snap: number): void {
  m.duck(t, 0.6, 0.22);
  const f = hz(50);
  for (const [k, pan] of [[-1, -0.8], [0, 0], [1, 0.8]]) {
    const v = m.voice(perc(t, 0.12, 0.0008, 0.32), { pan, send: 0.08 });
    if (!v) continue;
    const cr = v.shaper(m.sh.crush, v.filter("lowpass", sweep(t, 7000, t + 0.3, 900), 1));
    v.osc("sawtooth", [[t, f * (1 + 0.07 * k)], [snap, f * (1 + 0.05 * k), "exp"], [snap + 0.012, f]], 1, cr);
    v.osc("square", [[t, f * 2 * (1 - 0.05 * k)], [snap, f * 2 * (1 - 0.04 * k), "exp"], [snap + 0.012, f * 2]], 0.4, cr);
  }
  thump(m, t, 0.55, 160, 44, 0.5);
  const b = m.voice(perc(t, 0.11, 0.0004, 0.05), { send: 0.1 });
  if (b) b.osc("square", hz(93), 1, b.filter("lowpass", 5000, 0));
  // The snap to size.
  const z = m.voice(perc(snap, 0.28, 0.0004, 0.05), { send: 0.15 });
  if (z) {
    z.noise("white", 1, z.filter("bandpass", sweep(snap, 6000, snap + 0.04, 1800), 2), 1, snap + 0.07);
    z.osc("sine", sweep(snap, 2400, snap + 0.03, 900), 0.4);
  }
}

/** The extra words fall away in fragments, each a falling note. */
function scatter(m: Mix, t: number): void {
  const r = rng(404);
  const scale = [74, 76, 78, 81, 83, 86, 88, 90, 93];
  for (let i = 0; i < 12; i++) {
    const ti = i === 0 ? t : t + i * 0.024 + r() * 0.018;
    const f = hz(scale[Math.floor(r() * scale.length)]);
    const pan = (r() * 2 - 1) * 0.7;
    const v = m.voice(perc(ti, 0.13 * (1 - i / 18), 0.001, 0.24), { pan, send: 0.22 });
    if (!v) continue;
    v.osc("sine", sweep(ti, f, ti + 0.24, f * 0.55));
    v.osc("triangle", sweep(ti, f * 2, ti + 0.1, f * 1.1), perc(ti, 0.12, 0.0005, 0.04));
  }
  whoosh(m, ad(t, t + 0.03, 0.09, t + 0.45), sweep(t, 3200, t + 0.45, 500), 1.5, { send: 0.2 });
}

/** The three names fly on arcs; their whooshes peak as they reach the labels (s3 T_LAND), a frame before the cut. */
function flyToNodes(m: Mix, t0: number, land: number): void {
  [-0.65, 0.6, 0.7].forEach((pan, k) => {
    const s = t0 + 0.1 + k * 0.03;
    whoosh(m, ad(s, land, 0.09, land + 0.07), sweep(s, 600, land, 2600 + 400 * k), 2, {
      pan: line(s, 0, land, pan),
      send: 0.15,
    });
  });
}

export const part: Part = {
  cues(m, s) {
    dive(m, s.start, s.at(T_IRIS));
    typing(m, s);
    slamDrop(m, s.at(T_P));
    gitMain(m, s.at(T_MAIN), s.at(MAIN_TIP));
    slamSlide(m, s.at(T_W), s.at(T_BRANCH));
    glideAnd(m, s.at(T_AND));
    slamGlitch(m, s.at(T_CI), s.at(T_SNAP));
    scatter(m, s.at(T_OUT));
    flyToNodes(m, s.at(T_OUT), s.at(T_LAND));
  },
  drums: (m, s) => grooveBar(m, s.start, [0, 8], [4, 12]),
  bass: (m, s) => bassBar(m, s.start, 35, 0.45), // Bm
  pads: (m, s) => pad(m, s.start, s.end, [47, 54, 57, 62, 64], 0.07, 1400),
};
