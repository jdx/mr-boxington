// Every checkout compiles again: a breakdown. The groove drops to a
// lowpassed heartbeat under long, dark bass notes on G, then A. Each card
// thwacks up on its beat, its prompt types, and its build starts the same
// compile loop as the others, a few cents apart, so the three stack into a
// chorus of the same work. The thread zips through the `Compiling syn` rows
// with a pluck at each; the local cards slam shut, their slabs are thrown,
// the old slabs thud onto the pile on 32nds and the thrown ones land on top.
// A riser and a gathering inhale build to Mr Boxington's pop, half a beat
// before the half-time groove starts.

import { BEAT, type Section } from "../bible";
import { rng } from "../math";
import {
  CARDS,
  GATHER,
  ROWS,
  rowAt,
  STITCHES,
  T_STOOD,
  T_DIM,
  T_DROPS,
  T_FOLD,
  T_LANDS,
  T_POP,
  T_PULL,
  T_SHUT,
  T_SPARKS,
  T_TEETER,
  T_TYPE,
  TYPE,
} from "../scenes/every-checkout";
import type { Part } from ".";
import { ad, hold, hz, line, type Mix, perc, type Pt, swell, sweep, X } from "./mix";
import { CHORD, heartbeat } from "./grooves";
import { bassRun, boing, knock, pad, ping, pop, thump, tick, whoosh } from "./sounds";

/** Each card's place in the stereo field, left to right as on screen. */
const PAN = [-0.6, 0, 0.6] as const;
/** The three compile loops' tuning, cents: the same loop, a little apart. */
const DETUNE = [-18, 0, 21] as const;

/** A card stands up from its slab: a cardboard thwack, with a floor thud under the local ones. */
function standUp(m: Mix, t: number, k: number): void {
  m.duck(t, 0.15, 0.08);
  knock(m, t, hz(52 + 2 * k), 0.34, PAN[k], 0.14);
  if (CARDS[k] !== "ci") thump(m, t, 0.35, 150, 60, 0.16, { pan: PAN[k] });
  whoosh(m, ad(t - 0.12, t - 0.01, 0.06, t + 0.05), sweep(t - 0.12, 900, t, 3000), 1.4, { pan: PAN[k] });
}

/** `cargo build` typing on (map.ts typedChars: a character shows once its share of the time has passed). */
function typing(m: Mix, t0: number, k: number): void {
  const text = "cargo build";
  const r = rng(410 + k);
  for (let i = 0; i < text.length; i++) {
    if (text[i] === " ") continue;
    const t = t0 + ((i + 1) / text.length) * TYPE;
    tick(m, t, 1900 + 700 * r(), i % 2 ? 0.22 : 0.34, PAN[k] + (r() - 0.5) * 0.1);
  }
  // Enter.
  const enter = t0 + TYPE;
  tick(m, enter, 1300, 0.45, PAN[k]);
  thump(m, enter, 0.12, 300, 140, 0.05, { pan: PAN[k] });
}

/**
 * One card's compile loop from its Enter: a relay's patter on the
 * sixteenths, the same four-step phrase in every card, tuned a few cents
 * off the others. `fade` lets it close down (CI's dims) instead of stopping.
 */
function compileLoop(m: Mix, k: number, t0: number, t1: number, fade: readonly [number, number] | null): void {
  const steps = [
    [0, 1],
    [7, 0.45],
    [4, 0.7],
    [9, 0.4],
  ] as const;
  const cents = 2 ** (DETUNE[k] / 1200);
  for (let n = 0; ; n++) {
    const t = t0 + n * X;
    if (t >= t1 - 1e-6) break;
    const [iv, vel] = steps[n % 4];
    const f = hz(74 + iv) * cents;
    const out = fade ? 1 - Math.max(0, Math.min(1, (t - fade[0]) / (fade[1] - fade[0]))) : 1;
    const g = 0.085 * vel * out;
    if (g < 0.004) continue;
    const v = m.voice(perc(t, g, 0.001, 0.07), { pan: PAN[k], send: 0.1 });
    if (!v) continue;
    const lp = v.filter("lowpass", fade && t > fade[0] ? 900 : 2600, 3);
    v.osc("square", f, 0.5, lp);
    v.osc("triangle", f / 2, 0.6, lp);
    v.noise("white", perc(t, 0.4, 0.0005, 0.012), v.filter("bandpass", 4200, 2), 1, t + 0.03);
  }
}

