// The logo resolve: the downbeat everything builds to, the face popping back
// on, the snap out to the lockup, and the last blink and glint.

import type { Section } from "../bible";
import {
  LABEL_HIT,
  MONO_SEAT,
  T_BLINK,
  T_BROWS,
  T_GLINT,
  T_PULL,
  T_TAG,
  T_TAPE,
  T_URL,
  T_WORD,
  TAPE_LEAD,
} from "../scenes/s8-logo";
import type { Part } from ".";
import { ad, hz, line, type Mix, perc, type Pt, sweep, warmOf, X } from "./mix";
import { blip, boing, clap, clink, ding, flick, flutter, pad, pop, stamp, tick, whoosh } from "./sounds";

function resolve(m: Mix, t: number): void {
  m.duck(t, 0.7, 0.4);
  const sub = m.voice(perc(t, 0.42, 0.006, 1.6), { hold: true });
  if (sub) {
    sub.osc("sine", [[t, 120], [t + 0.07, 60, "exp"], [t + 1.5, 36.7, "exp"]]);
    sub.osc("sine", [[t, 240], [t + 0.07, 120, "exp"], [t + 1.5, 73.4, "exp"]], 0.15);
  }
  const k = m.voice(perc(t, 0.27, 0.001, 0.2));
  if (k) {
    k.osc("sine", sweep(t, 220, t + 0.03, 80));
    k.noise("white", perc(t, 0.22, 0.0004, 0.015), k.filter("highpass", 2000, 0), 1, t + 0.03);
  }
  // Dmaj9, wide, through a closing filter.
  const env: Pt[] = [[t, 0], [t + 0.004, 0.14], [t + 0.35, 0.09, "exp"], [t + 1.6, 0.0001, "exp"], [t + 1.604, 0]];
  for (const side of [-1, 1]) {
    const v = m.voice(env, { pan: side * 0.45, send: 0.35, hold: true });
    if (!v) continue;
    const lp = v.filter("lowpass", [[t, 7000], [t + 0.5, 1400, "exp"], [t + 1.6, 700, "exp"]], 1);
    for (const n of [50, 57, 64, 66, 73]) v.osc(warmOf(m.ac, m.sh), hz(n), 0.22, lp, side * 8);
  }
  // A soft crash: pink noise with a cymbal's shape (a bell around 6.5 kHz,
  // rolled off above 10 kHz so the master's air shelf adds no fizz), then
  // the shockwave ring.
  const c = m.voice(perc(t, 0.3, 0.002, 1.4), { send: 0.35, hold: true });
  if (c) {
    const bell = c.filter("peaking", 6500, 1.2, c.filter("lowpass", sweep(t, 11000, t + 1.2, 7500), 0));
    bell.gain.value = 5;
    c.noise("pink", 1, c.filter("highpass", sweep(t, 4200, t + 1.2, 2400), 0, bell));
  }
  whoosh(m, ad(t, t + 0.008, 0.2, t + 0.7), sweep(t, 3000, t + 0.6, 300), 2, { send: 0.3, hold: false });
  clap(m, t, 0.45);
}

function face(m: Mix, s: Section): void {
  const t0 = s.at(T_BROWS);
  // Brows, eyes, mustache, monocle, bow tie, tape, label: one per sixteenth,
  // climbing the D major arpeggio.
  [74, 78, 81, 86, 88, 90, 93].forEach((n, k) => {
    pop(m, t0 + k * X, hz(n), 0.2, (k % 2 ? -1 : 1) * 0.2, 0.25);
  });
  // Small echoes of scene 2 under the pops. s8 seats the monocle and slams
  // the label 4 ms before their sixteenths (MONO_SEAT, LABEL_HIT).
  flick(m, t0, 0, 0.08);
  boing(m, t0 + 2 * X, 0.1, hz(62), 0.25);
  clink(m, s.at(MONO_SEAT), 0.12, 0.2);
  flutter(m, t0 + 4 * X, 0.12, 0.12);
  stamp(m, s.at(LABEL_HIT), 0.35, 0.1);
  // s8 tapeAt: the tape swipes across the top from TAPE_LEAD (25 ms) before
  // its sixteenth, fastest into the cue, and lands softly down the side.
  const tp = s.at(T_TAPE);
  const v = m.voice(
    [[tp - TAPE_LEAD, 0], [tp - 0.021, 0.6], [tp, 0.4], [tp + 0.06, 0.1, "exp"], [tp + 0.19, 0.0001, "exp"], [tp + 0.194, 0]],
    { pan: line(tp - TAPE_LEAD, -0.3, tp + 0.19, 0.4), send: 0.12, hold: true },
  );
  if (v) {
    const am = v.vca(0.6, v.filter("bandpass", sweep(tp - TAPE_LEAD, 3200, tp + 0.19, 1500), 0.9));
    v.lfo("sawtooth", sweep(tp - TAPE_LEAD, 130, tp + 0.19, 30), 0.4, am.gain);
    v.noise("crackle", 2.5, am, sweep(tp - TAPE_LEAD, 2.6, tp + 0.19, 0.7));
    v.noise("white", 0.8, am);
  }
}

/**
 * s8 leanAt: the close-up snaps back out from 4 ms before b1 (T_PULL) on a
 * front-loaded curve, most of the way in its first 100 ms, revealing the
 * lockup below.
 */
function snapOut(m: Mix, t: number): void {
  whoosh(m, ad(t, t + 0.014, 0.16, t + 0.34), sweep(t, 2600, t + 0.3, 420), 1.1, { send: 0.15 });
  const s = m.voice(perc(t, 0.12, 0.004, 0.18), { send: 0.1 });
  if (s) s.osc("sine", sweep(t, 180, t + 0.15, 70));
}

function wordmark(m: Mix, t: number, tag: number, url: number): void {
  whoosh(m, ad(t, t + 0.15, 0.2, t + 0.5), sweep(t, 2000, t + 0.3, 7000), 1.5, { send: 0.35 }, "white");
  // The tagline and the URL tick in.
  tick(m, tag, 2100, 0.06, -0.1, 0.2);
  tick(m, url, 2500, 0.05, 0.1, 0.2);
}

export const part: Part = {
  cues(m, s) {
    resolve(m, s.start);
    face(m, s);
    snapOut(m, s.at(T_PULL));
    wordmark(m, s.at(T_WORD), s.at(T_TAG), s.at(T_URL));
    // A blink.
    blip(m, s.at(T_BLINK), hz(93), 0.07);
    blip(m, s.at(T_BLINK) + 0.07, hz(97), 0.05);
    // The final glint.
    ding(m, s.at(T_GLINT), hz(86), 0.2, 0.15, 0.9);
    ding(m, s.at(T_GLINT) + 0.003, hz(93), 0.1, 0.25, 0.7);
  },
  pads(m, s) {
    // A soft Dmaj9 afterglow under the logo, gone before the end.
    pad(m, s.start, s.end - 0.55, [50, 57, 62, 64, 66, 73], 0.07, 1300, 0.25, 0.35);
  },
};
