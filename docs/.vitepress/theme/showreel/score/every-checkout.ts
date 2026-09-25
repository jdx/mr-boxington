// Every checkout compiles again: a breakdown. The groove drops to a
// lowpassed heartbeat under long, dark bass notes on G, then A, while the
// section's cards run their builds. A placeholder until the section's own
// sounds are written: the three compile loops, the slab thuds, the riser.

import type { Part } from ".";
import { CHORD, heartbeat } from "./grooves";
import { bassRun, pad } from "./sounds";

export const part: Part = {
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
