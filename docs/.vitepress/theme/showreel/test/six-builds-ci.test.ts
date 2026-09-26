// Six builds at once and CI (scenes/six-builds.ts, scenes/ci.ts): their
// copy is the storyboard's and the facts', the permit rail behaves like one
// pool served first come first served, the storyboard's beats hold, and the
// score sounds where the picture puts its hits.

import assert from "node:assert/strict";
import { test } from "node:test";
import { playScore } from "../audio";
import { BEAT, type ReelFacts, sec, WHIP } from "../bible";
import { WHIP_AT, WHIP_WIND } from "../map";
import * as ci from "../scenes/ci";
import * as six from "../scenes/six-builds";
import { plain } from "../type";
import { MockContext } from "./mock-audio";
import { hk, NOTHING } from "./published";

const S = sec("six-builds");
const C = sec("ci");
const b = (n: number) => n * BEAT;
const lines = (facts: ReelFacts | null) =>
  (six.scene.captions?.(facts) ?? []).map((c) => ({ out: c.out, lines: c.lines.map((l) => [l.in, plain(l.text)]) }));

test("six-builds' figure line is the contention peaks, and without them the first caption holds", () => {
  const first = [
    [1.5, "Six builds share"],
    [1.75, "one pool of permits."],
  ];
  assert.deepEqual(lines(hk()), [
    { out: 6.5, lines: first },
    { out: 11.75, lines: [[7.25, "32 compilers at peak, not 162."]] },
  ]);
  const peaks = { ...hk(), contention: { scheduled: 28, unscheduled: 140, trials: 3 } };
  assert.equal(lines(peaks)[1].lines[0][1], "28 compilers at peak, not 140.");
  for (const facts of [null, NOTHING, { ...hk(), contention: null }]) {
    assert.deepEqual(lines(facts), [{ out: 11.5, lines: first }]);
  }
});

test("ci's caption is the storyboard's, with or without facts", () => {
  for (const facts of [hk(), null]) {
    assert.deepEqual(
      (ci.scene.captions?.(facts) ?? []).map((c) => ({ out: c.out, lines: c.lines.map((l) => [l.in, l.text]) })),
      [
        {
          out: 11.75,
          lines: [
            [3.5, "In CI, pushes to main publish."],
            [5, "Pull requests only restore."],
          ],
        },
      ],
    );
  }
});

test("the rail is one pool of 32 permits, served first come first served, from the slam on", () => {
  const held = (t: number) => six.SCHEDULE.filter((c) => c.grant <= t && t < c.release);
  for (const c of six.SCHEDULE) {
    assert.ok(c.emit >= six.T_FLOW - 1e-9 || c.emit === six.T_SYN_CHECK, "no compiler starts before the rail lands");
    assert.ok(c.arrive >= c.emit && c.grant >= c.arrive - 1e-9 && c.release > c.grant, "each waits, runs, and finishes in order");
    assert.ok(c.slot >= 0 && c.slot < 32);
  }
  // No slot holds two compilations at once.
  for (let s = 0; s < 32; s++) {
    const runs = six.SCHEDULE.filter((c) => c.slot === s).sort((x, y) => x.grant - y.grant);
    for (let i = 1; i < runs.length; i++) assert.ok(runs[i].grant >= runs[i - 1].release - 1e-9, `slot ${s} is shared`);
  }
  // First come, first served: permits go out in the order compilations arrive.
  const queue = six.SCHEDULE.filter((c) => c.emit !== six.T_SYN_CHECK).sort((x, y) => x.arrive - y.arrive);
  for (let i = 1; i < queue.length; i++) assert.ok(queue[i].grant >= queue[i - 1].grant - 1e-9, "a later arrival went first");
  // The pool fills and stays full, with a line waiting, until it tips.
  for (let t = b(2.5); t < six.T_TIP; t += 0.05) {
    assert.ok(held(t).length >= 28, `only ${held(t).length} permits held at b${(t / BEAT).toFixed(2)}`);
  }
  assert.ok(six.SCHEDULE.some((c) => c.grant > c.arrive + 0.2), "nothing ever waits in line");
  assert.ok(six.DOCKS.every((d, i) => i === 0 || d.at >= six.DOCKS[i - 1].at));
});

test("six loops type out of step until the rail slams down, then share its beat", () => {
  const all = [0, 1, 2, 3, 4, 5].flatMap((j) => six.loopHits(j));
  for (let j = 0; j < 6; j++) {
    const hits = six.loopHits(j);
    assert.ok(hits.length >= 1, `job ${j} never plays`);
    for (const t of hits) assert.ok(t >= six.T_TYPE - 1e-9 && t < six.T_SLAM, `job ${j} plays at b${t / BEAT}`);
  }
  // No two jobs hit together: they are out of step.
  const sorted = [...all].sort((x, y) => x - y);
  for (let i = 1; i < sorted.length; i++) assert.ok(sorted[i] - sorted[i - 1] > 0.005, "two loops hit together");
});

test("six-builds keeps the storyboard's beats", () => {
  assert.equal(six.T_TYPE, b(0.25));
  assert.equal(six.T_TYPED, b(0.75));
  assert.equal(six.T_SLAM, b(1));
  assert.ok(six.T_SYN_CHECK >= b(2.5) && six.T_SYN_DONE <= b(4), "the shared compilation plays b2.5 to b4");
  assert.equal(six.T_TIP, b(6.5));
  assert.ok(six.T_BARS < b(7.25), "the bars are up before the figure line lands");
  assert.equal(six.T_MERGE, b(11.5));
  assert.ok(six.TILES.every((t) => t > 0 && t < six.T_TYPE));
  assert.equal(S.len, b(12));
});

test("ci keeps the storyboard's beats and whips out on map.ts's timing", () => {
  assert.ok(ci.T_LAND <= b(1.5) && ci.T_REMOTE <= b(1.5), "the hop and the drop are done by b1.5");
  assert.ok(Math.max(...ci.CHIPS) <= b(1.5), "the chips are in by b1.5");
  assert.equal(ci.UPLOADS[0], b(1.5));
  assert.ok(ci.UPLOADS.every((u) => u + ci.UPLOAD_FLY <= b(4)), "the main runner uploads from b1.5 to b3.5");
  assert.equal(ci.RESTORES[0], b(4.5));
  assert.ok(ci.RESTORES.every((r) => r + ci.RESTORE_FLY <= b(8)), "the pull request restores from b4.5 to b8");
  assert.ok(ci.T_THROW > b(4.5) && ci.T_BACK < b(8));
  assert.ok(Math.abs(C.at(ci.T_WIND) - (WHIP_AT - WHIP - WHIP_WIND)) < 1e-9);
});

test("the score sounds on the picture's hits", () => {
  const WHEN = 0.2;
  const ac = new MockContext();
  playScore(ac.context, ac.destination as AudioNode, 0, WHEN);
  const starts = ac.starts().map(({ t }) => t - WHEN);
  const near = (t: number, what: string) =>
    assert.ok(
      starts.some((s) => Math.abs(s - t) < 0.012),
      `nothing sounds at ${what} (${t.toFixed(3)} s)`,
    );
  near(S.at(six.T_SLAM), "the rail's slam");
  near(S.at(six.T_SYN_DONE), "the shared result");
  near(S.at(six.loopHits(2)[0]), "a loop's hit");
  near(C.at(ci.T_LAND), "the landing");
  near(C.at(ci.T_BOUNCE), "the read-only bounce");
  near(C.at(ci.UPLOADS[3] + ci.UPLOAD_FLY), "an upload landing");
  near(C.end, "the whip");
});
