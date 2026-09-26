// First build fills the store: half-time. An amber crackle while rustc
// compiles syn, a pop as its carton comes out, and a gulp as it drops in
// under the lid. The cutaway tears open and the camera pushes in on a
// swell; the carton knocks onto the shelf, its tag flicks out, the inputs
// tick onto the wall, the paths' letters click over, and the key stamps on
// with a thunk. Backing out, the rest of the plan drops in with quick
// gulps, the lid clicks down four times with the mascot's, and the tape
// zips on. No fanfare: the store was empty, so nothing hit.

import type { Section } from "../bible";
import { lerp, progress, rng } from "../math";
import {
  FLIPS,
  LID_STEPS,
  T_CHIPS,
  T_GULP,
  T_HASH,
  T_NAMED,
  T_OPEN0,
  T_PATHS,
  T_PULL0,
  T_PULL1,
  T_PUSH0,
  T_PUSH1,
  T_RING0,
  T_RING1,
  T_SHELF,
  T_STAMP,
  T_TAG,
  T_TAPE0,
  T_TAPE1,
  T_THROW,
} from "../scenes/first-build";
import { TAPE_EASE } from "../scenes/first-build-kit";
import type { Part } from ".";
import { A, BM, bassBars, CHORD, chordBars, drumBars, G, HALF } from "./grooves";
import { ad, hz, line, type Mix, perc, type Pt, swell, sweep } from "./mix";
import { flick, knock, pop, stamp, thump, tick, whoosh } from "./sounds";

/** Mr Boxington stands left of the middle, the terminal and the plan right of it. */
const BOX = -0.45;
const PLAN = 0.2;

/**
 * rustc compiling: a crackle that quickens as the ring speeds up, over a
 * low hum that climbs, cut off by the pop.
 */
function crackle(m: Mix, t0: number, t1: number): void {
  const env: Pt[] = [
    [t0, 0],
    [t0 + 0.08, 0.05],
    [t1 - 0.05, 0.16, "exp"],
    [t1, 0.0001, "exp"],
    [t1 + 0.004, 0],
  ];
  const v = m.voice(env, { pan: PLAN, send: 0.15, hold: true });
  if (v) {
    v.noise("crackle", 3, v.filter("bandpass", sweep(t0, 1800, t1, 4200), 1.2), sweep(t0, 0.7, t1, 1.8));
    v.osc("sawtooth", sweep(t0, hz(38), t1, hz(50)), 0.12, v.filter("lowpass", sweep(t0, 300, t1, 1400), 4));
  }
}

/** The carton pops out of the chip, and its throw whooshes left into the box. */
function throwIn(m: Mix, pop0: number, t0: number, t1: number): void {
  m.duck(pop0, 0.2, 0.1);
  pop(m, pop0, hz(79), 0.34, PLAN, 0.25);
  thump(m, pop0, 0.25, 260, 120, 0.1, { pan: PLAN });
  whoosh(m, ad(t0, lerp(t0, t1, 0.4), 0.14, t1 + 0.05), sweep(t0, 2600, t1, 900), 1.6, { pan: line(t0, PLAN, t1, BOX), send: 0.2 });
}

/** A swallow: a sine that drops and comes back, through a closing filter (s4's gulp). */
export function gulp(m: Mix, t: number, f: number, vel: number, pan: number): void {
  m.duck(t, 0.12 * vel, 0.06);
  const v = m.voice(perc(t, 0.32 * vel, 0.003, 0.12), { pan, send: 0.08 });
  if (!v) return;
  const lp = v.filter("lowpass", sweep(t, 1600, t + 0.09, 450), 5);
  v.osc("sine", [[t, f * 1.6], [t + 0.028, f * 0.85, "exp"], [t + 0.09, f, "exp"]], 1, lp);
  v.osc("triangle", sweep(t, f * 3.2, t + 0.028, f * 1.7), 0.25, lp);
}

/** The cutaway: a papery tear as the hole opens, a swell into the push, a soft landing. */
function cutaway(m: Mix, open: number, p0: number, p1: number): void {
  const v = m.voice(perc(open, 0.1, 0.004, 0.2), { pan: BOX, send: 0.15 });
  if (v) v.noise("crackle", 2, v.filter("bandpass", sweep(open, 900, open + 0.2, 3800), 1), sweep(open, 1.4, open + 0.2, 0.6));
  whoosh(m, swell(p0, p0 + 0.1, 0.02, p1 - 0.02, 0.2, p1 + 0.08), sweep(p0, 400, p1, 2600), 1.2, { send: 0.3 });
  thump(m, p1, 0.3, 150, 60, 0.3);
}

