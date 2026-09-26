// Under cargo build: the half-time groove starts, on D. The camera rushes
// into the machine as the hk slab hops over and the terminal swings up;
// `cargo build` types and the build view comes up with a chime. Cargo's plan
// pops in one crate per eighth, climbing, and every `rustc` call lands in the
// gap under the lid with a wooden tock, syn's on the downbeat with an amber
// thunk. Under it all the raised lid hums; the ease in to the whole machine
// swells into first-build.

import { BEAT, type Section } from "../bible";
import { rng } from "../math";
import { FLY, SYN, T_CHIP, T_HIT, T_HOP, T_INTO, T_OPEN, T_PUSH, T_RUN, T_TYPE } from "../scenes/under-cargo-build";
import type { Part } from ".";
import { ad, hold, hz, line, type Mix, perc, sweep } from "./mix";
import { bassBars, CHORD, chordBars, D, drumBars, HALF } from "./grooves";
import { flick, knock, ping, pop, thump, tick, whoosh } from "./sounds";

/** Into the machine: a rush of air as the frame flies past the edges; the slab flicks off the pile and lands. */
function into(m: Mix, s: Section): void {
  const t0 = s.at(T_INTO[0]);
  const t1 = s.at(T_INTO[1]);
  whoosh(m, ad(t0, t1 - 0.05, 0.14, t1 + 0.12), sweep(t0, 600, t1, 3800), 1.2, { send: 0.18 });
  flick(m, t0 + 0.01, 0.2, 0.14);
  const land = s.at(T_HOP[1]);
  m.duck(land, 0.3, 0.12);
  thump(m, land, 0.45, 150, 55, 0.22, { pan: 0.45 });
  knock(m, land, hz(47), 0.25, 0.45, 0.12);
  // The window swings up off it.
  const o0 = s.at(T_OPEN[0]);
  whoosh(m, ad(o0, s.at(T_OPEN[1]) - 0.04, 0.07, s.at(T_OPEN[1]) + 0.05), sweep(o0, 1200, s.at(T_OPEN[1]), 3400), 1.6, { pan: 0.45 });
}

/** `cargo build` types; Enter, and the build view comes up with a small chime. */
function typing(m: Mix, s: Section): void {
  const text = "cargo build";
  const t0 = s.at(T_TYPE[0]);
  const len = T_TYPE[1] - T_TYPE[0];
  const r = rng(501);
  for (let i = 0; i < text.length; i++) {
    if (text[i] === " ") continue;
    tick(m, t0 + ((i + 1) / text.length) * len, 1900 + 700 * r(), i % 2 ? 0.22 : 0.34, 0.45);
  }
  const run = s.at(T_RUN);
  tick(m, run, 1300, 0.5, 0.45);
  thump(m, run, 0.14, 300, 140, 0.05, { pan: 0.45 });
  ping(m, run + 0.01, hz(81), 0.07, 0.5, { pan: 0.45, send: 0.35 });
  ping(m, run + 0.05, hz(86), 0.05, 0.45, { pan: 0.45, send: 0.35 });
}

/** Cargo's plan: a pop per crate, climbing the scale as the list unfolds downward. */
function plan(m: Mix, s: Section): void {
  const notes = [62, 64, 66, 69, 71, 74, 76, 78];
  T_CHIP.forEach((at, i) => pop(m, s.at(at), hz(notes[i]), 0.13, 0.15, 0.16));
}

/**
 * Each `rustc` call: a quick rush from its chip into the gap, then a wooden
 * tock as it lands. syn's is thicker: a darker rush, and on the downbeat a
 * tock over a low amber thunk.
 */
function calls(m: Mix, s: Section): void {
  T_HIT.forEach((at, i) => {
    const hit = s.at(at);
    const fire = hit - FLY;
    const syn = i === SYN;
    whoosh(m, ad(fire, hit - 0.01, syn ? 0.11 : 0.05, hit + 0.02), sweep(fire, syn ? 700 : 1500, hit, syn ? 2600 : 5000), 1.6, {
      pan: line(fire, 0.1, hit, -0.2),
      send: 0.1,
    });
    m.duck(hit, syn ? 0.35 : 0.12, 0.1);
    knock(m, hit, hz(syn ? 45 : 57 + 2 * (i % 3)), syn ? 0.42 : 0.3, -0.2, 0.14);
    if (syn) {
      thump(m, hit, 0.42, 110, 42, 0.35, { pan: -0.2 });
      const v = m.voice(perc(hit, 0.16, 0.002, 0.3), { pan: -0.2, send: 0.22 });
      if (v) {
        const lp = v.filter("lowpass", sweep(hit, 3000, hit + 0.25, 400), 5);
        v.osc("sawtooth", hz(38), 1, lp);
        v.osc("square", hz(50), 0.35, lp);
        v.noise("crackle", 1.5, v.filter("bandpass", 1800, 1.4), 1, hit + 0.12);
      }
    }
  });
}

/** The raised lid: a low hum on D and A, breathing slowly, that knocks up with each call. */
function hum(m: Mix, s: Section): void {
  const t0 = s.at(T_OPEN[0]);
  const t1 = s.end;
  const v = m.voice(hold(t0, 0.4, 0.028, t1 - 0.05, 0.022, 0.05), { send: 0.2, hold: true, pan: -0.25 });
  if (!v) return;
  const lp = v.filter("lowpass", 700, 1);
  const am = v.vca(0.8, lp);
  v.lfo("sine", 1 / (2 * BEAT), 0.2, am.gain);
  v.osc("sawtooth", hz(38), 0.5, am, -6);
  v.osc("sawtooth", hz(45), 0.3, am, 6);
  v.osc("sine", hz(50), 0.4, am);
}

/** The ease in to the whole machine (FOLLOW): a slow swell of air into first-build. */
function push(m: Mix, s: Section): void {
  const t0 = s.at(T_PUSH[0]);
  const t1 = s.at(T_PUSH[1]);
  whoosh(m, ad(t0, t1, 0.07, s.end + 0.05), sweep(t0, 400, t1, 1800), 1.3, { send: 0.25 });
}

export const part: Part = {
  cues(m, s) {
    into(m, s);
    typing(m, s);
    plan(m, s);
    calls(m, s);
    hum(m, s);
    push(m, s);
  },
  drums: (m, s) => drumBars(m, s, [HALF]),
  bass: (m, s) => bassBars(m, s, [D]),
  pads: (m, s) => chordBars(m, s, [CHORD.D]),
};
