// First build fills the store: half-time. A placeholder until the
// section's own sounds are written: the crackle, the gulp, the key's chips
// and letter flips, the stamp, four lid clicks, the tape.

import type { Part } from ".";
import { A, BM, bassBars, CHORD, chordBars, drumBars, G, HALF } from "./grooves";

export const part: Part = {
  drums: (m, s) => drumBars(m, s, [HALF]),
  bass: (m, s) => bassBars(m, s, [BM, G, A]),
  pads: (m, s) => chordBars(m, s, [CHORD.Bm, CHORD.G, CHORD.A]),
};