/** The key stamps on: the inputs zip into the tag, then the thunk. */
function stampOn(m: Mix, zip: number, t: number): void {
  whoosh(m, swell(zip, zip + 0.04, 0.02, t - 0.004, 0.12, t + 0.01), sweep(zip, 1500, t, 6500), 2.5, { pan: 0.1, send: 0.2 }, "white");
  m.duck(t, 0.35, 0.15);
  stamp(m, t, 1, -0.1);
}

/** A cardboard lid dropping a step: a click with a hollow knock under it. */
export function lidClick(m: Mix, t: number, i: number, pan: number): void {
  tick(m, t, 1500 + 180 * i, 0.22, pan, 0.08);
  knock(m, t, hz(55 + 2 * i), 0.18, pan, 0.06);
}

/**
 * Tape laid over the lid and down the front, its rip following the tape's
 * own pull (TAPE_EASE) from `t0` to `t1`, then a pat as it seats.
 */
export function tapeZip(m: Mix, t0: number, t1: number, pan: number, vel = 1): void {
  const at = (t: number) => TAPE_EASE(progress(t0, t1, t));
  const speed = (t: number) => at(t + 0.004) - at(t - 0.004);
  let top = 0;
  for (let i = 1; i < 16; i++) top = Math.max(top, speed(lerp(t0, t1, i / 16)));
  const env: Pt[] = [[t0, 0]];
  const rate: Pt[] = [[t0, 0.5]];
  for (let i = 1; i < 12; i++) {
    const t = lerp(t0, t1, i / 12);
    const k = speed(t) / top;
    env.push([t, vel * (0.02 + 0.4 * k ** 0.7)]);
    rate.push([t, 0.5 + 2.2 * k]);
  }
  env.push([t1, 0.0001, "exp"], [t1 + 0.004, 0]);
  rate.push([t1, 0.6, "exp"]);
  const v = m.voice(env, { pan: line(t0, pan - 0.2, t1, pan + 0.15), send: 0.12, hold: true });
  if (v) {
    const am = v.vca(0.6, v.filter("bandpass", sweep(t0, 1600, t1, 2800), 0.8));
    v.lfo("sawtooth", rate.map(([t, r, k]): Pt => [t, 50 * r, k]), 0.4, am.gain);
    v.noise("crackle", 3, am, rate);
    v.noise("white", 1.1, am);
  }
  thump(m, t1, 0.16 * vel, 240, 120, 0.08, { pan });
}

function cues(m: Mix, s: Section): void {
  crackle(m, s.at(T_RING0), s.at(T_RING1));
  throwIn(m, s.at(T_RING1), s.at(T_THROW), s.at(T_GULP));
  gulp(m, s.at(T_GULP) - 0.03, hz(50), 1, BOX);
  thump(m, s.at(T_GULP), 0.3, 120, 55, 0.18, { pan: BOX });
  cutaway(m, s.at(T_OPEN0), s.at(T_PUSH0), s.at(T_PUSH1));
  knock(m, s.at(T_SHELF), hz(52), 0.3, -0.2, 0.12);
  flick(m, s.at(T_TAG), -0.1, 0.12);
  // The inputs tick onto the wall, climbing.
  [...T_PATHS, ...T_CHIPS].forEach((t, i) => tick(m, s.at(t), 1900 + 140 * i, 0.3, 0.25, 0.12));
  // The letters click over, split-flap fashion.
  const r = rng(606);
  FLIPS.forEach((times, k) =>
    times.forEach((t) => {
      const at = s.at(t) + 0.035;
      const v = m.voice(perc(at, 0.05 + 0.03 * r(), 0.0004, 0.012), { pan: 0.15 + 0.1 * k, send: 0.05 });
      if (v) v.noise("white", 1, v.filter("bandpass", 3200 + 1600 * r(), 3), 1, at + 0.03);
    }),
  );
  stampOn(m, s.at(T_HASH), s.at(T_STAMP));
  // Backing out.
  whoosh(m, ad(s.at(T_PULL0), s.at(T_PULL0) + 0.05, 0.16, s.at(T_PULL1)), sweep(s.at(T_PULL0), 2800, s.at(T_PULL1), 500), 1.2, { send: 0.2 });
  // The rest of the plan drops in: a quick gulp for each named crate, climbing.
  T_NAMED.forEach((t, i) => gulp(m, s.at(t), hz(52 + 2 * i), 0.55, BOX + 0.1));
  LID_STEPS.forEach((t, i) => lidClick(m, t, i, BOX));
  tapeZip(m, s.at(T_TAPE0), s.at(T_TAPE1), BOX);
}

export const part: Part = {
  cues,
  drums: (m, s) => drumBars(m, s, [HALF]),
  bass: (m, s) => bassBars(m, s, [BM, G, A]),
  pads: (m, s) => chordBars(m, s, [CHORD.Bm, CHORD.G, CHORD.A]),
};
