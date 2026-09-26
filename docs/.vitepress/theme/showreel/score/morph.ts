// Liquid morph, the breakdown: the discs turn to jelly and merge, the blob
// breathes and sets into the silhouette, and a riser builds into a sixteenth
// of silence before the end card. The drums drop out to a filtered
// heartbeat, and the bass and pads fall back.

import { BEAT, type Section, sec } from "../bible";
import { ARM, FIRST, INHALE, LAND, MERGE, MORPH, PULL, RING, SECOND, SNAP } from "../scenes/s7-morph";
import type { Part } from ".";
import { ad, glide, hz, type Mix, perc, type Pt, swell, sweep, X } from "./mix";
import { bassRun, bloop, kick, pad, thump, whoosh } from "./sounds";

/** The breath before the resolve: nothing sounds from here to the end card's downbeat. */
export const GAP = sec("morph").end - X;

function jelly(m: Mix, t: number): void {
  // The shared splat: a wet slap on top, so the breakdown opens with some
  // top end over the wobble's body.
  const s = m.voice(perc(t, 0.16, 0.001, 0.07), { send: 0.25 });
  if (s) {
    s.noise("pink", 1, s.filter("bandpass", sweep(t, 3200, t + 0.06, 1300), 1.3));
    s.osc("sine", sweep(t, 2400, t + 0.03, 900), perc(t, 0.3, 0.001, 0.03));
  }
  const v = m.voice(ad(t, t + 0.005, 0.1, t + 0.55), { send: 0.3 });
  if (!v) return;
  // The wobble lives in a resonant filter swinging across a saw's overtones
  // (twice per squash, as s7 rings the discs at ~3.5 Hz), so it reads on
  // small speakers; a dip at 330 Hz keeps the body from booming.
  const dip = v.filter("peaking", 330, 1);
  dip.gain.value = -6;
  const lp = v.filter("lowpass", 1000, 8, dip);
  v.lfo("sine", 7, sweep(t, 600, t + 0.55, 30), lp.frequency);
  for (const [type, level] of [["sine", 0.8], ["sawtooth", 0.22]] as const) {
    const o = v.osc(type, hz(55), level, lp);
    v.lfo("sine", 7, sweep(t, 30, t + 0.55, 1), o.frequency);
  }
}

/**
 * s7-morph's anchors: the pull on b0.5; the first pair reaches the core on
 * b1; the core throws out arms (ARM: 0.22 s out, full length 0.1223 s after
 * the bridge) and yanks the second pair in on b1.5; the sides wind up and
 * slam in on b2, throwing up a drop whose thread snaps on b2.25 and which
 * plops back on b2.5 as the blob inhales; the squat on b2.75 that the morph
 * springs out of; and the landing on b3.5 with one rebound. Every tail is
 * gone before the breath at b3.75.
 */
function liquid(
  m: Mix,
  pull: number,
  top: number,
  bottom: number,
  merge: number,
  snap: number,
  inhale: number,
  morph: number,
  land: number,
): void {
  // The discs drift out, then the pull.
  whoosh(m, swell(pull - 0.2, pull, 0.04, top - 0.01, 0.08, top + 0.2), sweep(pull - 0.2, 500, top, 1400), 3, {
    send: 0.35,
  });
  // Bridges snap on the beat: the first pair, the second, then the sides with the merge.
  bloop(m, top, hz(74), 0.13, -0.35);
  bloop(m, top + 0.018, hz(78), 0.11, 0.35);
  const arms = bottom + ARM.lead - ARM.out;
  // The core throws out arms and yanks the second pair in.
  whoosh(m, ad(arms, bottom - 0.004, 0.07, bottom + 0.05), sweep(arms, 700, bottom, 2400), 2.5, { send: 0.2 });
  bloop(m, bottom, hz(71), 0.13, -0.3);
  bloop(m, bottom + 0.018, hz(76), 0.11, 0.3);
  const rush = merge - 0.14;
  // The sides rush in and slam: the merge.
  m.duck(merge, 0.3, 0.2);
  const rushEnv = swell(rush, rush + 0.05, 0.02, merge - 0.004, 0.12, merge + 0.02);
  whoosh(m, rushEnv, sweep(rush, 400, merge, 1500), 1.6, { send: 0.15 });
  bloop(m, merge, hz(62), 0.26, 0, 0.4);
  bloop(m, merge + 0.012, hz(69), 0.12, -0.4);
  bloop(m, merge + 0.024, hz(74), 0.1, 0.4);
  const g = m.voice(perc(merge, 0.32, 0.004, 0.4), { send: 0.2 });
  if (g) g.osc("sine", sweep(merge, 90, merge + 0.25, 45));
  // The drop rises out of the top on a thinning thread that snaps, then plops back in.
  // An elastic tink, bright enough to cut through the merge's low tail.
  m.duck(snap, 0.2, 0.08);
  const p = m.voice(perc(snap, 0.15, 0.0008, 0.08), { pan: 0.08, send: 0.25 });
  if (p) {
    p.osc("sine", sweep(snap, hz(86), snap + 0.025, hz(98)));
    p.osc("triangle", sweep(snap, hz(98), snap + 0.02, hz(105)), perc(snap, 0.3, 0.0005, 0.025));
    p.noise("white", perc(snap, 0.45, 0.0004, 0.008), p.filter("bandpass", 4500, 1.5), 1, snap + 0.02);
  }
  // The drop plops back in.
  bloop(m, inhale, hz(86), 0.12, -0.15, 0.16);
  // The blob breathes in on b2.5 and out into the morph.
  const breath: Pt[] = [[merge + 0.05, 0], [inhale, 0.06], [morph, 0.025], [land - 0.05, 0.0001, "exp"], [land - 0.046, 0]];
  whoosh(m, breath, [[merge, 500], [inhale, 1300, "exp"], [land - 0.05, 600, "exp"]], 1.2, { send: 0.3 });
  // The morph: a liquid squelch rising into the silhouette.
  const v = m.voice(ad(morph, land - 0.03, 0.09, land + 0.09), { send: 0.25, hold: true });
  if (v) {
    const lp = v.filter("lowpass", [[morph, 300], [land, 2200, "exp"], [land + 0.09, 900, "exp"]], 12);
    const o = v.osc("triangle", sweep(morph, hz(50), land, hz(62)), 1, lp);
    v.lfo("sine", 4.2, [[morph, 0.5], [land, 14, "exp"], [land + 0.09, 2, "exp"]], o.frequency);
  }
  // It stops dead on b3.5 and rebounds once: s7 RING rings it as exp(-10 d)
  // sin(2 pi 4.2 d), whose first peak comes atan(2 pi 4.2 / 10) / (2 pi 4.2) later.
  const w = 2 * Math.PI * RING.f;
  const rebound = land + Math.atan(w / RING.decay) / w;
  m.duck(land, 0.25, 0.08);
  thump(m, land, 0.55, 130, 55, 0.09);
  bloop(m, land, hz(62), 0.14, 0, 0.08);
  bloop(m, rebound, hz(69), 0.05, 0, 0.04);
}

