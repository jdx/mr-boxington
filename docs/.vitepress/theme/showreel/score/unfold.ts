// The fold: ignition, the pens, the flood, four folds, the lid, and the
// tape. No drums, only the build.

import type { Section } from "../bible";
import { lerp, progress, rng } from "../math";
import {
  FOLDS,
  LID_HANG,
  penT,
  T_CLOSE,
  T_CROUCH,
  T_SLAM,
  T_TAPE0,
  T_TAPE1,
  TAPE_KNEE,
  TAPE_PULL,
} from "../scenes/s1-unfold";
import type { Part } from ".";
import { ad, glide, hold, hz, line, type Mix, perc, type Pt, swell, sweep, X } from "./mix";
import { knock, pad, ping, thump, tick, whoosh } from "./sounds";

function ignition(m: Mix, t: number): void {
  // One small point of light: quiet (about -19.5 LUFS over its first 100
  // ms, level with the pen draw that follows), so the lid, the slams, and
  // the resolve stay the big moments and the fold builds from here.
  const v = m.voice(perc(t, 0.055, 0.001, 0.8), { send: 0.4 });
  if (v) {
    v.osc("sine", sweep(t, hz(84), t + 0.07, hz(86)));
    v.osc("sine", hz(98), perc(t, 0.2, 0.0005, 0.25));
    v.noise("white", perc(t, 0.8, 0.0003, 0.018), v.filter("bandpass", 5200, 1.2), 1, t + 0.03);
  }
  // A warm bloom that swells under the spark as the point grows.
  const g = m.voice(ad(t, t + 0.09, 0.058, t + 0.6), { hold: true });
  if (g) {
    g.osc("sine", hz(50));
    g.osc("sine", hz(57), 0.5);
  }
}

/**
 * The pens leave on the first 32nd and speed up (s1 penT: an arc-length
 * power of 1.45), so the corner glints crowd together toward the close. The
 * shimmer's band and pitch step up at each corner.
 */
function penDraw(m: Mix, s: Section): void {
  const t0 = s.start;
  const t1 = s.at(T_CLOSE);
  const launch = s.at(penT(0));
  const corner = (k: number) => s.at(penT(k));
  const band: Pt[] = [[t0, 2600]];
  const pitch: Pt[] = [[t0, hz(88)]];
  const pan: Pt[] = [[t0, 0]];
  const steps = [88, 90, 93, 95, 97, 98, 100];
  for (let k = 1; k <= 7; k++) {
    const t = corner(k);
    band.push([t, 2600 + 700 * k, "exp"]);
    pitch.push([t - 0.004, hz(steps[k - 1]), "exp"], [t, hz(steps[Math.min(6, k)]), "exp"]);
    pan.push([t, 0.5 * Math.sin(k * 2.1)]);
  }
  const v = m.voice(hold(t0, launch - t0, 0.09, t1 - 0.01, 0.28, 0.11), { pan, send: 0.3, hold: true });
  if (v) {
    v.noise("white", 0.9, v.filter("bandpass", band, 5));
    const s = v.osc("sine", pitch, 0.16);
    v.lfo("sine", 19, 16, s.frequency);
  }
  const r = rng(101);
  for (let k = 1; k <= 6; k++) {
    const t = corner(k);
    ping(m, t, hz(steps[k - 1] + 12), 0.035 + 0.01 * k, 0.12, { pan: (r() * 2 - 1) * 0.5, send: 0.3 });
  }
}

function flood(m: Mix, t: number): void {
  // The outline closes: a tick and a ping, then the flood whoosh.
  tick(m, t, 2600, 0.22, 0.1, 0.2);
  ping(m, t, hz(86), 0.12, 0.5, { send: 0.4 });
  const env: Pt[] = [[t, 0], [t + 0.08, 0.3], [t + 0.3, 0.14, "exp"], [t + 0.6, 0.0001, "exp"], [t + 0.604, 0]];
  const band: Pt[] = [[t, 350], [t + 0.14, 3000, "exp"], [t + 0.55, 600, "exp"]];
  whoosh(m, env, band, 0.8, { pan: line(t, -0.25, t + 0.5, 0.25), send: 0.25 });
}

/**
 * The walls wind up together on b1.75 (s1 T_CROUCH) as every crease flares
 * (s1 drawCreases): a scored-card scratch on the flare, well under the fold
 * clacks, then a short inhale into the first fold.
 */
function crouch(m: Mix, t: number, t1: number): void {
  tick(m, t, 3400, 0.18, 0, 0.15);
  const s = m.voice(perc(t, 0.06, 0.0006, 0.025), { pan: 0.1, send: 0.12 });
  if (s) s.noise("white", 1, s.filter("bandpass", sweep(t, 7000, t + 0.025, 3500), 1.6), 1, t + 0.035);
  whoosh(m, swell(t, t + 0.04, 0.01, t1 - 0.006, 0.07, t1 + 0.01), sweep(t, 500, t1, 1600), 1.4, { send: 0.12 });
}