/** A row lands: libc ticks in; syn, the heavy one, lands with a low amber chunk. */
function row(m: Mix, t: number, k: number, j: number): void {
  if (ROWS[j] === "syn") {
    m.duck(t, 0.12, 0.08);
    const v = m.voice(perc(t, 0.2, 0.002, 0.22), { pan: PAN[k], send: 0.16 });
    if (v) {
      const lp = v.filter("lowpass", sweep(t, 2800, t + 0.18, 500), 5);
      v.osc("sawtooth", hz(50) * 2 ** (DETUNE[k] / 1200), 1, lp);
      v.osc("square", hz(62) * 2 ** (DETUNE[k] / 1200), 0.4, lp);
    }
    thump(m, t, 0.2, 120, 55, 0.12, { pan: PAN[k] });
  } else {
    ping(m, t, hz(86), 0.05, 0.12, { pan: PAN[k], send: 0.15 });
  }
}

/** The thread zips from card to card, plucking where it stitches each `Compiling syn`. */
function thread(m: Mix, s: Section): void {
  const t0 = s.at(STITCHES[0]);
  const t1 = s.at(STITCHES[2]) + 0.12;
  whoosh(m, ad(t0 - 0.05, t1 - 0.05, 0.07, t1 + 0.1), sweep(t0, 1100, t1, 4200), 2.4, { pan: line(t0, -0.65, t1, 0.65), send: 0.2 });
  STITCHES.forEach((at, k) => {
    const t = s.at(at);
    ping(m, t, hz([74, 78, 81][k]), 0.11, 0.5, { pan: PAN[k], send: 0.32 });
    ping(m, t + 0.012, hz([86, 90, 93][k]), 0.05, 0.3, { pan: PAN[k], send: 0.32 });
  });
}

/** A spark runs the thread: a soft glassy glide left to right. */
function spark(m: Mix, t: number): void {
  const len = 0.75 * BEAT;
  const v = m.voice(ad(t, t + len * 0.5, 0.022, t + len), { pan: line(t, -0.6, t + len, 0.6), send: 0.4, hold: true });
  if (v) v.osc("sine", sweep(t, hz(86), t + len, hz(93)));
}

/** The local cards fold down: a flap of air, then two cardboard slams. */
function fold(m: Mix, t0: number, shut: number): void {
  whoosh(m, ad(t0, shut - 0.01, 0.1, shut + 0.02), sweep(t0, 700, shut, 2400), 1.2, { send: 0.1 });
  m.duck(shut, 0.4, 0.15);
  [PAN[0], PAN[1]].forEach((pan, i) => {
    thump(m, shut + i * 0.008, 0.42, 130, 48, 0.3, { pan });
    knock(m, shut + i * 0.008, hz(45), 0.3, pan, 0.1);
  });
}

/** The two slabs thrown up off the floor: a rising, then falling rush each. */
function toss(m: Mix, t0: number): void {
  T_LANDS.forEach((land, i) => {
    const t1 = t0 + (land - T_SHUT);
    const pan = i === 0 ? PAN[1] : PAN[0];
    whoosh(m, swell(t0, t0 + 0.04, 0.05, t1 - 0.03, 0.09, t1 + 0.02), [[t0, 900], [(t0 + t1) / 2, 2600, "exp"], [t1, 700, "exp"]], 1.3, {
      pan: line(t0, pan, t1, 0.05),
      send: 0.12,
    });
  });
}

/** A slab lands on the pile: a hollow cardboard thud, heavier for the thrown ones. */
function slabThud(m: Mix, t: number, f: number, vel: number): void {
  m.duck(t, 0.25 * vel, 0.1);
  thump(m, t, 0.5 * vel, 170, 58, 0.2, { pan: 0.05 });
  knock(m, t, f, 0.3 * vel, 0.05, 0.12);
}

/** The pile teeters: a slow wooden creak, rocking with it. */
function creak(m: Mix, t0: number, t1: number): void {
  const r = rng(707);
  const rate: Pt[] = [];
  for (let t = t0; t <= t1 + 1e-9; t += 0.015) rate.push([t, 18 + 10 * Math.sin(((t - t0) / (t1 - t0)) * Math.PI * 2) + 5 * r()]);
  const v = m.voice(hold(t0, 0.08, 0.6, t1 - 0.05, 0.3, 0.06), { send: 0.14, hold: true, pan: 0.1 });
  if (!v) return;
  const saw = v.osc("sawtooth", rate, 1, v.filter("bandpass", sweep(t0, 700, t1, 560), 8));
  saw.connect(v.gain(0.6, v.filter("bandpass", sweep(t0, 1600, t1, 1300), 10)));
}

/**
 * Into the pop: noise and rising fifths swelling under a gate that doubles
 * its rate, the sparks' inhale, and the camera pulling back, all cut dead on
 * the pop.
 */
