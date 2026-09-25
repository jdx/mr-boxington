// Same checkout, empty target/: the full groove returns. A placeholder until
// the section's own sounds are written: glint tings, the tape, the
// strawberry's plop and bell.

import type { Part } from ".";
import { A, bassBars, CHORD, chordBars, drumBars, FULL, G } from "./grooves";

export const part: Part = {
  drums: (m, s) => drumBars(m, s, [FULL]),
  bass: (m, s) => bassBars(m, s, [G, A]),
  pads: (m, s) => chordBars(m, s, [CHORD.G, CHORD.A]),
};
