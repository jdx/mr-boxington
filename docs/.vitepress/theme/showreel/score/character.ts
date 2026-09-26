// The character: the rock, the leap and its spin, the landing, the face
// popping in one feature per sixteenth, the name card typing in, and the
// breath before the dive. The groove waits for the second bar, so the kick
// enters under the name card.

import { rng } from "../math";
import {
  BLINK,
  BLUSH,
  CARD_LANDS,
  chainDotAt,
  EYE,
  GLINT,
  LAND,
  LAUNCH,
  LINE,
  MONO,
  MUST,
  NAME,
  PUSH,
  SPIN_QUARTERS,
} from "../scenes/s2-character";
import { WORD } from "../type";
import type { Part } from ".";
import { ad, hold, hz, line, type Mix, perc, type Pt, swell, sweep } from "./mix";
import { D } from "./grooves";
import { bassBar, blip, boing, clink, ding, flick, grooveBar, pad, thump, tick, whoosh } from "./sounds";

function creak(m: Mix, t0: number, t1: number): void {
  const r = rng(202);
  const rate: Pt[] = [];
  for (let t = t0; t <= t1 + 1e-9; t += 0.012) rate.push([t, 24 + 30 * ((t - t0) / (t1 - t0)) + 7 * r()]);
  const v = m.voice(hold(t0, 0.05, 1.2, t1 - 0.02, 2, 0.032), { send: 0.1, hold: true, pan: -0.1 });
  if (!v) return;
  // A slow pulse train rung through two cardboard formants.
  const saw = v.osc("sawtooth", rate, 1, v.filter("bandpass", sweep(t0, 620, t1, 840), 7));
  saw.connect(v.gain(0.7, v.filter("bandpass", sweep(t0, 1450, t1, 1950), 9)));
}

function launch(m: Mix, t: number, land: number): void {
  m.duck(t, 0.2, 0.1);
  const env: Pt[] = [
    [t, 0],
    [t + 0.06, 0.3],
    [t + 0.22, 0.24],
    [land - 0.02, 0.03, "exp"],
    [land, 0.0001, "exp"],
    [land + 0.004, 0],
  ];
  const v = m.voice(env, { pan: line(t, -0.15, land, 0.15), send: 0.18, hold: true });
  if (v) {
    const bp = v.filter("bandpass", [[t, 480], [t + 0.2, 2400, "exp"], [land, 800, "exp"]], 1.4);
    // The spin: one flutter per quarter turn of yaw, fast off the push and
    // slowing linearly toward touchdown (s2 SPIN_QUARTERS).
    const am = v.vca(0.62, bp);
    v.lfo("sine", line(t, SPIN_QUARTERS[0], land, SPIN_QUARTERS[1]), 0.34, am.gain);
    v.noise("pink", 1, am);
  }
  // The stretch: a rising fwip.
  const w = m.voice(perc(t, 0.2, 0.003, 0.22), { send: 0.25 });
  if (w) {
    w.osc("sine", sweep(t, hz(62), t + 0.09, hz(81)));
    w.osc("triangle", sweep(t, hz(74), t + 0.09, hz(93)), 0.12);
  }
}

function land(m: Mix, t: number): void {
  m.duck(t, 0.5, 0.2);
  thump(m, t, 1, 110, 42, 0.5);
  const v = m.voice(perc(t, 0.45, 0.0008, 0.1), { send: 0.15 });
  if (v) v.noise("white", 1, v.filter("lowpass", 1500, 0), 1, t + 0.12);
  // The dust puff.
  whoosh(m, ad(t, t + 0.03, 0.18, t + 0.45), sweep(t, 1200, t + 0.4, 500), 0.8, { send: 0.2, hold: false });
}

/** The eye opens: a dry eyelid tick with a small pitched blink of a note under it. */
function eyelid(m: Mix, t: number): void {
  m.duck(t, 0.15, 0.08);
  tick(m, t, 2300, 0.5, -0.25, 0.1);
  blip(m, t + 0.006, hz(88), 0.16);
  const v = m.voice(perc(t, 0.14, 0.001, 0.09), { pan: -0.25, send: 0.2 });
  if (v) v.osc("sine", sweep(t, hz(79), t + 0.03, hz(86)));
}