function fold(m: Mix, t: number, i: number): void {
  m.duck(t, 0.15, 0.08);
  // A3 B3 C#4 D4: each wall rises a scale step.
  knock(m, t, hz([57, 59, 61, 62][i]), 0.5, [-0.35, 0.35, -0.18, 0.18][i], 0.14);
}

/**
 * s1 LID: the lid hangs at -25 degrees from LID_HANG, then whips over on a
 * Hermite from rest to 2700 degrees a second, most of it in the last 60 ms.
 */
function lid(m: Mix, t: number, hang: number): void {
  whoosh(m, swell(hang, t - 0.06, 0.012, t - 0.004, 0.13, t + 0.01), sweep(hang, 350, t, 1700), 1.3, {
    send: 0.08,
    pan: line(hang, 0.1, t, -0.05),
  });
  // The lid slams shut.
  m.duck(t, 0.3, 0.12);
  thump(m, t, 0.85, 140, 50, 0.4);
  const v = m.voice(perc(t, 0.45, 0.0008, 0.12), { send: 0.18 });
  if (v) v.noise("white", 1, v.filter("lowpass", 1800, 0), 1, t + 0.15);
  whoosh(m, ad(t, t + 0.012, 0.16, t + 0.3), 500, 0.8, { send: 0.1, hold: false });
  // The lid hops twice off the rim before it settles.
  knock(m, t + 0.066, hz(50), 0.16, 0.05, 0.06);
  knock(m, t + 0.1, hz(50), 0.07, -0.05, 0.06);
}

/**
 * s1 tapeAt: the tape pulls across the lid on an ease in and out
 * (TAPE_PULL) that comes to rest at the corner at TAPE_KNEE of the time,
 * then presses down the side on an ease out and seats on b3.75. The rip
 * follows the tape's speed: it swells across the lid, stalls at the corner,
 * and tears again down the side.
 */
function tape(m: Mix, t0: number, t1: number): void {
  const pull = TAPE_PULL;
  const turn = t0 + TAPE_KNEE * (t1 - t0);
  // Speed across the lid, sampled from the scene's own curve.
  const speed = (t: number) => pull(progress(t0, turn, t + 0.002)) - pull(progress(t0, turn, t - 0.002));
  let top = 0;
  for (let i = 1; i < 16; i++) top = Math.max(top, speed(lerp(t0, turn, i / 16)));
  const env: Pt[] = [[t0, 0]];
  const rate: Pt[] = [[t0, 0.5]];
  for (let i = 1; i <= 12; i++) {
    const t = lerp(t0, turn, i / 12);
    const s = i === 12 ? 0 : speed(t) / top;
    env.push([t, 0.02 + 0.42 * s ** 0.7]);
    rate.push([t, 0.5 + 2.2 * s]);
  }
  // Down the side: torn off at full speed, slowing on the ease out.
  env.push([turn + 0.004, 0.48], [turn + 0.03, 0.34]);
  env.push([t1 - 0.012, 0.055, "exp"], [t1, 0.0001, "exp"], [t1 + 0.004, 0]);
  rate.push([turn + 0.004, 2.6, "set"], [t1, 0.6, "exp"]);
  const pan: Pt[] = [[t0, -0.45], [turn, 0.3], [t1, 0.45]];
  const v = m.voice(env, { pan, send: 0.12, hold: true });
  if (v) {
    const band: Pt[] = [
      [t0, 1500],
      [turn - 0.03, 3000, "exp"],
      [turn, 1700, "exp"],
      [turn + 0.01, 3200, "exp"],
      [t1, 1400, "exp"],
    ];
    const am = v.vca(0.6, v.filter("bandpass", band, 0.8));
    // Adhesive letting go in a sawtooth rhythm that runs with the speed.
    v.lfo("sawtooth", rate.map(([t, r, k]): Pt => [t, 50 * r, k]), 0.4, am.gain);
    v.noise("crackle", 3, am, rate);
    v.noise("white", 1.2, am);
  }
  // The tape creases over the edge, then seats with a pat.
  tick(m, turn, 1900, 0.18, 0.3, 0.08);
  thump(m, t1, 0.18, 240, 120, 0.08, { pan: 0.4 });
}

export const part: Part = {
  cues(m, s) {
    ignition(m, s.start);
    penDraw(m, s);
    flood(m, s.at(T_CLOSE));
    crouch(m, s.at(T_CROUCH), s.at(FOLDS[0]));
    FOLDS.forEach((t, i) => fold(m, s.at(t), i));
    lid(m, s.at(T_SLAM), s.at(LID_HANG));
    tape(m, s.at(T_TAPE0), s.at(T_TAPE1));
  },
  pads(m, s) {
    // The fold builds on A sus: the pad swells and opens as the box
    // assembles, then falls away over the last sixteenth so the drop lands
    // on contrast.
    pad(m, s.beat(1), s.end - X, [45, 52, 57, 59, 62], 0.1, glide(s.beat(1), s.end, 300, 2600, (u) => u * u), 0.9, X);
  },
};
