// Section 8, "Another worktree". A stand-in (stub.ts) until the map scene
// is built: a worktree at another path computes the same key for `syn`,
// the matching crates restore in green, the edited crate compiles in amber,
// and the taped box with its strawberry holds as the poster.

import type { Caption } from "../type";
import { stubScene } from "./stub";

const CAPTIONS: readonly Caption[] = [
  {
    out: 6.25,
    lines: [
      { in: 1.5, text: "New worktree, new path," },
      { in: 2, text: "same keys." },
    ],
  },
  {
    out: 11.75,
    lines: [
      { in: 6.75, text: "Matching crates restore." },
      { in: 8.5, text: "Your edit compiles." },
    ],
  },
];

export const scene = stubScene({
  id: "another-worktree",
  learns:
    "A worktree at another path computes the same keys; matching crates restore and the edited crate compiles.",
  captions: () => CAPTIONS,
});
