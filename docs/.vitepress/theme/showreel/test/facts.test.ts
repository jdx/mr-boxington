// The reel's figures come from the published benchmark run through facts.ts.
// Today's run gives the storyboard's figures. A run that is missing, failed,
// malformed or too noisy to call gives none, and then no caption shows a
// number.

import assert from "node:assert/strict";
import { test } from "node:test";
import {
  annotation,
  delta,
  factsFromBenchmarks,
  median,
  medianOf,
  type ReelFacts,
  separated,
  tenths,
} from "../facts";
import { scenes } from "../scenes";
import type { SectionId } from "../timeline";
import { plain } from "../type";
import { cell, hk, live, NOTHING, published, standIn } from "./published";

test("today's run gives the storyboard's figures", () => {
  const f = hk();
  assert.equal(f.subject, "hk");
  assert.equal(f.toolchain, "1.94.0");
  assert.deepEqual(f.versions, { mbx: "1.17.0" });

  // Same checkout, empty target/: "354 of 354 restored, 1.3 s."
  assert.deepEqual(f.warm, { hits: 354, lookups: 354, seconds: 1.254526143, trials: 3 });
  assert.equal(tenths(f.warm.seconds), "1.3");
  assert.equal(medianOf(f.warm.trials), "median of 3");

  // The next push: 18.9 s against 9.2 s, −9.6 s, and "353 of 354 restored,
  // 1 compiled" under the mbx bar.
  assert.deepEqual(f.commit, {
    cargo: 18.876873681,
    mbx: 9.229066562,
    runs: { cargo: [19.13781663, 18.279490596, 18.876873681], mbx: [9.313478745, 8.463068971, 9.229066562] },
    trials: 3,
    hits: 353,
    lookups: 354,
    misses: 1,
  });
  assert.equal(tenths(f.commit.cargo), "18.9");
  assert.equal(tenths(f.commit.mbx), "9.2");
  assert.equal(delta(f.commit), 9.6);
  assert.equal(annotation(f.commit), "353 of 354 restored, 1 compiled");

  // Six builds at once: "32 compilers at peak, not 162."
  assert.deepEqual(f.contention, { scheduled: 32, unscheduled: 162, trials: 3 });
});

test("the live results.json gives facts that hold together, or none", () => {
  // A refresh may publish a run too noisy to chart, which leaves the figures
  // out; it must never give ones that contradict each other.
  const f = factsFromBenchmarks(live());
  if (f?.warm) assert.ok(f.warm.hits > 0 && f.warm.hits <= f.warm.lookups);
  if (f?.commit) {
    const c = f.commit;
    assert.equal(c.hits + c.misses, c.lookups);
    assert.ok(separated(c.runs.cargo, c.runs.mbx));
    assert.equal(median(c.runs.cargo), c.cargo);
    assert.equal(median(c.runs.mbx), c.mbx);
  }
  if (f?.contention) assert.ok(f.contention.scheduled < f.contention.unscheduled);
});

test("separated() calls a gap only when it is wider than either tool's own spread", () => {
  const runs = (scenario: string, tool: string): number[] => cell(published(), scenario, tool).wall_durations_ns;
  // Today's next push: 9.6 s apart, each tool within 0.9 s of itself.
  assert.ok(separated(runs("commit", "cargo"), runs("commit", "mbx")));
  // Today's contention batches: 9.0 s apart, against a 4.1 s spread.
  assert.ok(separated(runs("contention", "mbx"), runs("contention", "mbx-unscheduled")));
  // Today's next push, mbx against kache: 0.35 s apart inside a 1.3 s spread.
  assert.ok(!separated(runs("commit", "mbx"), runs("commit", "kache")));
  // A gap as wide as the spread is still noise; a hair wider is not.
  assert.ok(!separated([1, 2, 3], [3, 4, 5]));
  assert.ok(separated([1, 2, 3], [3.5, 4.01, 5]));
  assert.ok(separated([3.5, 4.01, 5], [1, 2, 3]));
  // One run has no spread to compare against.
  assert.ok(!separated([1], [10, 11]));
  assert.ok(!separated([1], [10]));
});

