// first-build and same-checkout: each starts on the frame it inherits and
// settles onto the one it owes, the mascot's rules hold (a cold build ends
// matte and taped, a warm one rosy with its strawberry), the lid's four
// steps land before the tape, and the card's copy comes from the facts with
// nothing claimed without them. The frames themselves are checked by eye.

import assert from "node:assert/strict";
import { test } from "node:test";
import { BEAT, sec } from "../bible";
import type { BoxPose } from "../box";
import { BAR_CELLS, CAPTION_TOP, curveAt, FB_END, FLOOR, HOME, SC_END, segments, UC_END } from "../map";
import * as first from "../scenes/first-build";
import { bouncyLid, land, lidSteps, LID_DROP } from "../scenes/first-build-kit";
import * as same from "../scenes/same-checkout";
import { hk, NOTHING } from "./published";

const FIRST = sec("first-build");
const SAME = sec("same-checkout");
/** The last 120 fps frame of a section, section-local. */
const last = (s: typeof FIRST) => s.len - 1 / 120;

/** A pose as it draws: an unset lid is shut, and no callback or unit squash. */
function drawn(p: BoxPose | undefined): unknown {
  assert.ok(p);
  const { inside, squash, lid, ...rest } = p;
  assert.equal(inside, undefined, "nothing is dropping into the box");
  assert.ok(squash === undefined || squash === 1);
  return { ...rest, lid: lid ?? 0 };
}

test("first-build starts on under-cargo-build's frame and settles onto the cold finish", () => {
  const start = first.frameState(0);
  assert.deepEqual(start.cam, UC_END.cam);
  assert.deepEqual(drawn(start.pose), drawn(UC_END.box?.pose));
  assert.deepEqual(start.view, UC_END.pane?.view);
  assert.deepEqual(start.chips, UC_END.chips);
  for (const lt of [last(FIRST), FIRST.len - 1 / 60]) {
    const end = first.frameState(lt);
    assert.deepEqual(end.cam, HOME);
    assert.deepEqual(drawn(end.pose), drawn(FB_END.box?.pose));
    assert.ok(end.chips.every((c) => c === null), "Cargo's plan has gone into the box");
    const want = FB_END.pane?.view;
    assert.ok(want);
    assert.deepEqual(end.view.pose, want.pose);
    assert.equal(end.view.filled, BAR_CELLS);
    assert.deepEqual(end.view.status, want.status);
    // A cold store answers "not looked up": the bar is all grey.
    assert.deepEqual(segments(end.view.mix, BAR_CELLS), segments(want.mix, BAR_CELLS));
    assert.deepEqual(segments(end.view.mix, BAR_CELLS), [0, 0, BAR_CELLS]);
  }
});

test("same-checkout starts on the cold finish and ends on the bare benchmark card", () => {
  const start = same.frameState(0);
  assert.deepEqual(drawn(start.pose), drawn(FB_END.box?.pose));
  for (const lt of [last(SAME), SAME.len - 1 / 60]) {
    const end = same.frameState(lt);
    assert.deepEqual(drawn(end.pose), drawn(SC_END.box?.pose));
    const want = SC_END.pane?.view;
    assert.ok(want);
    assert.deepEqual(end.view.pose, want.pose);
    assert.equal(end.view.filled, BAR_CELLS);
    assert.deepEqual(segments(end.view.mix, BAR_CELLS), segments(want.mix, BAR_CELLS));
    assert.deepEqual(segments(end.view.mix, BAR_CELLS), [BAR_CELLS, 0, 0]);
  }
});

test("the cold build stays matte: no glint, no blush, no strawberry, grey all the way", () => {
  for (let lt = 0; lt < FIRST.len; lt += 1 / 60) {
    const { view, pose } = first.frameState(lt);
    assert.equal(view.pose.glint, null, `a glint at ${lt}`);
    assert.equal(view.pose.cheeks, 0);
    assert.equal(view.pose.strawberry, false);
    assert.equal(view.mix.hits + view.mix.misses, 0);
    assert.equal(pose.face?.cheeks, 0);
    assert.equal(pose.face?.strawberry ?? 0, 0);
    assert.ok(pose.face?.sweep === null || pose.face?.sweep === undefined);
  }
});

test("the warm build blushes at the first hit, glints only while hits land, and lands its strawberry after the tape", () => {
  const blush = same.T_BLUSH;
  assert.equal(same.frameState(blush - 1 / 120).pose.face?.cheeks ?? 0, 0);
  assert.equal(same.frameState(blush).pose.face?.cheeks, 3);
  const finish = same.T_FINISH;
  for (let lt = 0; lt < SAME.len; lt += 1 / 120) {
    const { pose, view } = same.frameState(lt);
    if (lt < blush || lt >= finish) assert.equal(view.pose.glint, null, `a glint at ${lt}`);
    if (lt < same.T_BERRY) assert.equal(view.pose.strawberry, false);
    if (lt >= same.T_BERRY) assert.ok(view.pose.strawberry && (pose.face?.strawberry ?? 0) > 0);
  }
  // The bar fills over a fixed 2.5 beats, whatever the benchmark counted.
  assert.ok(Math.abs(same.T_FILL1 - same.T_FILL0 - 2.5 * BEAT) < 1e-9);
  assert.equal(same.frameState(same.T_FILL0 - 1 / 120).view.filled, 0);
  assert.ok(same.frameState(same.T_FILL0 + BEAT).view.filled > 0);
  assert.ok(same.frameState(same.T_FILL1 - BEAT / 4).view.filled < BAR_CELLS);
  assert.equal(same.frameState(same.T_FILL1).view.filled, BAR_CELLS);
  assert.ok(same.SWEEPS.length >= 2, "the glint sweeps while the bar fills");
  assert.ok(same.SWEEPS.every((w) => w.at >= SAME.at(blush) && w.at < SAME.at(finish)));
});

