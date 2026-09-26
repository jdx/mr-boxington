import type { Scene } from "../bible";
import { scene as anotherWorktree } from "./another-worktree";
import { scene as ci } from "./ci";
import { scene as everyCheckout } from "./every-checkout";
import { scene as firstBuild } from "./first-build";
import { scene as fold } from "./s1-unfold";
import { scene as mrBoxington } from "./s2-character";
import { scene as what } from "./s3-type";
import { scene as nextPush } from "./s5-data";
import { scene as pruned } from "./s6-world";
import { scene as morph } from "./s7-morph";
import { scene as end } from "./s8-logo";
import { scene as sameCheckout } from "./same-checkout";
import { scene as sixBuilds } from "./six-builds";
import { scene as underCargoBuild } from "./under-cargo-build";

/** One scene per section, in the timeline's order (bible.ts SECTIONS). */
export const scenes: Scene[] = [
  fold,
  mrBoxington,
  what,
  everyCheckout,
  underCargoBuild,
  firstBuild,
  sameCheckout,
  anotherWorktree,
  sixBuilds,
  ci,
  nextPush,
  pruned,
  morph,
  end,
];
