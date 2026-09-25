import assert from "node:assert/strict";
import { test } from "node:test";
import { BAR, BEAT, CHAPTERS, DURATION, SECTIONS, sec } from "../bible";
import { scenes } from "../scenes";
import { PARTS } from "../score";

test("sections run end to end in whole bars, from 0 to DURATION", () => {
  let t = 0;
  for (const { id, bars } of SECTIONS) {
    const s = sec(id);
    assert.ok(Number.isInteger(bars) && bars > 0, `${id}: ${bars} bars`);
    assert.equal(s.start, t, `${id} starts where the previous section ends`);
    assert.equal(s.len, bars * BAR);
    assert.equal(s.end, s.start + s.len);
    t = s.end;
  }
  assert.equal(t, DURATION);
});

test("sec() places beats, bars, and local times from the section's start", () => {
  for (const { id } of SECTIONS) {
    const s = sec(id);
    assert.equal(s.beat(0), s.start);
    assert.equal(s.bar(0), s.start);
    assert.equal(s.at(0), s.start);
    assert.equal(s.beat(4 * s.bars), s.end);
    assert.equal(s.bar(s.bars), s.end);
    assert.equal(s.beat(1.5) - s.start, 1.5 * BEAT);
  }
  assert.throws(() => sec("nope" as never));
});

test("chapters mirror the sections", () => {
  assert.deepEqual(
    CHAPTERS,
    SECTIONS.map(({ id }) => {
      const { label, start, end } = sec(id);
      return { id, label, start, end };
    }),
  );
});

test("one scene per section, in order, on its section's span", () => {
  assert.deepEqual(
    scenes.map((s) => s.id),
    SECTIONS.map((s) => s.id),
  );
  for (const scene of scenes) {
    const s = sec(scene.id);
    assert.equal(scene.start, s.start, scene.id);
    assert.equal(scene.end, s.end, scene.id);
  }
});

test("every section has a part in the score", () => {
  assert.deepEqual(Object.keys(PARTS).sort(), SECTIONS.map((s) => s.id).sort());
});

test("the reel is the storyboard's fourteen sections: 35 bars, 65.625 s", () => {
  assert.deepEqual(
    SECTIONS.map(({ id, bars }) => `${id} ${bars}`),
    [
      "fold 1",
      "mr-boxington 2",
      "what 3",
      "every-checkout 3",
      "under-cargo-build 2",
      "first-build 3",
      "same-checkout 2",
      "another-worktree 3",
      "six-builds 3",
      "ci 3",
      "next-push 3",
      "pruned 3",
      "morph 1",
      "end 3",
    ],
  );
  assert.equal(DURATION, 65.625);
  assert.equal(sec("another-worktree").start, 30);
});
