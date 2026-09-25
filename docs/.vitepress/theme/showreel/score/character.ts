// The character: the rock, the leap and its spin, the landing, and the face
// assembling one feature per sixteenth.

import { rng } from "../math";
import {
  BLINK,
  BROWS,
  EYES,
  GLINT,
  LABEL,
  LAND,
  LAUNCH,
  MONO,
  MONO_UP,
  MUST,
  TIE,
} from "../scenes/s2-character";
import type { Part } from ".";
import { ad, hold, hz, line, type Mix, perc, type Pt, sweep } from "./mix";
import { bassBar, blip, boing, clink, ding, flick, flutter, grooveBar, pad, pop, stamp, thump, whoosh } from "./sounds";

function creak(m: Mix, t0: number, t1: number): void {
  const r = rng(202);
  const rate: Pt[] = [];
  for (let t = t0; t <= t1 + 1e-9; t += 0.012) rate.push([t, 24 + 30 * ((t - t0) / (t1 - t0)) + 7 * r()]);
  const v = m.voice(hold(t0, 0.05, 1.2, t1 - 0.02, 2, 0.032), { send: 0.1, hold: true, pan: 0.05 });
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
    // The spin: one flutter per quarter turn of yaw. s2 spins 374 degrees
    // in the air, fast off the push and slowing linearly (SPIN_K 0.35), so
    // the quarter turns come at 12 a second at takeoff and 5.8 at touchdown.
    const am = v.vca(0.62, bp);
    v.lfo("sine", line(t, 12, land, 5.8), 0.34, am.gain);
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

/**
 * s2 monocle: flicked up from behind the lid (MONO_UP, 0.19 s before the
 * seat) on an ease out to the top of its arc, it drops into the seat with a
 * clink, swings out once on a 7 Hz pendulum, and is blended back onto the
 * seat by about 64 ms later.
 */
function monocle(m: Mix, up: number, seat: number): void {
  const f = m.voice(perc(up, 0.1, 0.004, 0.07), { pan: 0.2, send: 0.2 });
  if (f) {
    f.noise("pink", 1, f.filter("bandpass", sweep(up, 1600, up + 0.05, 4200), 2.2));
    f.osc("sine", sweep(up, hz(81), up + 0.05, hz(93)), 0.18);
  }
  m.duck(seat, 0.2, 0.12);
  clink(m, seat, 0.2, 0.25);
  const home = seat + 0.064;
  clink(m, home, 0.06, 0.28);
}

export const part: Part = {
  cues(m, s) {
    creak(m, s.start, s.at(LAUNCH));
    launch(m, s.at(LAUNCH), s.at(LAND));
    land(m, s.at(LAND));
    // Eyes pop.
    m.duck(s.at(EYES), 0.15, 0.1);
    pop(m, s.at(EYES), hz(81), 0.3, -0.2);
    pop(m, s.at(EYES) + 0.014, hz(86), 0.26, 0.2);
    // Brows flick.
    flick(m, s.at(BROWS), -0.15);
    flick(m, s.at(BROWS) + 0.012, 0.15);
    // The mustache unfurls.
    m.duck(s.at(MUST), 0.2, 0.2);
    boing(m, s.at(MUST));
    // The label stamps on.
    m.duck(s.at(LABEL), 0.3, 0.1);
    stamp(m, s.at(LABEL));
    monocle(m, s.at(MONO_UP), s.at(MONO));
    // The monocle glints.
    m.duck(s.at(GLINT), 0.25, 0.2);
    ding(m, s.at(GLINT), hz(93), 0.2, 0.25, 0.7);
    // The bow tie spins in.
    m.duck(s.at(TIE), 0.15, 0.12);
    flutter(m, s.at(TIE));
    // A blink.
    blip(m, s.at(BLINK), hz(93), 0.12);
    blip(m, s.at(BLINK) + 0.07, hz(97), 0.08);
  },
  drums: (m, s) => grooveBar(m, s.start, [0, 8], [4, 12]),
  bass: (m, s) => bassBar(m, s.start, 38, 0.3), // D
  pads: (m, s) => pad(m, s.start, s.end, [50, 57, 62, 64, 66], 0.07, 1500),
};
