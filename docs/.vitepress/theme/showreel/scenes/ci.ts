// Section 10, "CI". A stand-in (stub.ts) until the map scene is built: a
// remote cache drops in with its backend chips, the `push to main` runner
// sends compiled cartons up, the `pull request` runner gets restored ones
// down, and the whip pan hands over to the chart.

import type { Caption } from "../type";
import { stubScene } from "./stub";

const CAPTIONS: readonly Caption[] = [
  {
    out: 11.75,
    lines: [
      { in: 3.5, text: "In CI, pushes to main publish." },
      { in: 5, text: "Pull requests only restore." },
    ],
  },
];

export const scene = stubScene({
  id: "ci",
  learns: "CI shares outputs through a remote backend; pushes to main write, pull requests only read.",
  captions: () => CAPTIONS,
  copy: () => [
    { text: "GitHub Actions cache", in: 1.5, out: 11.75 },
    { text: "cache server", in: 1.5, out: 11.75 },
    { text: "S3 bucket", in: 1.5, out: 11.75 },
  ],
  whipOut: true,
});
