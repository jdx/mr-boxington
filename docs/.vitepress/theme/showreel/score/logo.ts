// The end card, the resolve: the downbeat everything builds to, the face
// popping back on, the tape and the strawberry, the snap out to the card,
// the command typing, and the last glint and blink.

import type { Section } from "../bible";
import {
  berryBounces,
  INSTALL_KEYS,
  SNAP_DUR,
  T_BERRY,
  T_BLINK,
  T_BLUSH,
  T_EYE,
  T_GLINT,
  T_LINE,
  T_MONO,
  T_MUST,
  T_SHINE,
  T_SNAP,
  T_TAPE,
  T_URL,
  T_WORD,
  TAPE_LEAD,
  WORD_IN,
} from "../scenes/s8-logo";
import type { Part } from ".";
import { ad, hold, hz, line, type Mix, perc, type Pt, sweep, warmOf } from "./mix";
import { blip, boing, clap, clink, ding, flick, pad, pop, tick, whoosh } from "./sounds";

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

/**
 * s8's face, one feature per sixteenth up the D major arpeggio: the eye
 * with an eyelid tick, the mustache's flourish, the monocle's clink, the
 * glint's shimmer across the lens, and the cheeks' swell.
 */
function face(m: Mix, s: Section): void {
  const cues = [T_EYE, T_MUST, T_MONO, T_SHINE, T_BLUSH];
  [74, 78, 81, 86, 90].forEach((n, k) => {
    pop(m, s.at(cues[k]), hz(n), 0.18, (k % 2 ? -1 : 1) * 0.2, 0.25);
  });
  flick(m, s.at(T_EYE), -0.1, 0.1);
  boing(m, s.at(T_MUST) + 0.02, 0.1, hz(62), 0.3);
  clink(m, s.at(T_MONO), 0.14, 0.22);
  // The glint's band crosses the lens in four steps.
  const g = s.at(T_SHINE) - 0.05;
  [93, 97, 100, 105].forEach((n, i) => blip(m, g + i * 0.1, hz(n), 0.035));
  // The cheeks swell rose.
  const c = s.at(T_BLUSH);
  const v = m.voice(ad(c, c + 0.12, 0.05, c + 0.45), { send: 0.3, pan: 0.05 });
  if (v) {
    v.osc("sine", sweep(c, hz(78), c + 0.2, hz(81)));
    v.osc("triangle", sweep(c, hz(85), c + 0.2, hz(88)), 0.3);
  }
}

/** s8 tapeAt: the tape starts across the lid TAPE_LEAD before its cue, fastest into it, and lands soft. */
function tape(m: Mix, tp: number): void {
  const a = tp - TAPE_LEAD;
  const v = m.voice(
    [[a, 0], [tp - 0.01, 0.55], [tp + 0.02, 0.4], [tp + 0.08, 0.1, "exp"], [tp + 0.2, 0.0001, "exp"], [tp + 0.204, 0]],
    { pan: line(a, -0.3, tp + 0.2, 0.3), send: 0.12, hold: true },
  );
  if (!v) return;
  const am = v.vca(0.6, v.filter("bandpass", sweep(a, 3200, tp + 0.2, 1500), 0.9));
  v.lfo("sawtooth", sweep(a, 130, tp + 0.2, 30), 0.4, am.gain);
  v.noise("crackle", 2.5, am, sweep(a, 2.6, tp + 0.2, 0.7));
  v.noise("white", 0.8, am);
}

/** The strawberry: a plop on its seat and a bell, then a smaller plop per bounce. */
function berry(m: Mix, t: number, bounces: readonly number[]): void {
  m.duck(t, 0.25, 0.12);
  const p = m.voice(perc(t, 0.22, 0.002, 0.16), { send: 0.2, pan: -0.15 });
  if (p) {
    p.osc("sine", [[t, hz(64)], [t + 0.03, hz(76), "exp"], [t + 0.14, hz(72), "exp"]]);
    p.noise("pink", perc(t, 0.4, 0.001, 0.03), p.filter("bandpass", 1800, 1.5), 1, t + 0.05);
  }
  bounces.forEach((b, i) => {
    const q = m.voice(perc(b, 0.08 / (i + 1), 0.002, 0.08), { send: 0.2, pan: -0.15 });
    if (q) q.osc("sine", [[b, hz(71)], [b + 0.02, hz(79), "exp"]]);
  });
  ding(m, t + 0.012, hz(86), 0.16, -0.1, 1.1);
  ding(m, t + 0.03, hz(93), 0.08, 0.2, 0.8);
}

/** s8 squareAt: the close-up snaps back from T_SNAP, most of the way in its first frames. */
function snapOut(m: Mix, t: number): void {
  whoosh(m, ad(t, t + 0.014, 0.16, t + SNAP_DUR), sweep(t, 2600, t + 0.3, 420), 1.1, { send: 0.15 });
  const s = m.voice(perc(t, 0.12, 0.004, 0.18), { send: 0.1 });
  if (s) s.osc("sine", sweep(t, 180, t + 0.15, 70));
}

function card(m: Mix, s: Section): void {
  // The wordmark rises in, a word per 1/32 note.
  const w0 = s.at(WORD_IN);
  const w = s.at(T_WORD);
  whoosh(m, ad(w0 - 0.05, w - 0.02, 0.16, w + 0.3), sweep(w0 - 0.05, 2000, w, 7000), 1.5, { send: 0.35 }, "white");
  tick(m, s.at(T_LINE), 2100, 0.06, -0.1, 0.2);
  // The command types: a soft key tick on every third key.
  INSTALL_KEYS.forEach((k, i) => {
    if (i % 3 === 0 || i === INSTALL_KEYS.length - 1) tick(m, s.at(k), 2600 + 300 * (i % 2), 0.035, 0.15, 0.08);
  });
  // The address lands with its underline.
  const u = s.at(T_URL);
  tick(m, u, 2500, 0.07, 0.1, 0.25);
  ding(m, u + 0.004, hz(93), 0.06, 0.1, 0.6);
}

export const part: Part = {
  cues(m, s) {
    resolve(m, s.start);
    face(m, s);
    tape(m, s.at(T_TAPE));
    berry(m, s.at(T_BERRY), berryBounces().map((b) => s.at(b)));
    snapOut(m, s.at(T_SNAP));
    card(m, s);
    // The glint.
    ding(m, s.at(T_GLINT), hz(86), 0.2, 0.15, 0.9);
    ding(m, s.at(T_GLINT) + 0.003, hz(93), 0.1, 0.25, 0.7);
    // A blink.
    blip(m, s.at(T_BLINK), hz(93), 0.07);
    blip(m, s.at(T_BLINK) + 0.07, hz(97), 0.05);
  },
  pads(m, s) {
    // A soft Dmaj9 afterglow under the card, gone before the end.
    pad(m, s.start, s.end - 0.55, [50, 57, 62, 64, 66, 73], 0.07, 1300, 0.25, 0.35);
    // A low D swells in under the hold and out before the tail.
    const h = m.voice(hold(s.at(T_URL), 0.8, 0.03, s.end - 0.8, 0.02, 0.5), { bus: "music", send: 0.2, hold: true });
    if (h) h.osc("sine", hz(38));
  },
};
