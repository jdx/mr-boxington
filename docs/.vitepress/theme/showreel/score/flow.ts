// Particles: the nodes pop, the box pops in between them, compiled crates
// stream into it and are swallowed on the sixteenths, restored outputs burst
// back out on the eighths, and the whip pan into the chart.

import { rng } from "../math";
import { BURSTS, GULPS, T_CHARGE, T_DONE, T_POP, T_TWINKLE } from "../scenes/s4-flow";
import type { Part } from ".";
import { ad, hz, line, type Mix, outCubicInv, perc, swell, sweep, X } from "./mix";
import { bassBar, ding, grooveBar, pad, ping, pop, thump, tick, whoosh } from "./sounds";

function nodes(m: Mix, t: number): void {
  // project, worktree, CI
  const list: [number, number, number][] = [
    [0, 74, -0.65],
    [0.06, 78, 0.55],
    [0.12, 81, 0.7],
  ];
  for (const [d, n, pan] of list) {
    m.duck(t + d, 0.2, 0.08);
    pop(m, t + d, hz(n), 0.3, pan);
  }
}

function boxPop(m: Mix, t: number): void {
  // Sparks spiral in on the spot first: a gathering, inhaled swell.
  m.duck(t, 0.45, 0.18);
  whoosh(m, ad(t - 0.17, t - 0.006, 0.16, t), sweep(t - 0.17, 5000, t, 1200), 2, { send: 0.1 });
  pop(m, t, hz(69), 0.42, 0, 0.2);
  thump(m, t, 0.7, 120, 50, 0.35);
  whoosh(m, ad(t, t + 0.008, 0.2, t + 0.5), sweep(t, 3800, t + 0.45, 450), 2.2, { send: 0.3, hold: false });
}

/** Compiled crates stream from the project node into the box. */
function stream(m: Mix, t0: number, t1: number): void {
  const n = 7;
  const notes = [86, 88, 90, 93, 95, 98, 93];
  for (let i = 0; i < n; i++) {
    const t = t0 + 0.02 + ((t1 - t0) * i) / n;
    ping(m, t, hz(notes[i]), 0.06, 0.12, { pan: -0.6 + i * 0.07, send: 0.2 });
  }
}

function gulp(m: Mix, t: number, i: number): void {
  m.duck(t, 0.12, 0.06);
  const f = hz([50, 52, 54, 57][i]);
  const v = m.voice(perc(t, 0.32, 0.003, 0.12), { send: 0.08 });
  if (v) {
    const lp = v.filter("lowpass", sweep(t, 1600, t + 0.09, 450), 5);
    v.osc("sine", [[t, f * 1.6], [t + 0.028, f * 0.85, "exp"], [t + 0.09, f, "exp"]], 1, lp);
    v.osc("triangle", sweep(t, f * 3.2, t + 0.028, f * 1.7), 0.25, lp);
  }
}

/** The box charges, then sends restored artifacts in three bursts. */
function restore(m: Mix, charge: number, bursts: number[], done: number, twinkle: number): void {
  const c = m.voice(ad(charge, bursts[0] - 0.004, 0.14, bursts[0] + 0.01), { send: 0.2, hold: true });
  if (c) {
    const lp = c.filter("lowpass", sweep(charge, 400, bursts[0], 4000), 6);
    c.osc("sawtooth", sweep(charge, hz(62), bursts[0], hz(74)), 0.6, lp);
  }
  const r = rng(505);
  bursts.forEach((b, k) => {
    const vel = [1, 0.7, 0.55][k];
    m.duck(b, 0.3 * vel, 0.2);
    const notes = [[86, 90, 93, 98, 102], [88, 93, 97], [90, 95, 98]][k];
    notes.forEach((n, i) => {
      ping(m, b + (i * X) / 2, hz(n), (0.15 - i * 0.02) * vel, 0.4, { pan: 0.15 + i * 0.12, send: 0.3 });
    });
    whoosh(m, ad(b, b + 0.1, 0.15 * vel, b + 0.45), sweep(b, 1200, b + 0.35, 5200), 1.5, {
      pan: line(b, 0, b + 0.4, 0.65),
      send: 0.25,
    });
    for (let i = 0; i < 4; i++) {
      const ti = b + 0.03 + r() * 0.4;
      const f = 3500 + r() * 4500;
      const pan = 0.3 + r() * 0.55;
      const v = m.voice(perc(ti, (0.03 + 0.02 * r()) * vel, 0.0005, 0.06), { pan, send: 0.35 });
      if (v) v.osc("sine", f);
    }
  });
  // Arrivals: the counters tick up as artifacts land, ending on b3.5.
  const windows = [
    [bursts[0] + 0.26, bursts[1] + 0.2, 6],
    [bursts[1] + 0.24, bursts[2] + 0.17, 4],
    [bursts[2] + 0.2, done - 0.012, 2],
  ];
  let k = 0;
  for (const [a, b, n] of windows) {
    for (let i = 0; i < n; i++) {
      const t = a + (b - a) * outCubicInv(i / n);
      k++;
      tick(m, t, 2000 + k * 70, 0.1, i % 2 ? 0.5 : 0.72);
    }
  }
  // The last hit lands: a chime.
  tick(m, done, 3200, 0.2, 0.6);
  ping(m, done, hz(81), 0.16, 0.45, { pan: 0.5, send: 0.3 });
  ping(m, done + 0.004, hz(86), 0.12, 0.45, { pan: 0.72, send: 0.3 });
  // s4 T_TWINKLE: Mr Boxington's monocle twinkles 30 ms after the lock.
  ding(m, twinkle, hz(98), 0.035, 0, 0.35, 0.35);
}

/** Whip pan: out to the left into the cut, in from the right after it. */
function whip(m: Mix, t: number): void {
  m.duck(t, 0.5, 0.12);
  const out = swell(t - 0.14, t - 0.09, 0.05, t - 0.004, 0.45, t + 0.012);
  whoosh(m, out, sweep(t - 0.14, 900, t, 4800), 0.9, { pan: line(t - 0.14, 0, t, -0.85) }, "white");
  const inn = ad(t - 0.01, t + 0.004, 0.42, t + 0.16);
  whoosh(m, inn, sweep(t - 0.01, 4800, t + 0.14, 900), 0.9, { pan: line(t - 0.01, 0.85, t + 0.14, 0), send: 0.1 }, "white");
  thump(m, t, 0.4, 90, 45, 0.18);
}

export const part: Part = {
  cues(m, s) {
    nodes(m, s.start);
    boxPop(m, s.at(T_POP));
    stream(m, s.at(T_POP), s.at(T_POP) + 0.33);
    GULPS.forEach((t, i) => gulp(m, s.at(t), i));
    restore(m, s.at(T_CHARGE), BURSTS.map((t) => s.at(t)), s.at(T_DONE), s.at(T_TWINKLE));
    whip(m, s.end);
  },
  drums: (m, s) => grooveBar(m, s.start, [0, 8, 10], [4, 12], [7, 15]),
  bass: (m, s) => bassBar(m, s.start, 31, 0.9), // G
  pads: (m, s) => pad(m, s.start, s.end, [43, 50, 54, 59, 62], 0.07, 1500),
};
