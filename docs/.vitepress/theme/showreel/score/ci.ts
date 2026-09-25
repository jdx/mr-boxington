// CI: half-time, and the whip pan into the chart. A placeholder until the
// section's own sounds are written: upload swooshes, restore sparkles, a
// thunk and a lock click.

import type { Part } from ".";
import { A, bassBars, CHORD, chordBars, D, drumBars, G, HALF } from "./grooves";
import { ad, line, type Mix, swell, sweep } from "./mix";
import { thump, whoosh } from "./sounds";

/** Whip pan: out to the left into the cut, in from the right after it. */
function whip(m: Mix, t: number): void {
  m.duck(t, 0.5, 0.12);
  const out = swell(t - 0.14, t - 0.09, 0.05, t - 0.004, 0.45, t + 0.012);
  whoosh(m, out, sweep(t - 0.14, 900, t, 4800), 0.9, { pan: line(t - 0.14, 0, t, -0.85) }, "white");
  const inn = ad(t - 0.01, t + 0.004, 0.42, t + 0.16);
  whoosh(m, inn, sweep(t - 0.01, 4800, t + 0.14, 900), 0.9, { pan: line(t - 0.01, 0.85, t + 0.14, 0), send: 0.1 }, "white");
  thump(m, t, 0.4, 90, 45, 0.18);
}

export const part: Part = {
  cues: (m, s) => whip(m, s.end),
  drums: (m, s) => drumBars(m, s, [HALF]),
  bass: (m, s) => bassBars(m, s, [D, G, A]),
  pads: (m, s) => chordBars(m, s, [CHORD.D, CHORD.G, CHORD.A]),
};
