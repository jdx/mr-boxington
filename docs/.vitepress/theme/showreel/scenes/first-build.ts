// Section 6, "First build fills the store". A stand-in (stub.ts) until the
// map scene is built: `rustc` compiles `syn`, its carton arcs in under the
// lid, a cutaway shows the key forming with the checkout paths rewritten,
// and the cold build finishes taped, with no strawberry.

import type { Caption } from "../type";
import { stubScene } from "./stub";

const CAPTIONS: readonly Caption[] = [
  {
    out: 5.5,
    lines: [
      { in: 0.75, text: "First build: `rustc` compiles," },
      { in: 1, text: "mbx stores." },
    ],
  },
  {
    out: 11.75,
    lines: [
      { in: 6.75, text: "Keyed by inputs," },
      { in: 7, text: "not by checkout path." },
    ],
  },
];

export const scene = stubScene({
  id: "first-build",
  learns:
    "The first build compiles as usual; mbx stores each output under a key of its inputs, with checkout paths replaced.",
  captions: () => CAPTIONS,
});
