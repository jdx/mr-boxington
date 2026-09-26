// Six builds at once. The pane cracks into six job cards, which type
// together over six little drum loops out of step with each other (each
// card hops on its own loop's hits, six-builds loopHits). The permit rail
// falls and slams down on b1, and from that hit the loops are one groove,
// the full one. Every compilation that takes a permit clicks, panned to its
// slot; hits leaving the monocle glint in the treble; the one `syn`
// compilation two jobs share lands with a chime. The score takes no facts,
// so nothing here follows the rail's tip into the peaks, which only happens
// with them: under it the groove simply carries on.

import type { Section } from "../bible";
import { hash } from "../math";
import {
  DOCKS,
  HITS,
  LOOP,
  loopHits,
  T_DROP,
  T_HOP,
  T_MERGE,
  T_SLAM,
  T_SPLIT,
  T_SYN_CHECK,
  T_SYN_CLIPPY,
  T_SYN_DOCK,
  T_SYN_DONE,
  T_TYPE,
  T_TYPED,
  TILES,
} from "../scenes/six-builds";
import type { Part } from ".";
import { A, BM, CHORD, chordBars, FULL, G } from "./grooves";
import { ad, hz, line, type Mix, perc, sweep, X } from "./mix";
import { bassBar, bassRun, blip, clap, ding, grooveBar, hat, kick, knock, ping, pop, stamp, thump, tick, whoosh } from "./sounds";

/** Each job's loop: its drum's pitch and where it sits in the stereo field. */
const LOOPS = [
  { f: hz(43), pan: -0.55 },
  { f: hz(47), pan: 0 },
  { f: hz(50), pan: 0.55 },
  { f: hz(41), pan: -0.4 },
  { f: hz(45), pan: 0.15 },
  { f: hz(52), pan: 0.45 },
] as const;

/**
 * The crack: a dry snap across the pane as its seams light, then a small
 * pop for each tile leaving for its card, and the strawberry's hop.
 */
function crack(m: Mix, s: Section): void {
  const t = s.at(T_SPLIT);
  const c = m.voice(perc(t, 0.2, 0.001, 0.07), { pan: 0.45, send: 0.1 });
  if (c) c.noise("crackle", 3, c.filter("bandpass", 2400, 1.2));
  tick(m, t, 3200, 0.14, 0.45);
  TILES.forEach((at, j) => pop(m, s.at(at), hz(79 + [0, 2, 4, 5, 7, 9][j]), 0.07, LOOPS[j].pan * 0.6 + 0.3, 0.1));
  pop(m, s.at(T_HOP), hz(88), 0.16, -0.45, 0.2);
  tick(m, s.at(T_HOP) + 0.01, 3600, 0.08, -0.45);
}

/**
 * All six commands typing at once: a flurry of keys, each job's in its own
 * place in the field.
 */
function typing(m: Mix, s: Section): void {
  const t0 = s.at(T_TYPE);
  const t1 = s.at(T_TYPED);
  for (let i = 0; i < 22; i++) {
    const j = i % 6;
    const t = t0 + (t1 - t0) * ((i + hash(i, 5) * 0.8) / 22);
    tick(m, t, 1900 + 500 * hash(i, 7), 0.05 + 0.03 * hash(i, 9), LOOPS[j].pan);
  }
}

/**
 * Six loops out of step: each job's own little kit, a pitched knock on its
 * hit and a hat half a loop later, from the first keystroke to the slam.
 */
function loops(m: Mix, s: Section): void {
  LOOPS.forEach(({ f, pan }, j) => {
    for (const at of loopHits(j)) {
      const t = s.at(at);
      knock(m, t, f, 0.22, pan, 0.06);
      const v = m.voice(perc(t, 0.16, 0.002, 0.06), { pan, send: 0.04 });
      if (v) v.osc("sine", [[t, f * 3], [t + 0.03, f * 1.2, "exp"]]);
      const h = t + LOOP / 2;
      if (h < s.at(T_SLAM) - 0.01) tick(m, h, 5200 + 300 * j, 0.06, pan);
    }
  });
}

/** The rail falls, a rising whistle, and slams down on b1: the loops stop, the groove takes over. */
function slam(m: Mix, s: Section): void {
  const t0 = s.at(T_DROP);
  const t = s.at(T_SLAM);
  whoosh(m, ad(t0, t - 0.01, 0.16, t + 0.02), sweep(t0, 700, t, 3200), 1.6, { pan: line(t0, 0.4, t, 0), send: 0.1 }, "white");
  m.duck(t, 0.55, 0.2);
  stamp(m, t, 1.1, 0);
  thump(m, t, 0.7, 120, 42, 0.4);
  const r = m.voice(perc(t, 0.14, 0.001, 0.3), { send: 0.3 });
  if (r) r.noise("white", 1, r.filter("bandpass", sweep(t, 3000, t + 0.3, 900), 2.5));
}

