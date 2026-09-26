// The next push's chart (scenes/s5-data.ts): its copy is the storyboard's
// and the facts', its delta and annotation are the page's, it claims nothing
// without a commit fact, and its beat map keeps the race honest: both bars
// leave together, mbx locks flush with Cargo's tip on b1.25, and the
// finished chart holds long enough to read before the page falls.

import assert from "node:assert/strict";
import { test } from "node:test";
import { BEAT, sec } from "../bible";
import { annotation, type CommitFact, delta, type ReelFacts, tenths } from "../facts";
import {
  CARD,
  CARGO_DONE,
  deltaText,
  mbxGrow,
  mbxLock,
  noteParts,
  SETTLE,
  STEP_PLAN,
  STRIP_SWEEP,
  subtitle,
  T,
  TITLE,
} from "../scenes/s5-data";
import { plain, readingTime, wordCount } from "../type";
import { hk, NOTHING } from "./published";

/** One frame of the 120 fps render. */
const FRAME = 1 / 120;

const commit = (): CommitFact => {
  const c = hk().commit;
  assert.ok(c, "today's run has a next push");
  return c;
};

test("the chart's copy is the storyboard's", () => {
  assert.equal(TITLE, "CI builds the next push");
  assert.deepEqual(subtitle(hk()), [
    "hk, a mid-size Rust CLI with C dependencies",
    "the cache holds the previous commit · Linux CI runner · median of 3",
  ]);
  const c = commit();
  assert.equal(tenths(c.cargo), "18.9");
  assert.equal(tenths(c.mbx), "9.2");
  // The delta comes from the raw medians, not the rounded readouts' 9.7.
  assert.equal(deltaText(c), "9.6");
  assert.equal(noteParts(c).filter(Boolean).join(", "), "353 of 354 restored, 1 compiled");
});

test("the annotation is facts.annotation, split where its colour changes", () => {
  const c = commit();
  for (const v of [c, { ...c, hits: 354, misses: 0 }, { ...c, lookups: 215, hits: 180, misses: 35 }]) {
    assert.equal(noteParts(v).filter(Boolean).join(", "), annotation(v));
  }
  assert.equal(noteParts({ ...c, hits: 354, misses: 0 })[1], null);
});

test("the delta is only claimed when mbx was a printable tenth faster", () => {
  const c = commit();
  assert.equal(deltaText({ ...c, cargo: c.mbx, mbx: c.cargo }), "");
  assert.equal(deltaText({ ...c, cargo: 9.26, mbx: 9.229 }), "");
  assert.equal(deltaText(c), tenths(delta(c) ?? 0));
});

test("the subject's description is hk's alone, and every figure falls back", () => {
  const other: ReelFacts = { ...hk(), subject: "ripgrep" };
  assert.deepEqual(subtitle(other), ["ripgrep · the cache holds the previous commit", "Linux CI runner · median of 3"]);
  // Without a commit fact the chart gives way to the card, which claims no number.
  for (const f of [null, NOTHING, { ...hk(), commit: null }]) {
    for (const line of [...subtitle(f), ...CARD]) assert.doesNotMatch(line, /\d/, `"${line}"`);
  }
  assert.deepEqual(subtitle(null), ["the cache holds the previous commit", "Linux CI runner"]);
});

test("the title and card hold for their reading time", () => {
  // The title is in from the whip (b0.5) and the annotation from b2; both
  // leave with the page on b11. The card's lines land on b1 and b1.25.
  const hold = (text: string, from: number) =>
    assert.ok(T.clear - from >= readingTime(wordCount(plain(text))), `"${text}" holds ${T.clear - from} s`);
  hold(TITLE, 0.5 * BEAT);
  hold(annotation(commit()), T.amber);
  hold(CARD[0], BEAT);
  hold(CARD[1], T.lock);
});

test("both bars leave together and meet as mbx locks on b1.25", () => {
  assert.equal(STEP_PLAN[0][0], T.race);
  // mbx first reaches its mark within a frame before b1.25, and the
  // readout locks before that.
  const lock = mbxLock();
  assert.ok(lock < T.lock && lock >= T.lock - FRAME - 1e-9, `lock at ${lock}`);
  assert.ok(mbxGrow(lock) < 1 && mbxGrow(T.lock) >= 1);
  // Cargo's step to the mark is at rest by then; its next waits for b1.5.
  const meet = STEP_PLAN[2];
  assert.ok(meet[1] < lock && STEP_PLAN[3][0] > T.lock);
  // Cargo's lurches run in order and on the grid of eighths.
  for (let i = 0; i < STEP_PLAN.length; i++) {
    const [s, e] = STEP_PLAN[i];
    assert.ok(Math.abs(s / (BEAT / 4) - Math.round(s / (BEAT / 4))) < 1e-9, `step ${i} starts off the grid`);
    assert.ok(e > s && (i === 0 || s >= STEP_PLAN[i - 1][1]), `step ${i}`);
  }
  assert.equal(STEP_PLAN[STEP_PLAN.length - 1][1], CARGO_DONE);
});

test("the beat map follows the storyboard, and the chart holds 3.87 s", () => {
  const s = sec("next-push");
  assert.ok(T.strip + STRIP_SWEEP < T.amber, "the green tiles are in before the compile lands");
  assert.ok(CARGO_DONE < T.delta && T.delta < T.label);
  assert.equal(T.label, 2.75 * BEAT);
  assert.equal(T.clear, 11 * BEAT);
  assert.ok(Math.abs(T.clear - T.label - 3.867) < 0.001, `holds ${T.clear - T.label} s`);
  // The cube is at rest on the handoff before the bar line.
  assert.ok(SETTLE < s.len && s.len - SETTLE < 0.1);
});