/**
 * The build into the end card: noise and rising fifths swell from b2 under a
 * filtered roll that doubles its rate every half bar (eighths, sixteenths,
 * thirty-seconds), all cutting dead on `gap` for a sixteenth of silence
 * before the downbeat.
 */
function riser(m: Mix, s: Section, t0: number, gap: number): void {
  const cut = (a: number, peak: number): Pt[] => [
    [t0, 0],
    [t0 + 0.25, a],
    [gap - 0.006, peak, "exp"],
    [gap, 0.0001, "exp"],
    [gap + 0.003, 0],
  ];
  const v = m.voice(cut(0.012, 0.2), { send: 0.05, hold: true });
  if (v) v.noise("white", 1, v.filter("bandpass", sweep(t0, 400, gap, 7000), 1.1));
  const f = m.voice(cut(0.02, 0.16), { send: 0.06, hold: true });
  if (f) {
    // Fifths rising an octave, A over E, into the D of the downbeat.
    const lp = f.filter("lowpass", sweep(t0, 400, gap, 5000), 3);
    for (const det of [-10, 10]) f.osc("sawtooth", sweep(t0, hz(45), gap, hz(57)), 0.5, lp, det);
    f.osc("sawtooth", sweep(t0, hz(52), gap, hz(64)), 0.35, lp);
  }
  const r = m.voice(cut(0.01, 0.2), { send: 0.04, hold: true, pan: 0.05 });
  if (r) {
    // The gate opens every eighth until b3, every sixteenth until b3.5,
    // then every 32nd. Each rate runs whole cycles, so a mid-sound entry can
    // start the gate on the next period boundary and stay on the grid.
    const am = r.vca(0.5, r.filter("bandpass", sweep(t0, 900, gap, 3600), 0.9));
    const period = (t: number) => (t < s.beat(3) ? 2 * X : t < s.beat(3.5) ? X : X / 2);
    const cycle = (t: number) => t0 + Math.ceil((t - t0) / period(t) - 1e-9) * period(t);
    r.lfo("square", [[t0, 2 / BEAT], [s.beat(3), 4 / BEAT, "set"], [s.beat(3.5), 8 / BEAT, "set"]], 0.5, am.gain, cycle);
    r.noise("white", 1, am);
  }
}

export const part: Part = {
  cues(m, s) {
    jelly(m, s.start);
    liquid(m, s.at(PULL), s.at(FIRST), s.at(SECOND), s.at(MERGE), s.at(SNAP), s.at(INHALE), s.at(MORPH), s.at(LAND));
    riser(m, s, s.beat(2), GAP);
  },
  drums(m, s) {
    // Drums out, only a lowpassed heartbeat under the breakdown.
    for (const t of [s.start, s.beat(2)]) {
      kick(m, t, 0.5, true);
    }
  },
  bass(m, s) {
    // Long, darker notes on G then A, gone before the breath.
    bassRun(m, [[s.start, s.beat(2) - 0.02, 31, 1]], 0.02, 220, 0);
    bassRun(m, [[s.beat(2), GAP - 0.03, 33, 1]], 0.02, 220, 0);
  },
  pads(m, s) {
    // An open Gmaj9, then A sus4 opening toward the breath.
    pad(m, s.start, s.beat(2), [43, 50, 59, 66, 69], 0.06, glide(s.start, s.beat(2), 900, 2000, (u) => u), 0.15, 0.3, 200);
    pad(m, s.beat(2), GAP - 0.03, [45, 52, 62, 64, 69], 0.06, glide(s.beat(2), GAP, 1200, 3200, (u) => u * u), 0.1, 0.024, 200);
  },
};
