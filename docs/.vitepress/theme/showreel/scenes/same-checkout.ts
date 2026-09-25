// Section 7, "Same checkout, empty target/". A stand-in (stub.ts) until the
// map scene is built: a benchmark card redrawn from the warm run's counts,
// its bar filling green over a fixed 2.5 beats, then taped, with a
// strawberry.

import type { ReelFacts } from "../bible";
import type { Caption } from "../type";
import { stubScene } from "./stub";

/** The warm run's result, or a line that claims no number. */
function restored(facts: ReelFacts | null): string {
  const warm = facts?.warm;
  return warm ? `${warm.hits} of ${warm.lookups} restored, ${warm.seconds.toFixed(1)} s.` : "restored, not recompiled.";
}

export const scene = stubScene({
  id: "same-checkout",
  learns: "Measured on hk: with the store warm, every compilation came back, and the mascot shows it.",
  captions: (facts): readonly Caption[] => [
    {
      out: 7.75,
      lines: [
        { in: 0.75, text: "Same checkout, empty `target/`:" },
        { in: 3.25, text: restored(facts) },
      ],
    },
  ],
  copy: ({ facts }) => [{ text: facts?.subject ? `${facts.subject} benchmark` : "benchmark", in: 0.5, out: 7.75 }],
});
