// Data: Cargo lurches to its time crate by crate while mbx springs to its
// mark, the delta lands, and the chart falls away into the first cube.

import type { Section } from "../bible";
import { CARGO_DONE, mbxGrow, mbxLock, SETTLE, STEP_PLAN, T } from "../scenes/s5-data";
import type { Part } from ".";
import { ad, crossing, hold, hz, line, type Mix, onFrame, perc, type Pt, swell, sweep } from "./mix";
import { A, bassBars, CHORD, chordBars, D, drumBars } from "./grooves";
import { ding, knock, ping, pop, thump, tick, whoosh } from "./sounds";

/**
 * Cargo lurches forward crate by crate (s5-data STEP_PLAN): steps on b0.5,
 * b0.75, and b1, a fourth two frames after b1.25, then a grind that starts
 * slow, arrives at speed, and clunks home half a frame before b1.75
 * (CARGO_DONE, so the frame nearest the beat shows it home). Each lurch
 * knocks a scale step higher (the fold motif, an octave down) and its
 * readout's drums ratchet over.
 */
function cargoBar(m: Mix, s: Section): void {
  const steps = STEP_PLAN.slice(0, 4).map(([t]) => s.at(t));
  steps.forEach((t, i) => {
    knock(m, t, hz([45, 47, 49, 50][i]), 0.3 + 0.04 * i, -0.2, 0.08);
    const s = m.voice(perc(t, 0.12, 0.003, 0.07), { pan: -0.2, send: 0.06 });
    if (s) s.noise("pink", 1, s.filter("bandpass", 800, 1.1));
    for (let k = 0; k < 3; k++) tick(m, t + 0.014 + k * 0.016, 1500 + 110 * i + 60 * k, 0.05, -0.25);
  });
  const g0 = s.at(STEP_PLAN[4][0]);
  const home = s.at(CARGO_DONE);
  m.duck(home, 0.2, 0.1);
  const g = m.voice(hold(g0, 0.03, 0.05, home - 0.006, 0.2, 0.012), { pan: -0.15, send: 0.06, hold: true });
  if (g) {
    const bp = g.filter("bandpass", sweep(g0, 450, home, 1300), 1.2);
    g.noise("crackle", 2.5, bp, sweep(g0, 0.45, home, 1.3));
    g.noise("pink", 0.6, bp);
    g.osc("sawtooth", sweep(g0, hz(33), home, hz(38)), 0.3, g.filter("lowpass", 500, 2));
  }
  thump(m, home, 0.45, 140, 55, 0.22, { pan: -0.1 });
  knock(m, home, hz(38), 0.32, -0.15, 0.1);
  tick(m, home, 2200, 0.16, -0.2);
}

/**
 * mbx launches on s5-data's spring (`mbxGrow`: 3.7 Hz, damping 0.66, launch
 * velocity 7.75): its tone rises an octave with the bar, overshoots with it,
 * and settles, while the readout ratchets up evenly. The scene locks the
 * readout half a frame before the bar first reaches its mark (`mbxLock`), so
 * the lock, the photo-finish hairline, and the flash all first show on the
 * frame after that (2.6 ms before b1.25); the lock sounds there.
 */
function mbxBar(m: Mix, s: Section): void {
  const t0 = s.at(T.mbx);
  const grow = (t: number) => mbxGrow(t - s.start);
  const lock = onFrame(s.at(mbxLock()));
  m.duck(lock, 0.3, 0.15);
  pop(m, t0, hz(69), 0.2, 0.05, 0.15);
  tick(m, t0, 2600, 0.14, 0.05);
  const v = m.voice(hold(t0, 0.012, 0.15, lock + 0.1, 0.12, 0.3), { send: 0.22, hold: true, pan: 0.05 });
  if (v) {
    const pitch: Pt[] = [[t0, hz(57)]];
    for (let i = 1; i <= 16; i++) pitch.push([t0 + i * 0.025, hz(57) * 2 ** grow(t0 + i * 0.025), "exp"]);
    const lp = v.filter("lowpass", [[t0, 700], [lock, 4200, "exp"], [lock + 0.4, 1200, "exp"]], 3);
    v.osc("sawtooth", pitch, 0.45, lp, -8);
    v.osc("sawtooth", pitch, 0.45, lp, 8);
  }
  for (let k = 1; k <= 7; k++) {
    const t = crossing(grow, k / 8, t0, 0.2);
    tick(m, t, 2400 + k * 40, 0.06, 0.15);
  }
  thump(m, lock, 0.3, 160, 80, 0.15);
  tick(m, lock, 3000, 0.2, 0.1);
  ping(m, lock, hz(81), 0.17, 0.4, { send: 0.3, pan: 0.1 });
  ping(m, lock + 0.004, hz(86), 0.12, 0.4, { send: 0.3, pan: 0.2 });
}

