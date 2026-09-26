// Another worktree: half-time under a checkout swap, the new key rolling and
// rewriting, the match click, climbing hit pings as the restores burst out,
// one amber crackle for the edited crate, then the tape and the strawberry.
// Every cue sits on an anchor or a formula from the scene, so the sound
// lands where the picture does.

import type { Section } from "../bible";
import {
  BURSTS,
  FLIPS,
  FOLD,
  HK,
  lidSteps,
  OPEN,
  RESTORES,
  RIP,
  SWAP,
  SYN,
  T_BERRY,
  T_CHIP,
  T_CLICK,
  T_CLOSE,
  T_DIGEST,
  T_HK,
  T_LID,
  T_SHOOT,
  T_START,
  T_STORED,
  T_TAKE,
  T_TAPE,
  TAPE_RUN,
  tapeAt,
  TYPE,
} from "../scenes/another-worktree";
import type { Part } from ".";
import { ad, crossing, hold, hz, line, type Mix, perc, swell, sweep } from "./mix";
import { A, BM, bassBars, CHORD, chordBars, D, drumBars, HALF } from "./grooves";
import { bloop, clink, ding, flick, knock, ping, pop, thump, tick, whoosh } from "./sounds";

/** The card folds flat, the two slabs trade places, and hk-fix's terminal flips up. */
function swap(m: Mix, s: Section): void {
  const fold = s.at(FOLD[1]);
  whoosh(m, swell(s.at(FOLD[0]), fold - 0.05, 0.02, fold, 0.1, fold + 0.03), sweep(s.at(FOLD[0]), 1400, fold, 500), 1.2, {
    send: 0.08,
    pan: 0.4,
  });
  knock(m, fold, hz(45), 0.3, 0.4, 0.1);
  thump(m, fold, 0.3, 150, 70, 0.14, { pan: 0.4 });
  // The slabs cross: one swoosh across them, each landing with its own knock.
  const a = s.at(SWAP[0]);
  const b = s.at(SWAP[1]);
  whoosh(m, swell(a, (a + b) / 2 - 0.04, 0.04, (a + b) / 2, 0.12, b + 0.02), sweep(a, 600, b, 2200), 1.4, {
    send: 0.1,
    pan: line(a, 0.55, b, 0.3),
  });
  knock(m, b, hz(47), 0.26, 0.45, 0.1);
  knock(m, b + 0.012, hz(52), 0.18, 0.2, 0.1);
  flick(m, s.at(OPEN[0]), 0.35, 0.14);
  pop(m, s.at(OPEN[1]), hz(66), 0.12, 0.35, 0.12);
}

/** `cargo build` types, and Enter starts the build: the tape rips, the lid pops, the strawberry flies off. */
function start(m: Mix, s: Section): void {
  const n = "cargo build".length;
  for (let i = 0; i < n; i++) {
    const t = s.at(TYPE[0] + ((i + 1) / n) * (TYPE[1] - TYPE[0]));
    tick(m, t, 2300 + ((i * 7) % 5) * 90, 0.12, 0.35, 0.04);
  }
  const enter = s.at(T_START);
  tick(m, enter, 1500, 0.24, 0.35, 0.06);
  // The tape tears off backward in one pull.
  const rip = m.voice(ad(enter, enter + 0.01, 0.4, enter + RIP + 0.03), { send: 0.1, pan: -0.25 });
  if (rip) {
    rip.noise("crackle", 3, rip.filter("bandpass", sweep(enter, 3200, enter + RIP, 1400), 0.8), 2.4);
    rip.noise("white", 0.8, rip.filter("highpass", 2500, 0));
  }
  const lid = s.at(T_LID);
  m.duck(lid, 0.2, 0.1);
  pop(m, lid, hz(62), 0.3, -0.25, 0.15);
  thump(m, lid, 0.25, 220, 110, 0.1, { pan: -0.25 });
  // The strawberry, flung: a little rising chirp as it goes.
  const f = m.voice(perc(lid + 0.02, 0.12, 0.004, 0.16), { pan: line(lid, -0.3, lid + 0.2, -0.45), send: 0.2 });
  if (f) f.osc("sine", sweep(lid + 0.02, hz(79), lid + 0.14, hz(91)));
}

