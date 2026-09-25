// Section 9, "Six builds at once". A stand-in (stub.ts) until the map scene
// is built: the benchmark's six jobs type together, the permit rail lands
// before any compiler starts, one `syn` compilation serves two jobs, and the
// rail tips into the two runs' peaks.

import type { Caption } from "../type";
import { stubScene } from "./stub";

// The figure line is the storyboard's copy, standing in for the contention
// peaks until facts.ts carries them. Without them the scene drops the line
// and holds the first caption to b11.5.
const CAPTIONS: readonly Caption[] = [
  {
    out: 6.5,
    lines: [
      { in: 1.5, text: "Six builds share" },
      { in: 1.75, text: "one pool of permits." },
    ],
  },
  { out: 11.75, lines: [{ in: 7.25, text: "32 compilers at peak, not 162." }] },
];

export const scene = stubScene({
  id: "six-builds",
  learns: "Builds running together share one pool of permits, and identical compilations run once.",
  captions: () => CAPTIONS,
});
