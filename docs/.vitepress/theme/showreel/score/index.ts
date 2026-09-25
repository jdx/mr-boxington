// The score, section by section. Each section's share lives in its own
// module, keyed by its section id (the kept sections' modules keep their
// scenes' names: unfold, character, type, data, world, logo) and written
// from the section's origin (`sec(id)`), so lengthening or inserting a
// section moves its sounds with it. Where a scene times its motion with a
// formula (the tape, the Data bars, the cube rain, the prune beam), its
// module imports that formula or anchor from the scene, so the sound lands
// where the picture does.

import { SECTIONS, type Section, type SectionId, sec } from "../bible";
import { part as anotherWorktree } from "./another-worktree";
import { part as character } from "./character";
import { part as ci } from "./ci";
import { part as data } from "./data";
import { part as everyCheckout } from "./every-checkout";
import { part as firstBuild } from "./first-build";
import { part as logo } from "./logo";
import type { Mix } from "./mix";
import { part as morph } from "./morph";
import { part as sameCheckout } from "./same-checkout";
import { part as sixBuilds } from "./six-builds";
import { part as type } from "./type";
import { part as underCargoBuild } from "./under-cargo-build";
import { part as unfold } from "./unfold";
import { part as world } from "./world";

/** One section's share of the score. Every layer is optional. */
export interface Part {
  /** Sound design: a sound for each accent the section's picture choreographs. */
  cues?(m: Mix, s: Section): void;
  /** The groove, and the kicks the bass and pads pump with. */
  drums?(m: Mix, s: Section): void;
  bass?(m: Mix, s: Section): void;
  pads?(m: Mix, s: Section): void;
}

/** Every section's part. A new section needs an entry, even an empty one. */
export const PARTS: Record<SectionId, Part> = {
  fold: unfold,
  "mr-boxington": character,
  what: type,
  "every-checkout": everyCheckout,
  "under-cargo-build": underCargoBuild,
  "first-build": firstBuild,
  "same-checkout": sameCheckout,
  "another-worktree": anotherWorktree,
  "six-builds": sixBuilds,
  ci,
  "next-push": data,
  pruned: world,
  morph,
  end: logo,
};

/** The layers, built in this order: the effects first, then the music under them. */
const LAYERS = ["cues", "drums", "bass", "pads"] as const;

/** Build one pass of the whole score into `m`. */
export function compose(m: Mix): void {
  for (const layer of LAYERS) {
    for (const { id } of SECTIONS) PARTS[id][layer]?.(m, sec(id));
  }
}