test("medians are the published ones: the middle run, the lower middle of an even count", () => {
  assert.equal(median([3, 1, 2]), 2);
  assert.equal(median([4, 1, 3, 2]), 2);
  assert.equal(median([7]), 7);
});

test("the delta rounds the raw medians once, and appears only for a saving", () => {
  const c = hk().commit;
  assert.ok(c);
  assert.equal(delta(c), 9.6);
  // Subtracting the rounded readouts would say 9.7.
  assert.equal(Math.round((Number(tenths(c.cargo)) - Number(tenths(c.mbx))) * 10) / 10, 9.7);
  assert.equal(delta({ ...c, cargo: 9.33, mbx: 9.229 }), 0.1);
  // A saving that prints as 0.0 is none.
  assert.equal(delta({ ...c, cargo: 9.26, mbx: 9.229 }), null);
  assert.equal(delta({ ...c, cargo: c.mbx, mbx: c.cargo }), null);
});

test("the annotation drops ', 0 compiled' when nothing missed", () => {
  const c = hk().commit;
  assert.ok(c);
  assert.equal(annotation(c), "353 of 354 restored, 1 compiled");
  assert.equal(annotation({ ...c, hits: 354, misses: 0 }), "354 of 354 restored");
  assert.equal(annotation({ ...c, hits: 340, misses: 14 }), "340 of 354 restored, 14 compiled");
  assert.equal(medianOf(1), null);
});

test("a missing, failed or unreadable run gives no facts", () => {
  const unreadable: unknown[] = [
    null,
    undefined,
    {},
    "results",
    42,
    [],
    { passed: true },
    { passed: true, scenarios: "warm" },
    { passed: true, scenarios: [] },
    { passed: true, scenarios: [null, 7, { scenario: "warm", results: "mbx" }, { scenario: "commit" }] },
  ];
  for (const data of unreadable) assert.equal(factsFromBenchmarks(data as never), null, JSON.stringify(data));

  const failed = published();
  failed.passed = false;
  assert.equal(factsFromBenchmarks(failed), null);
  const unsaid = published();
  delete unsaid.passed;
  assert.equal(factsFromBenchmarks(unsaid), null);

  // Every claim garbled at once leaves nothing to claim.
  const garbled = published();
  for (const s of garbled.scenarios) for (const c of s.results) c.wall_duration_ns = "fast";
  assert.equal(factsFromBenchmarks(garbled), null);
});

type Claim = "warm" | "commit" | "contention";
const GARBLED: [Claim, string, (run: ReturnType<typeof published>) => void][] = [
  ["warm", "restored more than it looked up", (r) => { cell(r, "warm", "mbx").stats.hits = 355; }],
  ["warm", "restored nothing", (r) => { cell(r, "warm", "mbx").stats.hits = 0; }],
  ["warm", "counts that are not numbers", (r) => { cell(r, "warm", "mbx").stats.lookups = "354"; }],
  ["warm", "no counts", (r) => { delete cell(r, "warm", "mbx").stats; }],
  ["warm", "a timing that is not its median run", (r) => { cell(r, "warm", "mbx").wall_duration_ns = 1300000000; }],
  ["warm", "a run count that disagrees with its runs", (r) => { cell(r, "warm", "mbx").trials = 4; }],
  ["warm", "a negative run", (r) => { cell(r, "warm", "mbx").wall_durations_ns[0] = -1; }],
  ["commit", "counts that do not add up", (r) => { cell(r, "commit", "mbx").stats.misses = 2; }],
  ["commit", "no misses", (r) => { delete cell(r, "commit", "mbx").stats.misses; }],
  ["commit", "nothing looked up", (r) => { cell(r, "commit", "mbx").stats = { lookups: 0, hits: 0, misses: 0 }; }],
  ["commit", "tools within each other's noise", (r) => {
    Object.assign(cell(r, "commit", "cargo"), { wall_duration_ns: 9300000000, wall_durations_ns: [9300000000, 8900000000, 9600000000] });
  }],
  ["commit", "one run per tool", (r) => {
    for (const tool of ["cargo", "mbx"]) {
      delete cell(r, "commit", tool).wall_durations_ns;
      delete cell(r, "commit", tool).trials;
    }
  }],
  ["commit", "tools run a different number of times", (r) => {
    Object.assign(cell(r, "commit", "cargo"), { trials: 2, wall_durations_ns: [19137816630, 18876873681] });
  }],
  ["commit", "no Cargo row", (r) => {
    const s = r.scenarios.find((x: { scenario: string }) => x.scenario === "commit");
    s.results = s.results.filter((c: { tool: string }) => c.tool !== "cargo");
  }],
  ["contention", "a scheduled peak no lower", (r) => { cell(r, "contention", "mbx").peak_compilers = 162; }],
  ["contention", "more compilers than permits", (r) => { cell(r, "contention", "mbx").permits = 16; }],
  ["contention", "a peak that is not a count", (r) => { cell(r, "contention", "mbx").peak_compilers = 32.5; }],
  ["contention", "no compiler seen", (r) => { cell(r, "contention", "mbx").peak_compilers = 0; }],
  ["contention", "batches within each other's noise", (r) => {
    Object.assign(cell(r, "contention", "mbx-unscheduled"), {
      wall_duration_ns: 40000000000,
      wall_durations_ns: [38000000000, 40000000000, 42000000000],
    });
  }],
  ["contention", "no unscheduled batch", (r) => { cell(r, "contention", "mbx-unscheduled").tool = "mbx-other"; }],
];