/**
 * A click for every permit taken, panned to its slot on the rail; in the
 * first rush they come a few milliseconds apart, so the closest merge.
 */
function permits(m: Mix, s: Section): void {
  let last = -Infinity;
  DOCKS.forEach(({ at, x }, i) => {
    const t = s.at(at);
    if (t - last < 0.022) return;
    last = t;
    tick(m, t, 1300 + 900 * ((x - 696) / 1168), 0.05 + 0.03 * hash(i, 3), ((x - 696) / 1168) * 1.2 - 0.6, 0.03);
  });
}

/** Hits leave the monocle, left of centre: a small high blip each, a glint in the treble. */
function hits(m: Mix, s: Section): void {
  HITS.forEach((at, i) => {
    const t = s.at(at);
    blip(m, t, hz(96 + [0, 3, 5, 7][i % 4]), 0.05);
    ping(m, t + 0.004, hz(100 + [0, 2, 4][i % 3]), 0.03, 0.12, { pan: -0.35, send: 0.25 });
  });
}

/**
 * check's `syn` drops into its permit with a knock; clippy's, the same
 * compilation, arrives with a softer one and waits; the one result lands for
 * both with a chime, one note for each.
 */
function syn(m: Mix, s: Section): void {
  pop(m, s.at(T_SYN_CHECK), hz(62), 0.14, -0.1, 0.1);
  knock(m, s.at(T_SYN_DOCK), hz(50), 0.3, -0.05, 0.12);
  pop(m, s.at(T_SYN_CLIPPY), hz(62), 0.1, 0.1, 0.1);
  const t = s.at(T_SYN_DONE);
  m.duck(t, 0.3, 0.3);
  ding(m, t, hz(86), 0.09, -0.15, 1.2, 0.4);
  ding(m, t + 0.03, hz(93), 0.08, 0.15, 1.2, 0.4);
  tick(m, t, 3000, 0.12, 0);
}

/** The six cards fly together into the two runners. */
function merge(m: Mix, s: Section): void {
  const t0 = s.at(T_MERGE);
  const t1 = s.end;
  whoosh(m, ad(t0, t1 - 0.05, 0.12, t1 + 0.05), sweep(t0, 1800, t1, 600), 1.2, { pan: line(t0, 0.2, t1, 0.4), send: 0.08 });
  thump(m, t1 - 0.02, 0.18, 150, 70, 0.14, { pan: 0.4 });
}

/**
 * The groove, from the slam: the first bar's kick, clap and hats only from
 * b1 (the loops have the bar before it), then the full groove.
 */
function drums(m: Mix, s: Section): void {
  const t0 = s.bar(0);
  for (const k of [4, 8, 10]) {
    m.kick(t0 + k * X);
    kick(m, t0 + k * X, k === 10 ? 0.7 : 1);
  }
  for (const k of [4, 12]) clap(m, t0 + k * X, 1);
  for (const k of [6, 10, 14]) hat(m, t0 + k * X, 1);
  for (const k of [7, 15]) hat(m, t0 + k * X, 0.4);
  for (let b = 1; b < s.bars; b++) grooveBar(m, s.bar(b), ...FULL);
}

/** The bass enters with the slam on B, then G and A. */
function bass(m: Mix, s: Section): void {
  const t0 = s.bar(0);
  const [root, grit] = BM;
  bassRun(
    m,
    [
      [t0 + 4 * X, t0 + 8 * X, root, 1],
      [t0 + 8 * X, t0 + 14 * X, root, 1],
      [t0 + 14 * X, t0 + 16 * X, root + 12, 0.7],
    ],
    0.08,
    360,
    grit,
  );
  [G, A].forEach(([note, g], i) => bassBar(m, s.bar(i + 1), note, g));
}

export const part: Part = {
  cues(m, s) {
    crack(m, s);
    typing(m, s);
    loops(m, s);
    slam(m, s);
    permits(m, s);
    hits(m, s);
    syn(m, s);
    merge(m, s);
  },
  drums,
  bass,
  pads: (m, s) => chordBars(m, s, [CHORD.Bm, CHORD.G, CHORD.A]),
};
