// The landing page lists what each chapter of the reel shows, for readers
// who cannot watch it (describe.ts, under the player in HomeShowreel.vue).
// Its figures are the video's: every number a caption or the chart puts on
// screen is in the chapter's text, and the text states no number the video
// does not show, with the facts, with some withheld, and with none.

import assert from "node:assert/strict";
import { test } from "node:test";
import { describeChapters } from "../describe";
import { annotation, delta, factsFromBenchmarks, type ReelFacts, tenths } from "../facts";
import { scenes } from "../scenes";
import { SECTIONS, type SectionId } from "../timeline";
import { plain } from "../type";
import { hk, live, NOTHING, numbers } from "./published";

/**
 * The figures a chapter of the video shows under `f`: `told` are the ones
 * the page must state, `shown` the details it may leave out. Captions come
 * from the scenes; what the scenes draw besides them is the storyboard's
 * (Appendix B), through the helpers the scenes draw it with.
 */
function onScreen(id: SectionId, f: ReelFacts | null): { told: string[]; shown: string[] } {
  const scene = scenes.find((s) => s.id === id);
  const told = (scene?.captions?.(f) ?? [])
    .flatMap((c) => c.lines)
    .flatMap((l) => numbers(plain(l.text)));
  const shown: string[] = [];
  if (id === "first-build" && f?.toolchain) shown.push(...numbers(f.toolchain));
  if (id === "same-checkout" && f?.warm) {
    // The must-read line, the hit counter, and the source line.
    told.push(String(f.warm.hits), String(f.warm.lookups), tenths(f.warm.seconds));
    shown.push(String(f.warm.trials), ...numbers(f.versions.mbx ?? ""));
  }
  if (id === "six-builds" && f?.contention) {
    // The two bars, and the source line.
    told.push(String(f.contention.scheduled), String(f.contention.unscheduled));
    shown.push(String(f.contention.trials));
  }
  if (id === "next-push" && f?.commit) {
    // The chart's readouts, its delta and annotation, and the subtitle.
    const saved = delta(f.commit);
    told.push(tenths(f.commit.cargo), tenths(f.commit.mbx), ...numbers(annotation(f.commit)));
    if (saved) told.push(tenths(saved));
    shown.push(String(f.commit.trials));
  }
  // The rule "unused 30 days" is a default, not a measurement.
  if (id === "pruned") shown.push("30");
  return { told, shown };
}

/** Facts the page and the video meet: today's, the live file's, and every kind withheld. */
function cases(): [string, ReelFacts | null][] {
  const today = hk();
  const c = today.commit;
  assert.ok(c);
  return [
    ["today's run", today],
    ["the live results.json", factsFromBenchmarks(live())],
    ["no facts", null],
    ["no claims", NOTHING],
    ["no warm run", { ...today, warm: null }],
    ["no next push", { ...today, commit: null }],
    ["no contention", { ...today, contention: null }],
    ["a next push that compiled nothing", { ...today, commit: { ...c, hits: 354, misses: 0 } }],
    ["a next push mbx lost", { ...today, commit: { ...c, cargo: c.mbx, mbx: c.cargo } }],
    [
      "another project",
      {
        subject: "ripgrep",
        toolchain: "1.96.0",
        versions: { mbx: "2.0.0" },
        warm: { hits: 212, lookups: 215, seconds: 0.84, trials: 5 },
        commit: { ...c, cargo: 31.26, mbx: 7.04, trials: 5, hits: 180, lookups: 215, misses: 35 },
        contention: { scheduled: 16, unscheduled: 90, trials: 5 },
      },
    ],
  ];
}

test("the page describes every chapter, in the reel's order", () => {
  const described = describeChapters(hk());
  assert.deepEqual(
    described.map(({ id, label }) => ({ id, label })),
    SECTIONS.map(({ id, label }) => ({ id, label })),
  );
  for (const { id, text } of described) assert.ok(text.length > 60, `${id}: "${text}"`);
});

test("the page states the figures the video shows, and no others", () => {
  for (const [name, f] of cases()) {
    for (const { id, text } of describeChapters(f)) {
      const page = numbers(text);
      const { told, shown } = onScreen(id, f);
      for (const n of told) assert.ok(page.includes(n), `${name}, ${id}: the video shows ${n}, the page leaves it out: "${text}"`);
      for (const n of page) {
        assert.ok(told.includes(n) || shown.includes(n), `${name}, ${id}: the page says ${n}, the video does not: "${text}"`);
      }
    }
  }
});

test("today's chapters read the storyboard's figures", () => {
  const text = Object.fromEntries(describeChapters(hk()).map((c) => [c.id, c.text]));
  assert.match(text["same-checkout"], /restores 354 of 354 compilations from the store in 1\.3 seconds\./);
  assert.match(text["six-builds"], /peaked at 32 compilers running at once with the scheduler on, against 162 with it off\./);
  assert.match(
    text["next-push"],
    /Cargo alone takes 18\.9 seconds and Cargo with mbx takes 9\.2 seconds, 9\.6 seconds less, with 353 of 354 compilations restored and 1 compiled\./,
  );
});

test("without the chart the page points to the benchmarks instead", () => {
  const text = (f: ReelFacts | null) => describeChapters(f).find((c) => c.id === "next-push")?.text ?? "";
  assert.match(text(null), /mr-boxington\.jdx\.dev\/benchmarks/);
  assert.doesNotMatch(text(null), /bar chart/);
  // A next push that compiled nothing says so without a "0 compiled".
  const c = hk().commit;
  assert.ok(c);
  assert.match(text({ ...hk(), commit: { ...c, hits: 354, misses: 0 } }), /with 354 of 354 compilations restored\./);
  // Nor is there a saving to state when mbx was not faster.
  assert.doesNotMatch(text({ ...hk(), commit: { ...c, cargo: c.mbx, mbx: c.cargo } }), /less/);
});
