// The groove over a whole section: a drum pattern, a bass root, and a pad
// chord for each of its bars. The reel keeps 128 BPM throughout, so a
// passage that should breathe gets the half-time pattern (kick on 1, clap on
// 3) rather than a slower tempo.

import type { Section } from "../bible";
import { bassBar, grooveBar, kick, pad } from "./sounds";
import type { Mix } from "./mix";

/** One bar of drums in sixteenths: kicks, claps, and ghost hats. */
export type Drums = readonly [kicks: readonly number[], claps: readonly number[], ghosts?: readonly number[]];

/** Kick on 1, clap on 3, hats on the off-beat eighths. */
export const HALF: Drums = [[0], [8]];
/** The full groove: kicks on 1, 3, and the and of 3, claps on 2 and 4, and ghost hats. */
export const FULL: Drums = [[0, 8, 10], [4, 12], [7, 15]];

/** A bass root, as a MIDI note, and how much grit carries it to small speakers. */
export type Root = readonly [note: number, grit: number];
export const D: Root = [38, 0.3];
export const BM: Root = [35, 0.45];
export const G: Root = [31, 0.9];
export const A: Root = [33, 0.75];

/** Pad voicings in D major. */
export const CHORD = {
  D: [50, 57, 62, 64, 66],
  Bm: [47, 54, 57, 62, 64],
  G: [43, 50, 54, 59, 62],
  A: [45, 52, 59, 61, 64],
} as const;

/** The item for bar `b`: the list's own, or its last one once the list runs out. */
const nth = <T>(list: readonly T[], b: number): T => list[Math.min(b, list.length - 1)];

/** A bar of drums for each bar of the section. */
export function drumBars(m: Mix, s: Section, bars: readonly Drums[]): void {
  for (let b = 0; b < s.bars; b++) grooveBar(m, s.bar(b), ...nth(bars, b));
}

/** A bar of the bass line for each bar of the section. */
export function bassBars(m: Mix, s: Section, roots: readonly Root[]): void {
  for (let b = 0; b < s.bars; b++) bassBar(m, s.bar(b), ...nth(roots, b));
}

/** A pad chord per bar, each held until the chord changes. */
export function chordBars(m: Mix, s: Section, chords: readonly (readonly number[])[], cutoff = 1500): void {
  let b0 = 0;
  for (let b = 1; b <= s.bars; b++) {
    if (b < s.bars && nth(chords, b) === nth(chords, b0)) continue;
    pad(m, s.bar(b0), s.bar(b), [...nth(chords, b0)], 0.07, cutoff);
    b0 = b;
  }
}

/** The breakdown's heartbeat: a lowpassed kick on 1 and 3, and no groove. */
export function heartbeat(m: Mix, s: Section): void {
  for (let b = 0; b < s.bars * 4; b += 2) kick(m, s.beat(b), 0.5, true);
}