/**
 * The new key: its digest rattles over at 30 Hz, the path's glyphs flip
 * one by one, the stored tag rises out of the box, each digit settles, and
 * the tags meet with the match click.
 */
function key(m: Mix, s: Section): void {
  const chip = s.at(T_CHIP);
  pop(m, chip, hz(76), 0.14, 0.1, 0.15);
  // The digest rolling: a soft rattle, one tick per change, until it settles.
  const settle = s.at(T_DIGEST);
  for (let k = 1; chip + k / 30 < settle - 0.105; k++) {
    tick(m, chip + k / 30, 3400 + (k % 3) * 300, 0.035, 0.15, 0.02);
  }
  // The stored tag rises on its string.
  const up = s.at(T_STORED);
  whoosh(m, ad(up - 0.02, up + 0.05, 0.08, up + 0.2), sweep(up, 900, up + 0.15, 2600), 2, { send: 0.12, pan: -0.05 });
  // The rewrite: each glyph of the checkout's path turns over.
  FLIPS.forEach((t, i) => tick(m, s.at(t), 2000 + i * 60, 0.07, 0.05 + i * 0.01, 0.05));
  tick(m, s.at(T_CLOSE), 2900, 0.12, 0.15, 0.08);
  // The rewritten path is taken up into the key; its digits land.
  whoosh(m, ad(s.at(T_TAKE), s.at(T_TAKE) + 0.08, 0.05, s.at(T_DIGEST)), sweep(s.at(T_TAKE), 1200, s.at(T_DIGEST), 3000), 2, {
    send: 0.1,
  });
  for (let i = 0; i < 4; i++) tick(m, settle - (3 - i) * 0.035, 2600 + i * 120, 0.12, 0.1, 0.05);
  // The new tag rises onto the stored one: the click.
  const click = s.at(T_CLICK);
  whoosh(m, swell(settle, click - 0.08, 0.02, click - 0.005, 0.1, click + 0.01), sweep(settle, 700, click, 2400), 1.5, {
    send: 0.08,
  });
  m.duck(click, 0.3, 0.12);
  tick(m, click, 3200, 0.4, 0, 0.1);
  tick(m, click + 0.012, 1800, 0.25, 0, 0.1);
  knock(m, click, hz(62), 0.2, 0, 0.12);
  clink(m, click + 0.01, 0.1, 0.05);
}

/**
 * The hits: syn shoots out as the tag is reeled in, then each burst of
 * restores leaves with a ping a scale step higher than the last (D major
 * from D5 up to C#6), a whoosh as it sprays, and a soft plink for every
 * carton that drops into hk-fix's target/.
 */
function hits(m: Mix, s: Section): void {
  const reel = s.at(T_CLICK) + 0.03;
  whoosh(m, ad(reel, s.at(T_SHOOT), 0.06, s.at(T_SHOOT) + 0.02), sweep(reel, 2400, s.at(T_SHOOT), 900), 2, { send: 0.08 });
  const shots = [SYN.launch, ...BURSTS];
  const notes = [74, 76, 78, 79, 81, 83, 85];
  shots.forEach((at, k) => {
    const t = s.at(at);
    const pan = -0.3 + 0.08 * k;
    ping(m, t, hz(notes[k]), 0.16, 0.5, { send: 0.3, pan });
    ping(m, t + 0.006, hz(notes[k] + 12), 0.05, 0.3, { send: 0.3, pan: pan + 0.1 });
    whoosh(m, ad(t, t + 0.03, 0.07, t + 0.3), sweep(t, 1500, t + 0.3, 4200), 1.8, { send: 0.12, pan: line(t, -0.3, t + 0.3, 0.3) });
  });
  // The monocle's glint on the first hit.
  const first = s.at(SYN.land);
  ding(m, first, hz(98), 0.05, 0.1, 0.6, 0.4);
  for (const f of [SYN, ...RESTORES]) {
    const t = s.at(f.land);
    tick(m, t, 3600 + (Math.round(f.land * 1000) % 7) * 150, 0.06, 0.3, 0.1);
  }
  // The lid comes down a step at a time as the bar fills.
  for (const t of lidSteps().filter((lt) => lt > T_LID + 0.3)) knock(m, s.at(t), hz(55), 0.12, -0.25, 0.06);
}

