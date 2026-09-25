import assert from "node:assert/strict";
import { test } from "node:test";
import { playScore } from "../audio";
import { DURATION, SECTIONS, sec } from "../bible";
import { GAP } from "../score/morph";
import { MockContext } from "./mock-audio";

/** The context time reel time 0 plays at, as the MP4 renderer schedules it. */
const WHEN = 0.2;

function render(from = 0): MockContext {
  const ac = new MockContext();
  playScore(ac.context, ac.destination as AudioNode, from, WHEN);
  return ac;
}

test("the score starts every source inside the reel, and sounds in every section", () => {
  const starts = render().starts();
  for (const { target, t } of starts) {
    // Sources start a few milliseconds early, ahead of the compressor's lookahead.
    const reel = t - WHEN;
    assert.ok(reel > -0.01 && reel < DURATION, `${target} starts at ${reel}`);
  }
  for (const { id } of SECTIONS) {
    const s = sec(id);
    assert.ok(
      starts.some(({ t }) => t - WHEN >= s.start - 0.01 && t - WHEN < s.end - 0.01),
      `nothing sounds in ${id}`,
    );
  }
});

test("nothing starts in the breath before the end card's downbeat", () => {
  const resolve = sec("logo").start;
  const inGap = render()
    .starts()
    .filter(({ t }) => t - WHEN > GAP && t - WHEN < resolve - 0.01);
  assert.deepEqual(inGap, []);
});

test("the score schedules the same calls every time, from any start", () => {
  for (const from of [0, 4.2, 11]) {
    assert.deepEqual(render(from).calls, render(from).calls, `from ${from}`);
  }
});
