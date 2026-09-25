// Six builds at once: the full groove. A placeholder until the section's own
// sounds are written: six drum loops out of phase that snap onto this
// groove when the permit rail lands, permit clicks, a chime on the shared
// result.

import type { Part } from ".";
import { A, BM, bassBars, CHORD, chordBars, drumBars, FULL, G } from "./grooves";

export const part: Part = {
  drums: (m, s) => drumBars(m, s, [FULL]),
  bass: (m, s) => bassBars(m, s, [BM, G, A]),
  pads: (m, s) => chordBars(m, s, [CHORD.Bm, CHORD.G, CHORD.A]),
};