/**
 * hk (edited) compiles: an amber crackle that builds under the ring for as
 * long as rustc runs, a grind rising with it, and a thunk as its carton
 * drops into the slab and closes the bar.
 */
function compile(m: Mix, s: Section): void {
  const t0 = s.at(T_HK);
  const t1 = s.at(HK.launch);
  pop(m, t0, hz(57), 0.2, 0.1, 0.2);
  m.duck(t0, 0.15, 0.2);
  const v = m.voice(hold(t0, 0.04, 0.18, t1, 0.3, 0.06), { send: 0.1, pan: 0.05, hold: true });
  if (v) {
    const bp = v.filter("bandpass", sweep(t0, 700, t1, 2200), 1.1);
    v.noise("crackle", 2.8, bp, sweep(t0, 0.6, t1, 1.6));
    v.noise("pink", 0.4, bp);
    v.osc("sawtooth", sweep(t0, hz(38), t1, hz(45)), 0.25, v.filter("lowpass", sweep(t0, 300, t1, 900), 3));
  }
  // It leaves the ring and drops into the slab.
  flick(m, t1, 0.15, 0.12);
  const land = s.at(HK.land);
  m.duck(land, 0.25, 0.12);
  thump(m, land, 0.4, 170, 60, 0.2, { pan: 0.2 });
  knock(m, land, hz(45), 0.22, 0.2, 0.1);
  tick(m, land + 0.004, 2600, 0.12, 0.25, 0.08);
}

/** Taped, then the strawberry: a zip down the tape, a pat, a plop and a small bell. */
function finish(m: Mix, s: Section): void {
  const t0 = s.at(T_TAPE);
  const t1 = s.at(T_TAPE + TAPE_RUN);
  // tapeAt runs over the lid to 0.5, then down the front.
  const turn = crossing((t) => tapeAt(t - s.start), 0.5, t0, TAPE_RUN);
  const v = m.voice(
    [
      [t0, 0],
      [t0 + 0.006, 0.36],
      [turn, 0.26],
      [t1, 0.0001, "exp"],
      [t1 + 0.004, 0],
    ],
    { send: 0.12, pan: -0.2, hold: true },
  );
  if (v) {
    const bp = v.filter("bandpass", [[t0, 3000], [turn, 1800, "exp"], [t1, 1300, "exp"]], 0.8);
    v.noise("crackle", 3, bp, 2.2);
    v.noise("white", 1, bp);
  }
  tick(m, turn, 1900, 0.14, -0.2, 0.08);
  thump(m, t1, 0.2, 240, 120, 0.08, { pan: -0.2 });
  const berry = s.at(T_BERRY);
  bloop(m, berry, hz(62), 0.3, -0.25);
  ding(m, berry + 0.03, hz(86), 0.07, -0.15, 1, 0.4);
}

export const part: Part = {
  cues(m, s) {
    swap(m, s);
    start(m, s);
    key(m, s);
    hits(m, s);
    compile(m, s);
    finish(m, s);
  },
  drums: (m, s) => drumBars(m, s, [HALF]),
  bass: (m, s) => bassBars(m, s, [D, BM, A]),
  pads: (m, s) => chordBars(m, s, [CHORD.D, CHORD.Bm, CHORD.A]),
};
