// Section 5, "Under cargo build". A stand-in (stub.ts) until the map scene
// is built: `cargo build` types in a time-lapse terminal, Cargo's plan of
// hk's dependencies unfolds, and each crate fires a `rustc` arrow under the
// lid.

import type { Caption } from "../type";
import { stubScene } from "./stub";

const CAPTIONS: readonly Caption[] = [
  {
    out: 7.75,
    lines: [
      { in: 1.25, text: "Keep typing `cargo build`." },
      { in: 1.75, text: "mbx wraps each `rustc` call." },
    ],
  },
];

export const scene = stubScene({
  id: "under-cargo-build",
  learns: "The command does not change; mbx sits at each compiler call.",
  captions: () => CAPTIONS,
});