test("a garbled or inconclusive claim is withheld, and only that one", () => {
  const today = hk();
  for (const [claim, what, garble] of GARBLED) {
    const run = published();
    garble(run);
    const f = factsFromBenchmarks(run);
    assert.ok(f, `${claim} with ${what}: every claim went`);
    assert.equal(f[claim], null, `${claim} with ${what} still claims ${JSON.stringify(f[claim])}`);
    for (const other of ["warm", "commit", "contention"] as const) {
      if (other !== claim) assert.deepEqual(f[other], today[other], `${claim} with ${what} changed ${other}`);
    }
  }
});

test("names and versions print only when they are plain", () => {
  const facts = (edit: (run: ReturnType<typeof published>) => void): ReelFacts => {
    const run = published();
    edit(run);
    const f = factsFromBenchmarks(run);
    assert.ok(f);
    return f;
  };
  assert.equal(facts((r) => (r.toolchain = "rustc 1.94.0 (4a4ef493e 2026-03-02)")).toolchain, null);
  assert.equal(facts((r) => delete r.toolchain).toolchain, null);
  assert.equal(facts((r) => (r.toolchain = "1.96.0-nightly")).toolchain, "1.96.0-nightly");
  assert.deepEqual(facts((r) => (r.versions.mbx = "mbx 1.17.0")).versions, { mbx: null });
  assert.deepEqual(facts((r) => (r.versions.mbx = null)).versions, { mbx: null });
  assert.deepEqual(facts((r) => delete r.versions).versions, { mbx: null });
  assert.equal(facts((r) => (r.subject = "hk\n<script>")).subject, "");
  assert.equal(facts((r) => (r.subject = 7)).subject, "");
});

/** A scene's caption lines under `facts`, as plain text. */
function lines(id: SectionId, facts: ReelFacts | null): string[] {
  const scene = scenes.find((s) => s.id === id);
  return (scene?.captions?.(facts) ?? []).flatMap((c) => c.lines.map((l) => plain(l.text)));
}

test("no caption shows a number the facts do not back", () => {
  const today = hk();
  const every = scenes.map((s) => s.id);
  const withheld: [string, ReelFacts | null, readonly SectionId[]][] = [
    ["no facts", null, every],
    ["no claims", NOTHING, every],
    ["no warm run", { ...today, warm: null }, ["same-checkout"]],
    ["no next push", { ...today, commit: null }, ["next-push"]],
    ["no contention", { ...today, contention: null }, ["six-builds"]],
  ];
  for (const [what, facts, ids] of withheld) {
    for (const id of ids) {
      for (const line of lines(id, facts)) {
        if (standIn(id, line)) continue;
        assert.doesNotMatch(line, /\d/, `${id} with ${what}: "${line}"`);
      }
    }
  }
});
