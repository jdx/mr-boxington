// Another worktree: half-time. A placeholder until the section's own sounds
// are written: a match click, climbing hit pings, one amber crackle.

import type { Part } from ".";
import { A, BM, bassBars, CHORD, chordBars, D, drumBars, HALF } from "./grooves";

export const part: Part = {
  drums: (m, s) => drumBars(m, s, [HALF]),
  bass: (m, s) => bassBars(m, s, [D, BM, A]),
  pads: (m, s) => chordBars(m, s, [CHORD.D, CHORD.Bm, CHORD.A]),
};