test("each lid steps down four times with the mascot's, and is shut before the tape", () => {
  for (const [s, steps, tape, state] of [
    [FIRST, first.LID_STEPS, first.T_TAPE0, first.frameState],
    [SAME, same.LID_STEPS, same.T_TAPE0, same.frameState],
  ] as const) {
    assert.equal(steps.length, 4, `${s.id}: ${steps.length} steps`);
    assert.ok(steps.every((t) => t > s.start && t < s.at(tape)), `${s.id}: a step comes after the tape`);
    // Shut, bounce and all, on the frame before the tape starts to go on.
    assert.equal(state(tape - 1 / 120).pose.lid, 0, `${s.id}: the lid is still open`);
    assert.equal(bouncyLid(steps, s.at(tape) + LID_DROP), 0);
  }
  // first-build's last step is home well before its tape.
  assert.ok(first.LID_STEPS.every((t) => t + LID_DROP <= FIRST.at(first.T_TAPE0)));
  // first-build's steps come after the camera is back out, where the pane shows them.
  assert.ok(first.LID_STEPS.every((t) => t > FIRST.at(first.T_PULL1) - BEAT / 2));
  assert.deepEqual(lidSteps({ start: 0, total: 4, units: [] }, 1), []);
  assert.equal(land(0.5, 0, 0.5), 1);
});

test("the card's copy comes from the warm fact, and claims nothing without it", () => {
  const facts = hk();
  const copy = same.cardCopy(facts);
  assert.equal(copy.label, `${facts.subject} benchmark`);
  assert.equal(copy.hits, facts.warm?.hits);
  assert.equal(
    copy.source,
    `Linux CI runner · warm store, same checkout, target/ emptied · mbx ${facts.versions.mbx} · median of ${facts.warm?.trials} · build view redrawn from these counts`,
  );
  assert.equal(same.restored(facts), `${facts.warm?.hits} of ${facts.warm?.lookups} restored, 1.3 s.`);
  for (const none of [null, NOTHING]) {
    assert.deepEqual(same.cardCopy(none), { label: null, hits: null, source: null });
    assert.equal(same.restored(none), "restored, not recompiled.");
  }
  // Segments drop out when a run does not record them.
  const bare = same.cardCopy({ ...facts, subject: "", versions: { mbx: null }, warm: { hits: 3, lookups: 3, seconds: 1, trials: 1 } });
  assert.equal(bare.label, "benchmark");
  assert.equal(bare.source, "Linux CI runner · warm store, same checkout, target/ emptied · build view redrawn from these counts");
});

test("the score's anchors sit inside their sections", () => {
  const firstAnchors = [
    first.T_RING0,
    first.T_RING1,
    first.T_THROW,
    first.T_GULP,
    first.T_PUSH1,
    first.T_SHELF,
    ...first.T_PATHS,
    ...first.T_CHIPS,
    ...first.FLIPS.flat(),
    first.T_STAMP,
    ...first.T_NAMED,
    first.T_TAPE1,
  ];
  for (const t of firstAnchors) assert.ok(t >= 0 && t < FIRST.len, `first-build cue at ${t}`);
  // The paths finish flipping before the key stamps on.
  assert.ok(Math.max(...first.FLIPS.flat()) < first.T_STAMP);
  const sameAnchors = [same.T_FLIP1, same.T_RIP1, same.T_LIFT, same.T_BLUSH, same.T_TAPE1, same.T_BERRY, same.T_BLINK, same.T_OUT];
  for (const t of sameAnchors) assert.ok(t >= 0 && t < SAME.len, `same-checkout cue at ${t}`);
});

test("what the sections throw about stays above the captions' band", () => {
  // The stations are the kit's (map.test.ts); these sections add the
  // cartons dropping into the box and the restored outputs sparking into
  // target/, all above the floor.
  const low = (c: first.Flight["curve"]) => Math.max(...Array.from({ length: 33 }, (_, i) => curveAt(c, i / 32).y));
  for (const f of [first.SYN_FLIGHT, ...first.STREAM]) assert.ok(low(f.curve) <= FLOOR, `a carton drops to y ${low(f.curve)}`);
  for (const s of same.SPARKS) assert.ok(low(s.curve) <= FLOOR, `a spark reaches y ${low(s.curve)}`);
  assert.ok(FLOOR <= CAPTION_TOP);
  // At home, where the camera ends up, map px are screen px.
  assert.deepEqual(first.frameState(FIRST.len - 0.5).cam, HOME);
});
