// What first-build and same-checkout share: motion that lands exactly on its
// mark (so each section settles onto its handoff frame to the pixel), the big
// box's lid stepping down with the pixel mascot's, the tape laid over the
// lid, and the times the score reads off a build plan. Every function is a
// pure function of its arguments.

import { type BuildPlan, type Unit } from "../map";
import { clamp, cubicBezier, lerp, progress, TAU } from "../math";
import { glintAt, LID_FPS, LID_MAX, lidOffset, stepLid } from "../sprite";

/**
 * 0 before `at`, then up to 1 over `dur` seconds, overshooting by about
 * `over` of the way (a back-out curve) and landing exactly on 1: a pop or a
 * snap that is home, to the last bit, when it says it is.
 */
export function land(t: number, at: number, dur: number, over = 0.12): number {
  const p = progress(at, at + dur, t);
  if (p <= 0 || p >= 1) return p;
  // outBack with the overshoot solved for: s 1.70158 gives 10%.
  const s = 1.70158 * (over / 0.1);
  return 1 + (s + 1) * (p - 1) ** 3 + s * (p - 1) ** 2;
}

/**
 * A knock: a damped wobble starting at full swing on `at` and exactly 0
 * from `at + dur`, `freq` swings a second.
 */
export function jolt(t: number, at: number, dur: number, freq = 6): number {
  const p = progress(at, at + dur, t);
  if (p <= 0 || p >= 1) return 0;
  return Math.sin(TAU * freq * (t - at)) * (1 - p) ** 2;
}

/** A bump up from 0 and back to exactly 0: sin over [at, at + dur], eased. */
export function bump(t: number, at: number, dur: number): number {
  const p = progress(at, at + dur, t);
  return p <= 0 || p >= 1 ? 0 : Math.sin(Math.PI * p) ** 2;
}

/** Units finished at `time`, as buildAt counts them. */
export const doneAt = (units: readonly Unit[], time: number): number => units.filter((u) => u.at <= time).length;

/**
 * The global times the pixel mascot's lid steps down a pixel, from the
 * plan's start to `until`: lidAt's steps, found frame by frame, so the big
 * box and the score's lid clicks land on the frame the sprite changes.
 */
export function lidSteps(plan: BuildPlan, until: number): number[] {
  const out: number[] = [];
  let shown = LID_MAX;
  const frames = Math.floor((until - plan.start) * LID_FPS + 1e-9);
  for (let f = 0; f <= frames; f++) {
    const next = stepLid(shown, lidOffset(doneAt(plan.units, plan.start + f / LID_FPS), plan.total));
    if (next < shown) out.push(plan.start + f / LID_FPS);
    shown = next;
  }
  return out;
}

/** How long the big box's lid takes to drop one step, seconds. */
export const LID_DROP = 0.14;

/**
 * The big box's lid under a plan: LID_MAX, less one step for each of the
 * sprite's, each dropping in with a little bounce and home exactly LID_DROP
 * later.
 */
export function bouncyLid(steps: readonly number[], t: number): number {
  let lid = LID_MAX;
  for (const s of steps) lid -= land(t, s, LID_DROP, 0.25);
  return lid;
}

/**
 * The tape's pull, as s1 lays it: an ease in and out that crosses the lid
 * and then runs down the front, 0 at `t0` and exactly 1 at `t1`.
 */
export const TAPE_EASE = cubicBezier(0.45, 0, 0.2, 1);
export const tapeAt = (t: number, t0: number, t1: number): number => TAPE_EASE(progress(t0, t1, t));

/**
 * The sprite's glint sweeps under a plan: the global time each sweep's band
 * first shows, with the position it enters at (a hit mid-cycle joins a sweep
 * already under way). The score rings a ting on each.
 */
export function glintSweeps(plan: BuildPlan, until: number): { at: number; band: number }[] {
  const out: { at: number; band: number }[] = [];
  let last: number | null = null;
  // Sample every millisecond of the mascot's own clock: the band is a pure
  // function of whole milliseconds.
  const end = Math.min(until, plan.finish ?? until);
  for (let ms = 0; plan.start + ms / 1000 < end; ms++) {
    const t = plan.start + ms / 1000;
    let lastHit: number | null = null;
    for (const u of plan.units) if (u.outcome === "hit" && u.at <= t) lastHit = lastHit === null ? u.at : Math.max(lastHit, u.at);
    const band = glintAt(ms, lastHit === null ? null : Math.max(0, Math.floor((t - lastHit) * 1000)));
    if (band !== null && last === null) out.push({ at: t, band });
    last = band;
  }
  return out;
}

/** Evenly spaced units from `t0` to `t1` inclusive, all with one outcome. */
export function evenUnits(n: number, t0: number, t1: number, outcome: Unit["outcome"]): Unit[] {
  return Array.from({ length: n }, (_, i) => ({ at: n === 1 ? t0 : lerp(t0, t1, i / (n - 1)), outcome }));
}

/** A clamped 0..1 ramp from `a` to `b`. */
export const ramp = (a: number, b: number, t: number): number => clamp(progress(a, b, t));
