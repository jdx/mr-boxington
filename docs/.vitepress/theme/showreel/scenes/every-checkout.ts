// Section 4, "Every checkout compiles again". A stand-in (stub.ts) until the
// map scene is built: three cards run `cargo build` one after another and
// stitch their repeated `Compiling syn` rows, the local cards fold into a
// leaning tower of old `target/` slabs, and Mr Boxington pops in.

import type { Caption } from "../type";
import { stubScene } from "./stub";

const CAPTIONS: readonly Caption[] = [
  {
    out: 6,
    lines: [
      { in: 1, text: "Every checkout compiles" },
      { in: 1.25, text: "the same crates again." },
    ],
  },
  { out: 11.25, lines: [{ in: 7, text: "Old `target/` directories pile up." }] },
];

export const scene = stubScene({
  id: "every-checkout",
  learns:
    "Without a shared cache each checkout compiles the same crates, and old target/ directories stay.",
  captions: () => CAPTIONS,
  nodesIn: true,
});
