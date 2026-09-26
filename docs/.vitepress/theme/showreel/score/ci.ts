// CI: half-time. Mr Boxington leaps into his corner and lands; the remote
// falls in with a thud and its three backends pop in beside it; the Action
// line types. The push runner's outputs whoosh up and click onto the
// shelf; the read-only line locks with a click; the pull request's restores
// drop in with a sparkle each (a sound, not a glint: no mbx hit happens
// there), and its own compile thunks off the line. The whip pan into the
// chart ends the section.

import type { Section } from "../bible";
import {
  CHIPS,
  RESTORE_FLY,
  RESTORES,
  T_BACK,
  T_BOUNCE,
  T_DROP,
  T_LAND,
  T_LEAP,
  T_LOCK,
  T_REMOTE,
  T_THROW,
  T_USES,
  UPLOAD_FLY,
  UPLOADS,
} from "../scenes/ci";
import type { Part } from ".";
import { A, bassBars, CHORD, chordBars, D, drumBars, G, HALF } from "./grooves";
import { ad, hz, line, type Mix, perc, swell, sweep } from "./mix";
import { clink, knock, ping, pop, stamp, thump, tick, whoosh } from "./sounds";

/** Whip pan: out to the left into the cut, in from the right after it. */
function whip(m: Mix, t: number): void {
  m.duck(t, 0.5, 0.12);
  const out = swell(t - 0.14, t - 0.09, 0.05, t - 0.004, 0.45, t + 0.012);
  whoosh(m, out, sweep(t - 0.14, 900, t, 4800), 0.9, { pan: line(t - 0.14, 0, t, -0.85) }, "white");
  const inn = ad(t - 0.01, t + 0.004, 0.42, t + 0.16);
  whoosh(m, inn, sweep(t - 0.01, 4800, t + 0.14, 900), 0.9, { pan: line(t - 0.01, 0.85, t + 0.14, 0), send: 0.1 }, "white");
  thump(m, t, 0.4, 90, 45, 0.18);
}

/** The leap left, a rush of air, and the landing's thud in the corner. */
function hop(m: Mix, s: Section): void {
  const t0 = s.at(T_LEAP);
  const t1 = s.at(T_LAND);
  whoosh(m, ad(t0, (t0 + t1) / 2, 0.1, t1 + 0.03), sweep(t0, 900, t1, 1800), 1.3, { pan: line(t0, -0.2, t1, -0.6), send: 0.06 });
  thump(m, t1, 0.45, 130, 52, 0.25, { pan: -0.6 });
  knock(m, t1, hz(38), 0.22, -0.6, 0.08);
}

/** The remote falls in with a thud; its backends pop in, one per beat's eighth. */
function remote(m: Mix, s: Section): void {
  const t0 = s.at(T_DROP);
  const t = s.at(T_REMOTE);
  whoosh(m, ad(t0, t - 0.01, 0.1, t + 0.02), sweep(t0, 2600, t, 800), 1.4, { pan: 0.5, send: 0.08 }, "white");
  m.duck(t, 0.3, 0.15);
  stamp(m, t, 0.7, 0.5);
  CHIPS.forEach((at, i) => pop(m, s.at(at) + 0.02, hz(74 + [0, 3, 7][i]), 0.14, -0.05 + 0.1 * i, 0.15));
  // `uses: jdx/mr-boxington-action@v1` types over three quarters of a beat.
  const u0 = s.at(T_USES);
  for (let i = 0; i < 12; i++) tick(m, u0 + (i / 12) * 0.35, 2000 + 60 * (i % 5), 0.05, -0.1);
}

/** Each output leaves the push runner with a short upward swoosh and clicks onto the shelf. */
function uploads(m: Mix, s: Section): void {
  UPLOADS.forEach((at, k) => {
    const t0 = s.at(at);
    const t1 = t0 + UPLOAD_FLY;
    whoosh(m, ad(t0, t0 + 0.08, 0.07, t1), sweep(t0, 700, t1, 2600), 1.6, { pan: line(t0, 0.2, t1, 0.35 + 0.05 * k), send: 0.08 });
    tick(m, t1, 1700 + 80 * k, 0.12, 0.35 + 0.05 * k);
    knock(m, t1, hz(55 + (k % 4) * 2), 0.1, 0.35 + 0.05 * k, 0.06);
  });
}

/** The read-only line locks: a small metal click. */
function lock(m: Mix, s: Section): void {
  const t = s.at(T_LOCK);
  tick(m, t, 2600, 0.16, 0.6);
  tick(m, t + 0.035, 3400, 0.12, 0.6);
}

/** Restores drop to the pull request runner, each landing with a soft sparkle. */
function restores(m: Mix, s: Section): void {
  RESTORES.forEach((at, k) => {
    const t = s.at(at) + RESTORE_FLY;
    ping(m, t, hz(88 + [0, 2, 4, 7, 9, 7, 4][k]), 0.07, 0.25, { pan: 0.55, send: 0.35 });
    ping(m, t + 0.03, hz(95 + [0, 2, 4, 7, 9, 7, 4][k]), 0.04, 0.2, { pan: 0.6, send: 0.35 });
    tick(m, t, 2400, 0.06, 0.55);
  });
}

/** The pull request's own compile, thrown up, thunks off the line with a clank of the lock, and drops back. */
function bounce(m: Mix, s: Section): void {
  const t0 = s.at(T_THROW);
  const t = s.at(T_BOUNCE);
  whoosh(m, ad(t0, t - 0.01, 0.06, t + 0.01), sweep(t0, 600, t, 1600), 1.4, { pan: 0.5 });
  m.duck(t, 0.3, 0.15);
  thump(m, t, 0.45, 170, 70, 0.2, { pan: 0.5 });
  clink(m, t + 0.012, 0.09, 0.6);
  const v = m.voice(perc(t, 0.1, 0.001, 0.05), { pan: 0.5, send: 0.1 });
  if (v) v.noise("white", 1, v.filter("bandpass", 1200, 1.5));
  knock(m, s.at(T_BACK), hz(45), 0.16, 0.5, 0.06);
}

export const part: Part = {
  cues(m, s) {
    hop(m, s);
    remote(m, s);
    uploads(m, s);
    lock(m, s);
    restores(m, s);
    bounce(m, s);
    whip(m, s.end);
  },
  drums: (m, s) => drumBars(m, s, [HALF]),
  bass: (m, s) => bassBars(m, s, [D, G, A]),
  pads: (m, s) => chordBars(m, s, [CHORD.D, CHORD.G, CHORD.A]),
};
