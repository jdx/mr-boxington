// Under cargo build: the half-time groove starts, on D. A placeholder until
// the section's own sounds are written: the typing, a tock per rustc arrow,
// the lid's hum.

import type { Part } from ".";
import { bassBars, CHORD, chordBars, D, drumBars, HALF } from "./grooves";

export const part: Part = {
  drums: (m, s) => drumBars(m, s, [HALF]),
  bass: (m, s) => bassBars(m, s, [D]),
  pads: (m, s) => chordBars(m, s, [CHORD.D]),
};
