// The benchmark runs the facts tests read: today's published run, frozen
// beside the tests so their figures do not move when a benchmark refresh
// lands, and the live results.json as the page and the renderer load it.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { factsFromBenchmarks, type ReelFacts } from "../facts";
import type { SectionId } from "../timeline";
import { plain } from "../type";
import { REPO, SHOWREEL } from "./repo";

/** Parsed JSON, which the tests garble on purpose. */
type Json = any;

/**
 * benchmarks/results.json as workflow run 35910301201 published it (mbx
 * 1.17.0 on hk). Parsed afresh on every call, so a test can garble its copy.
 */
export const published = (): Json => JSON.parse(readFileSync(join(SHOWREEL, "test/results-35910301201.json"), "utf8"));

/** The live benchmarks/results.json through the loaders' own check. */
export function live(): Json {
  try {
    const run = JSON.parse(readFileSync(join(REPO, "benchmarks/results.json"), "utf8"));
    // As benchmarks.data.ts and showreel-video.mjs load it.
    return [1, 2].includes(run.schema) && run.passed ? run : null;
  } catch {
    return null;
  }
}

/** One cell of a parsed run, for a test to garble. */
export function cell(run: Json, scenario: string, tool: string): Json {
  const found = run.scenarios.find((s: Json) => s.scenario === scenario)?.results.find((c: Json) => c.tool === tool);
  assert.ok(found, `no ${scenario}/${tool} cell`);
  return found;
}

/** The facts today's run gives. */
export function hk(): ReelFacts {
  const facts = factsFromBenchmarks(published());
  assert.ok(facts, "today's run gives no facts");
  return facts;
}

/** Facts that back no figure: what a scene meets when every claim is withheld. */
export const NOTHING: ReelFacts = {
  subject: "hk",
  toolchain: null,
  versions: { mbx: null },
  warm: null,
  commit: null,
  contention: null,
};

/**
 * The numbers in a text, "354" and "1.3", but not the 3 in "S3" or a
 * version's parts.
 */
export const numbers = (text: string): string[] => text.match(/(?<![\w.])\d+(?:\.\d+)?(?!\w|\.\d)/g) ?? [];

/**
 * The six-builds stand-in still hard-codes the storyboard's peaks, and its
 * scene owner switches it to facts.contention. Once it does, this matches
 * nothing and can go.
 */
export const standIn = (id: SectionId, text: string): boolean =>
  id === "six-builds" && plain(text) === "32 compilers at peak, not 162.";