function riser(m: Mix, s: Section, t0: number, pop: number): void {
  const cut = (a: number, peak: number): Pt[] => [
    [t0, 0],
    [t0 + 0.3, a],
    [pop - 0.006, peak, "exp"],
    [pop, 0.0001, "exp"],
    [pop + 0.003, 0],
  ];
  const n = m.voice(cut(0.006, 0.12), { send: 0.06, hold: true });
  if (n) n.noise("white", 1, n.filter("bandpass", sweep(t0, 500, pop, 6500), 1.1));
  const f = m.voice(cut(0.012, 0.1), { send: 0.08, hold: true });
  if (f) {
    const lp = f.filter("lowpass", sweep(t0, 350, pop, 4200), 3);
    // G over D rising an octave into the D of the half-time downbeat.
    for (const det of [-9, 9]) f.osc("sawtooth", sweep(t0, hz(43), pop, hz(55)), 0.5, lp, det);
    f.osc("sawtooth", sweep(t0, hz(50), pop, hz(62)), 0.35, lp);
  }
  const g = m.voice(cut(0.006, 0.12), { send: 0.05, hold: true, pan: -0.05 });
  if (g) {
    const am = g.vca(0.5, g.filter("bandpass", sweep(t0, 1000, pop, 3800), 0.9));
    const period = (t: number) => (t < s.beat(10) ? 2 * X : t < s.beat(11) ? X : X / 2);
    const cycle = (t: number) => t0 + Math.ceil((t - t0) / period(t) - 1e-9) * period(t);
    g.lfo("square", [[t0, 2 / BEAT], [s.beat(10), 4 / BEAT, "set"], [s.beat(11), 8 / BEAT, "set"]], 0.5, am.gain, cycle);
    g.noise("white", 1, am);
  }
}

/** Sparks spiral in, then he pops: a cork pop over a woody thump, and the lid springing into its hover. */
function popIn(m: Mix, t: number): void {
  whoosh(m, ad(t - GATHER, t - 0.006, 0.14, t), sweep(t - GATHER, 5200, t, 1100), 2, { send: 0.14 });
  m.duck(t, 0.5, 0.2);
  pop(m, t, hz(69), 0.45, -0.25, 0.22);
  thump(m, t, 0.7, 125, 48, 0.4, { pan: -0.25 });
  boing(m, t + 0.03, 0.12, hz(57), 0.3);
  whoosh(m, ad(t, t + 0.01, 0.16, t + 0.45), sweep(t, 4200, t + 0.4, 500), 2.2, { send: 0.3, pan: -0.25 });
}

export const part: Part = {
  cues(m, s) {
    CARDS.forEach((_, k) => {
      standUp(m, s.at(T_STOOD[k]), k);
      typing(m, s.at(T_TYPE[k]), k);
      ROWS.forEach((_, j) => row(m, s.at(rowAt(k, j)), k, j));
      const ci = CARDS[k] === "ci";
      compileLoop(m, k, s.at(rowAt(k, 0)), s.at(ci ? T_DIM[1] : T_FOLD), ci ? [s.at(T_DIM[0]), s.at(T_DIM[1])] : null);
    });
    thread(m, s);
    for (const at of T_SPARKS) spark(m, s.at(at));
    fold(m, s.at(T_FOLD), s.at(T_SHUT));
    toss(m, s.at(T_SHUT));
    T_DROPS.forEach((at, i) => slabThud(m, s.at(at), hz(43 + 2 * i), 0.55));
    T_LANDS.forEach((at, i) => slabThud(m, s.at(at), hz(52 + 3 * i), 1));
    creak(m, s.at(T_TEETER) - 0.05, s.at(T_TEETER) + 0.75);
    whoosh(m, ad(s.at(T_PULL[0]), s.at(T_PULL[1]) - 0.1, 0.06, s.at(T_PULL[1]) + 0.1), sweep(s.at(T_PULL[0]), 2800, s.at(T_PULL[1]), 700), 1.5, { send: 0.15 });
    riser(m, s, s.beat(8.5), s.at(T_POP));
    popIn(m, s.at(T_POP));
  },
  drums: heartbeat,
  bass(m, s) {
    bassRun(m, [[s.start, s.bar(2) - 0.02, 31, 1]], 0.02, 220, 0);
    bassRun(m, [[s.bar(2), s.end - 0.03, 33, 1]], 0.02, 220, 0);
  },
  pads(m, s) {
    pad(m, s.start, s.bar(2), [...CHORD.G], 0.06, 1200, 0.15, 0.3, 200);
    pad(m, s.bar(2), s.end, [...CHORD.A], 0.06, 1600, 0.1, 0.3, 200);
  },
};