function delta(m: Mix, t: number, label: number, glint: number): void {
  m.duck(t, 0.3, 0.15);
  tick(m, t, 2800, 0.24, 0.1, 0.12);
  // Short enough to clear the way for the figure's pop a sixteenth later.
  ping(m, t, hz(81), 0.18, 0.28, { send: 0.3 });
  ping(m, t + 0.05, hz(86), 0.2, 0.32, { send: 0.3 });
  // The figure pops in, then glints.
  tick(m, label, 2600, 0.16, 0.15);
  pop(m, label, hz(90), 0.2, 0.15, 0.2);
  ding(m, glint, hz(98), 0.05, 0.2, 0.5, 0.4);
}

/**
 * The chart crouches for 0.11 s, then clears on b3 as one move: the page
 * falls back in perspective while the bar compacts to a square on b3.25,
 * extrudes, and the camera swings round and eases to a stop on the world
 * view 45 ms before the cut (s5 SETTLE), where the cube sits as the handoff.
 * The stop is an ease to rest with nothing landing, so it gets no hit: the
 * drone and the swing fade out on it and leave the downbeat to the kick.
 */
function extrude(m: Mix, clear: number, hit: number, settle: number): void {
  m.duck(clear, 0.25, 0.1);
  m.duck(hit, 0.25, 0.12);
  // The page swipes away and falls back: a bright swipe, then air falling in pitch.
  const sw = m.voice(perc(clear, 0.2, 0.0008, 0.035), { send: 0.1 });
  if (sw) sw.noise("white", 1, sw.filter("highpass", 2800, 0));
  whoosh(m, swell(clear - 0.1, clear - 0.04, 0.03, clear, 0.12, clear + 0.02), sweep(clear - 0.1, 900, clear, 2600), 1.2, {
    send: 0.1,
  });
  whoosh(m, ad(clear, clear + 0.02, 0.3, clear + 0.3), sweep(clear, 5000, clear + 0.28, 600), 0.8, { send: 0.12 }, "white");
  knock(m, hit, hz(50), 0.3, 0, 0.12);
  const v = m.voice(hold(hit, 0.06, 0.08, settle - 0.015, 0.2, 0.04), { send: 0.2, hold: true });
  if (v) {
    const lp = v.filter("lowpass", sweep(hit, 250, settle, 2600), 8);
    const g = sweep(hit, hz(38), settle, hz(50));
    v.osc("sawtooth", g, 0.5, lp, -7);
    v.osc("sawtooth", g, 0.5, lp, 7);
  }
  // The camera swings off the chart and back to the world view.
  const band: Pt[] = [[clear, 700], [clear + 0.25, 1800, "exp"], [settle, 900, "exp"]];
  whoosh(m, ad(clear + 0.05, clear + 0.25, 0.09, settle), band, 1.2, { pan: line(clear, -0.5, settle, 0.5), send: 0.1 });
}

export const part: Part = {
  cues(m, s) {
    cargoBar(m, s);
    mbxBar(m, s);
    delta(m, s.at(T.delta), s.at(T.label), s.at(T.glint));
    extrude(m, s.at(T.clear), s.at(T.hit), s.at(SETTLE));
  },
  // The drums thin out under the finished chart's hold.
  drums: (m, s) => drumBars(m, s, [[[0, 6, 8], [4, 12], [7, 15]], [[0, 8], [4, 12]]]),
  bass: (m, s) => bassBars(m, s, [A, D, A]),
  pads: (m, s) => chordBars(m, s, [CHORD.A, CHORD.D, CHORD.A], 1700),
};