/** The mustache: a flick up into a short jaw-harp flourish as its tips curl and wave. */
function flourish(m: Mix, t: number): void {
  m.duck(t, 0.2, 0.15);
  flick(m, t - 0.012, -0.1, 0.12);
  boing(m, t, 0.24, hz(52), 0.3);
}

/**
 * The monocle: its chain pays out a dot at a time from the anchor (s2
 * chainDotAt), each link a tiny brass tick climbing in pitch, and the lens
 * clinks onto its end.
 */
function monocle(m: Mix, s: (t: number) => number, seat: number): void {
  for (let i = 5; i >= 0; i--) {
    const t = s(chainDotAt(i));
    const v = m.voice(perc(t, 0.07, 0.0004, 0.03), { pan: 0.3, send: 0.12 });
    if (v) {
      v.osc("sine", hz(98 + (5 - i)), perc(t, 0.8, 0.0003, 0.02));
      v.noise("white", 1, v.filter("bandpass", 6500 + 400 * (5 - i), 4), 1, t + 0.02);
    }
  }
  m.duck(seat, 0.2, 0.12);
  clink(m, seat, 0.22, 0.25);
  clink(m, seat + 0.07, 0.05, 0.3);
}

/** The glint arc draws on: a bright shimmer riding up over a high bell. */
function shimmer(m: Mix, t: number): void {
  ding(m, t, hz(100), 0.1, 0.3, 0.9);
  const v = m.voice(swell(t - 0.01, t + 0.02, 0.1, t + 0.05, 0.08, t + 0.32), { pan: line(t, 0.1, t + 0.3, 0.4), send: 0.35, hold: true });
  if (v) {
    const am = v.vca(0.5, v.filter("highpass", sweep(t, 5000, t + 0.3, 9000), 0));
    v.lfo("sine", 32, 0.45, am.gain);
    v.noise("white", 1, am);
  }
}

/** The cheeks: a soft rising swell, warm rather than bright. */
function swellUp(m: Mix, t: number): void {
  const v = m.voice(ad(t - 0.02, t + 0.12, 0.09, t + 0.5), { send: 0.3 });
  if (v) {
    v.osc("sine", sweep(t - 0.02, hz(69), t + 0.12, hz(74)));
    v.osc("triangle", sweep(t - 0.02, hz(76), t + 0.12, hz(81)), 0.25);
  }
}

/** A typewriter tick as each of the name card's words lands (type.ts drawWords). */
function typing(m: Mix, s: (t: number) => number): void {
  const r = rng(212);
  const lines = [NAME, ...LINE];
  lines.forEach((text, j) => {
    const words = text.split(" ").length;
    for (let i = 0; i < words; i++) {
      const t = s(CARD_LANDS[j] - (words - 1 - i) * WORD);
      tick(m, t, 1700 + 900 * r(), j === 0 ? 0.42 : 0.3, 0.2 + 0.3 * (r() - 0.5), 0.08);
    }
  });
}

/** The breath before the dive: the air draws in as the push starts and the card wipes. */
function inhale(m: Mix, t0: number, t1: number): void {
  whoosh(m, swell(t0 - 0.02, t0 + 0.04, 0.02, t1, 0.09, t1 + 0.03), sweep(t0, 300, t1, 1400), 1.2, { send: 0.15 });
}

export const part: Part = {
  cues(m, s) {
    const at = (t: number) => s.at(t);
    creak(m, s.start, at(LAUNCH));
    launch(m, at(LAUNCH), at(LAND));
    land(m, at(LAND));
    eyelid(m, at(EYE));
    flourish(m, at(MUST));
    monocle(m, at, at(MONO));
    shimmer(m, at(GLINT));
    swellUp(m, at(BLUSH));
    typing(m, at);
    // A blink.
    blip(m, at(BLINK), hz(93), 0.12);
    blip(m, at(BLINK) + 0.12, hz(97), 0.08);
    inhale(m, at(PUSH), s.end);
  },
  drums(m, s) {
    // The kick enters on the section's second bar, under the name card.
    grooveBar(m, s.bar(1), [0, 8], [4, 12]);
  },
  bass(m, s) {
    bassBar(m, s.bar(1), ...D);
  },
  pads: (m, s) => pad(m, s.start, s.end, [50, 57, 62, 64, 66], 0.07, 1500),
};
