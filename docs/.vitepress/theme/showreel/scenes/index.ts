import type { Scene } from "../bible";
import { scene as unfold } from "./s1-unfold";
import { scene as character } from "./s2-character";
import { scene as type } from "./s3-type";
import { scene as flow } from "./s4-flow";
import { scene as data } from "./s5-data";
import { scene as world } from "./s6-world";
import { scene as morph } from "./s7-morph";
import { scene as logo } from "./s8-logo";

/** One scene per section, in the timeline's order (bible.ts SECTIONS). */
export const scenes: Scene[] = [unfold, character, type, flow, data, world, morph, logo];
