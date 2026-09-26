// Same checkout, empty target/: the full groove returns. The terminal flips
// over into the benchmark card, the old tape tears off and the lid springs
// up; restored outputs shimmer out of the box into target/, the glint tings
// on each sweep, the cheeks swell warm at the first hit, the lid clicks down
// with the mascot's, the tape rips on, and the strawberry lands with a plop
// and a bell.

import { rng } from "../math";
import {
  BUILD,
  LID_STEPS,
  SWEEPS,
  T_BERRY,
  T_BLINK,
  T_BLUSH,
  T_FLIP0,
  T_FLIP1,
  T_LABEL,
  T_LIFT,
  T_RIP0,
  T_RIP1,
  T_TAPE0,
  T_TAPE1,
} from "../scenes/same-checkout";
import type { Part } from ".";
import { lidClick, tapeZip } from "./first-build";
import { A, bassBars, CHORD, chordBars, drumBars, FULL, G } from "./grooves";
import { ad, hold, hz, line, type Mix, perc, sweep } from "./mix";
import { blip, bloop, boing, ding, tick, whoosh } from "./sounds";

/** Mr Boxington stands left of the middle, the card right of it. */
const BOX = -0.45;
const CARD = 0.45;

/** The card turning over: a quick fwip that peaks edge on. */
function flip(m: Mix, t0: number, t1: number): void {
  const mid = (t0 + t1) / 2;
  whoosh(m, ad(t0, mid, 0.16, t1 + 0.04), sweep(t0, 900, mid, 3600), 1.8, { pan: CARD, send: 0.15 }, "white");
  tick(m, t1, 2400, 0.14, CARD, 0.1);
}

/** The old tape torn off in one pull. */
function rip(m: Mix, t0: number, t1: number): void {
  const v = m.voice(hold(t0, 0.01, 0.34, t1, 0.2, 0.05), { pan: line(t0, BOX - 0.1, t1, BOX + 0.15), send: 0.1 });
  if (!v) return;
  const am = v.vca(0.6, v.filter("bandpass", sweep(t0, 2200, t1, 4200), 0.9));
  v.lfo("sawtooth", sweep(t0, 60, t1, 150), 0.4, am.gain);
  v.noise("crackle", 3, am, sweep(t0, 1.2, t1, 2.8));
}

/** The warm build pouring out: a shimmer of high pings, one for every other hit, travelling right. */
function shimmer(m: Mix): void {
  const r = rng(707);
  const notes = [86, 88, 90, 93, 95, 98, 100, 102];
  BUILD.units.forEach((u, i) => {
    if (i % 2) return;
    const n = notes[Math.floor(r() * notes.length)];
    ping(m, u.at, hz(n), 0.035 + 0.02 * r(), -0.2 + 0.7 * r());
  });
}

function ping(m: Mix, t: number, f: number, vel: number, pan: number): void {
  const v = m.voice(perc(t, vel, 0.001, 0.14), { pan, send: 0.3 });
  if (!v) return;
  v.osc("sine", f);
  v.osc("sine", f * 2.76, perc(t, 0.2, 0.0005, 0.04));
}

/** The cheeks warm: a short swell on the chord. */
function blush(m: Mix, t: number): void {
  const v = m.voice(ad(t, t + 0.12, 0.05, t + 0.6), { pan: BOX, send: 0.35, hold: true });
  if (!v) return;
  const lp = v.filter("lowpass", sweep(t, 800, t + 0.2, 2400), 1);
  for (const n of [67, 71, 74]) v.osc("triangle", hz(n), 0.3, lp);
}

export const part: Part = {
  cues(m, s) {
    flip(m, s.at(T_FLIP0), s.at(T_FLIP1));
    rip(m, s.at(T_RIP0), s.at(T_RIP1));
    m.duck(s.at(T_LIFT), 0.12, 0.1);
    boing(m, s.at(T_LIFT), 0.14, hz(55), 0.3);
    tick(m, s.at(T_LABEL), 2100, 0.08, CARD, 0.2);
    shimmer(m);
    // A ting as each sweep of the glint crosses the lens.
    SWEEPS.forEach((w, i) => ding(m, w.at, hz(98 + (i % 2) * 2), 0.09, BOX + 0.15, 0.5, 0.35));
    blush(m, s.at(T_BLUSH));
    LID_STEPS.forEach((t, i) => lidClick(m, t, i, BOX));
    tapeZip(m, s.at(T_TAPE0), s.at(T_TAPE1), BOX);
    // The strawberry: a plop onto the lid, then a small bell.
    m.duck(s.at(T_BERRY), 0.2, 0.12);
    bloop(m, s.at(T_BERRY), hz(62), 0.3, BOX);
    ding(m, s.at(T_BERRY) + 0.07, hz(93), 0.14, BOX + 0.1, 1.1);
    ding(m, s.at(T_BERRY) + 0.074, hz(98), 0.07, BOX + 0.2, 0.9);
    blip(m, s.at(T_BLINK), hz(93), 0.07);
    blip(m, s.at(T_BLINK) + 0.07, hz(97), 0.05);
  },
  drums: (m, s) => drumBars(m, s, [FULL]),
  bass: (m, s) => bassBars(m, s, [G, A]),
  pads: (m, s) => chordBars(m, s, [CHORD.G, CHORD.A]),
};
