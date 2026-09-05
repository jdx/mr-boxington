<template>
  <div v-if="!data" class="bench-empty">
    <p>
      No benchmark run has been published yet. The
      <a
        href="https://github.com/jdx/mr-boxington/actions/workflows/bench-refresh.yml"
        >bench-refresh workflow</a
      >
      opens a pull request with the numbers, and they appear here once it
      merges.
    </p>
  </div>
  <div v-else class="bench">
    <nav v-if="tiles.length" class="bench-summary" aria-label="At a glance">
      <a
        v-for="tile in tiles"
        :key="tile.id"
        :href="`#${tile.id}`"
        class="bench-tile"
        :class="{ 'is-ahead': tile.ahead }"
      >
        <span class="bench-tile-name">{{ tile.name }}</span>
        <span class="bench-tile-value">{{ tile.value }}</span>
        <span class="bench-tile-note">{{ tile.note }}</span>
      </a>
    </nav>

    <section
      v-for="card in cards"
      :id="card.id"
      :key="card.id"
      class="bench-card"
    >
      <header class="bench-head">
        <div>
          <h3 class="bench-title">
            <span class="bench-kicker">{{ card.name }}</span>
            {{ card.title }}
          </h3>
          <p class="bench-caption">{{ card.caption }}</p>
        </div>
        <span class="bench-axis">wall clock · lower is better</span>
      </header>

      <table v-if="card.rows.length" class="bench-table">
        <thead class="bench-sr-only">
          <tr>
            <th scope="col">Tool</th>
            <th scope="col">Relative wall clock</th>
            <th scope="col">Median and range</th>
          </tr>
        </thead>
        <tbody>
          <tr
            v-for="row in card.rows"
            :key="row.tool"
            :class="{ 'is-mbx': row.mbx }"
            :title="row.tooltip"
          >
            <th scope="row" class="bench-tool">
              <span class="bench-tool-line">
                <span class="bench-tool-name">{{ row.label }}</span>
                <span v-if="row.tag" class="bench-tag">{{ row.tag }}</span>
                <span v-if="row.fastest" class="bench-tag is-fastest"
                  >fastest</span
                >
              </span>
              <small v-if="row.note" class="bench-sub">{{ row.note }}</small>
            </th>
            <td class="bench-bar-cell">
              <div class="bench-track" aria-hidden="true">
                <span
                  class="bench-bar"
                  :class="{ 'is-mbx': row.mbx }"
                  :style="{ width: `${row.width}%` }"
                />
                <span
                  v-if="row.range"
                  class="bench-range"
                  :style="{
                    left: `${row.range.left}%`,
                    width: `${row.range.width}%`,
                  }"
                />
              </div>
            </td>
            <td class="bench-time">
              {{ row.time }}
              <small v-if="row.spread" class="bench-sub">{{
                row.spread
              }}</small>
            </td>
          </tr>
        </tbody>
      </table>

      <div class="bench-foot">
        <p v-if="card.verdict" class="bench-verdict">{{ card.verdict }}</p>
        <p v-for="line in card.notes" :key="line" class="bench-footnote">
          {{ line }}
        </p>
        <p v-if="card.skipped" class="bench-footnote">
          Not measured: {{ card.skipped }}
        </p>
        <p v-if="card.provenance" class="bench-footnote">
          Measured separately on {{ card.provenance.runner }}.
          <a :href="card.provenance.url">View run</a>.
        </p>
      </div>
    </section>

    <p class="bench-provenance">
      Measured on {{ data.platform }} ({{ data.runner }}) with Rust
      {{ data.toolchain }}, building <code>{{ data.subject }}</code> at
      <code>{{ data.revision.slice(0, 7) }}</code> under {{ versionList }}.
      <template v-if="data.workflow_run">
        <a
          :href="`https://github.com/jdx/mr-boxington/actions/runs/${data.workflow_run}`"
          >The run that produced them</a
        >
        has the per-build logs.
      </template>
    </p>
  </div>
</template>

<script setup lang="ts">
import { computed } from "vue";
import { data } from "../benchmarks.data";
import type { BenchmarkCell, BenchmarkScenario } from "../benchmarks.data";

// Page copy for the scenarios the harness knows. A scenario it does not know
// falls back to the description the harness wrote into results.json.
const COPY: Record<string, { title: string; caption: string }> = {
  warm: {
    title: "CI rebuilds a commit it has already built",
    caption:
      "The store is warm from an earlier build of the same commit and target/ is empty. Cargo is left out: with nothing to reuse it would repeat that earlier build.",
  },
  commit: {
    title: "CI builds the next push",
    caption:
      "The caches were warmed at the parent commit and build the child. Cargo has no cache to restore, so its row is the uncached build.",
  },
  edit: {
    title: "One line changed, rebuilt in place",
    caption:
      "The local edit loop with incremental compilation on. Cargo's own incremental rebuild is the thing to beat here, not a control.",
  },
  contention: {
    title: "Six CI jobs on one runner",
    caption:
      "Overlapping check, Clippy, and test-compilation jobs. Run one after another, then all at once with and without mbx's machine-wide compiler limit.",
  },
};

const CONTENTION_LABELS: Record<string, { label: string; tag: string | null }> =
  {
    "mbx-sequential": { label: "sequential", tag: null },
    "mbx-unscheduled": { label: "parallel", tag: "scheduler off" },
    mbx: { label: "parallel", tag: "mbx scheduler" },
  };

function seconds(ns: number) {
  const s = ns / 1e9;
  if (s >= 60) return `${Math.floor(s / 60)}m ${(s % 60).toFixed(0)}s`;
  return `${s.toFixed(1)}s`;
}

// A margin the fastest rule rejects can be smaller than a tenth, and "0.0s"
// reads as a rounding error rather than as the reason nothing is marked.
function fine(ns: number) {
  const s = ns / 1e9;
  return `${s < 0.1 ? s.toFixed(2) : s.toFixed(1)}s`;
}

function trials(cell: BenchmarkCell) {
  return cell.wall_durations_ns ?? [cell.wall_duration_ns];
}

// How far one tool moved between its own repeats: the noise floor, measured
// on the same machine, in the same scenario, minutes apart.
function spread(cell: BenchmarkCell) {
  const t = trials(cell);
  return t.length < 2 ? 0 : Math.max(...t) - Math.min(...t);
}

// Two results are separated when the gap between them is wider than either
// tool's own spread. Anything closer is noise, and a single trial has no
// spread to compare against, so it separates nothing.
function separated(a: BenchmarkCell, b: BenchmarkCell) {
  if (trials(a).length < 2 || trials(b).length < 2) return false;
  const margin = Math.abs(a.wall_duration_ns - b.wall_duration_ns);
  return margin > Math.max(spread(a), spread(b));
}

function byTime(cells: BenchmarkCell[]) {
  return [...cells].sort((a, b) => a.wall_duration_ns - b.wall_duration_ns);
}

function name(cell: BenchmarkCell) {
  return cell.tool === "cargo" ? "Cargo" : cell.tool;
}

function ratio(x: number) {
  return `${x.toFixed(2)}×`;
}

// Bars and whiskers share one scale per card: the slowest single trial fills
// the track. Cards are never scaled against each other, since each answers
// which tool was faster on its own workload.
function scale(cells: BenchmarkCell[]) {
  return Math.max(...cells.flatMap(trials), 1);
}

function geometry(cell: BenchmarkCell, max: number) {
  const t = trials(cell);
  const range =
    t.length < 2
      ? null
      : {
          left: (Math.min(...t) / max) * 100,
          width: ((Math.max(...t) - Math.min(...t)) / max) * 100,
        };
  return {
    width: Math.max(1, (cell.wall_duration_ns / max) * 100),
    range,
    spread: range
      ? `${(Math.min(...t) / 1e9).toFixed(1)}–${(Math.max(...t) / 1e9).toFixed(1)}s`
      : "",
    tooltip: `${t.length} ${t.length === 1 ? "run" : "runs"}: ${t
      .map(seconds)
      .join(", ")}`,
  };
}

interface Row {
  tool: string;
  label: string;
  tag: string | null;
  fastest: boolean;
  mbx: boolean;
  width: number;
  range: { left: number; width: number } | null;
  time: string;
  spread: string;
  note: string;
  tooltip: string;
}

interface Card {
  id: string;
  name: string;
  title: string;
  caption: string;
  rows: Row[];
  verdict: string | null;
  notes: string[];
  skipped: string;
  provenance: { runner: string; url: string } | null;
}

interface Tile {
  id: string;
  name: string;
  value: string;
  note: string;
  ahead: boolean;
}

function buildCard(scenario: BenchmarkScenario): {
  card: Card;
  tile: Tile | null;
} {
  const copy = COPY[scenario.scenario];
  const cells = byTime(scenario.results);
  const max = scale(cells);
  const baseline = cells.find((c) => c.tool === "cargo") ?? null;
  const baselineTag = baseline
    ? (scenario.baseline ?? "uncached baseline").replace(/ baseline$/, "")
    : null;
  const [best, next] = cells;
  const decisive = best && next ? separated(best, next) : false;

  const rows: Row[] = cells.map((cell) => {
    const g = geometry(cell, max);
    let note = "";
    if (baseline && cell !== baseline) {
      const r = baseline.wall_duration_ns / cell.wall_duration_ns;
      note = separated(cell, baseline)
        ? r >= 1
          ? `${ratio(r)} faster than Cargo`
          : `${fine(cell.wall_duration_ns - baseline.wall_duration_ns)} behind Cargo`
        : "level with Cargo";
    }
    return {
      tool: cell.tool,
      label: name(cell),
      tag: cell === baseline ? baselineTag : null,
      fastest: decisive && cell === best,
      mbx: cell.tool === "mbx",
      time: seconds(cell.wall_duration_ns),
      note,
      ...g,
    };
  });

  // The verdict names a fastest tool only across a gap wider than the noise.
  let verdict: string | null;
  if (cells.length === 0) verdict = null;
  else if (cells.length === 1)
    verdict = `Only ${name(best)} completed this scenario, so nothing is marked fastest.`;
  else if (trials(best).length < 2 || trials(next).length < 2)
    verdict = "Measured once per tool, so nothing is marked fastest.";
  else if (decisive)
    verdict =
      `${name(best)} fastest, ${fine(next.wall_duration_ns - best.wall_duration_ns)} ahead of ${name(next)}. ` +
      `That lead is wider than either tool's own range across ${trials(best).length} runs.`;
  else
    verdict =
      `Too close to call: ${name(best)} and ${name(next)} finished ${fine(next.wall_duration_ns - best.wall_duration_ns)} apart, ` +
      `inside the ${fine(Math.max(spread(best), spread(next)))} one of them moved across its own runs.`;

  const mbx = cells.find((c) => c.tool === "mbx") ?? null;
  if (verdict && mbx && mbx !== best && mbx !== next) {
    verdict += separated(mbx, best)
      ? ` mbx finished ${fine(mbx.wall_duration_ns - best.wall_duration_ns)} behind ${name(best)}.`
      : " mbx finished within that same noise.";
  }

  const notes: string[] = [];
  const restored = cells.filter(
    (c) => c.stats?.hits !== undefined && c.stats?.restored_output_files,
  );
  for (const c of restored) {
    notes.push(
      `${name(c)} restored ${c.stats!.restored_output_files!.toLocaleString("en-US")} output files on ${c.stats!.hits!.toLocaleString("en-US")} cache hits.`,
    );
  }
  const warm = cells.filter((c) => c.warmup_wall_duration_ns);
  if (warm.length) {
    notes.push(
      "First edit after a build, before the loop settles: " +
        warm
          .map((c) => `${name(c)} ${seconds(c.warmup_wall_duration_ns!)}`)
          .join(", ") +
        ".",
    );
  }

  // The tile answers the page's question in one line: where did mbx land.
  let tile: Tile | null = null;
  if (mbx) {
    const other =
      baseline ?? cells.find((c) => c !== mbx && c.tool !== "cargo") ?? null;
    let note = "";
    if (other) {
      const r = other.wall_duration_ns / mbx.wall_duration_ns;
      note = !separated(mbx, other)
        ? `level with ${name(other)}`
        : r >= 1
          ? `${ratio(r)} faster than ${name(other)}`
          : `${fine(mbx.wall_duration_ns - other.wall_duration_ns)} behind ${name(other)}`;
    }
    tile = {
      id: scenario.scenario,
      name: scenario.scenario,
      value: seconds(mbx.wall_duration_ns),
      note,
      ahead: decisive && best === mbx,
    };
  }

  return {
    card: {
      id: scenario.scenario,
      name: scenario.scenario,
      title: copy?.title ?? scenario.scenario,
      caption: copy?.caption ?? scenario.description,
      rows,
      verdict,
      notes,
      skipped: scenario.skipped.join("; "),
      provenance: provenance(scenario),
    },
    tile,
  };
}

// Contention compares one binary against itself, so it asks a different
// question: did running the jobs at once beat running them in turn, and did
// the scheduler get there by sharing the machine or by oversubscribing it.
function contentionCard(scenario: BenchmarkScenario): {
  card: Card;
  tile: Tile | null;
} {
  const copy = COPY[scenario.scenario];
  const cells = scenario.results;
  const max = scale(cells);
  const scheduled = cells.find((c) => c.tool === "mbx") ?? null;
  const unscheduled = cells.find((c) => c.tool === "mbx-unscheduled") ?? null;
  const sequential = cells.find((c) => c.tool === "mbx-sequential") ?? null;
  const fastest = byTime(cells)[0];
  const decisive =
    scheduled && unscheduled ? separated(scheduled, unscheduled) : false;

  const rows: Row[] = cells.map((cell) => {
    const g = geometry(cell, max);
    const labels = CONTENTION_LABELS[cell.tool] ?? {
      label: cell.tool,
      tag: null,
    };
    const peak = cell.peak_compilers ?? 0;
    const compilers = cell.permits
      ? `${peak} of ${cell.permits} permits used`
      : `${peak} compilers at peak`;
    // Only Linux reports free memory, and only Linux runs this benchmark.
    // Zero would be a real reading (a machine that ran itself out), so it is
    // never folded into the "not measured" case.
    const memory =
      cell.min_available_bytes === null ||
      cell.min_available_bytes === undefined
        ? null
        : `${(cell.min_available_bytes / 1e9).toFixed(1)} GB free at the low`;
    return {
      tool: cell.tool,
      label: labels.label,
      tag: labels.tag,
      fastest: decisive && cell === fastest && cell === scheduled,
      mbx: cell.tool === "mbx",
      time: seconds(cell.wall_duration_ns),
      note: [compilers, memory].filter(Boolean).join(", "),
      ...g,
    };
  });

  let verdict: string | null = null;
  if (scheduled && unscheduled) {
    const gap = unscheduled.wall_duration_ns - scheduled.wall_duration_ns;
    if (trials(scheduled).length < 2 || trials(unscheduled).length < 2)
      verdict = "Measured once per row, so nothing is marked fastest.";
    else if (!decisive)
      verdict = `The scheduler made no measurable difference: ${fine(Math.abs(gap))} between the parallel rows, inside their own ranges.`;
    else {
      verdict =
        gap > 0
          ? `With the scheduler on, the parallel batch finished ${fine(gap)} sooner`
          : `With the scheduler on, the parallel batch finished ${fine(-gap)} later`;
      if (sequential) {
        const seq = sequential.wall_duration_ns - scheduled.wall_duration_ns;
        verdict += ` than unscheduled and ${fine(Math.abs(seq))} ${seq > 0 ? "sooner" : "later"} than running the jobs in turn`;
      } else verdict += " than unscheduled";
      verdict += `, peaking at ${scheduled.peak_compilers ?? 0} compilers instead of ${unscheduled.peak_compilers ?? 0}.`;
    }
  }

  const notes: string[] = [];
  const hits = cells.filter((c) => c.stats?.hits !== undefined);
  if (hits.length === cells.length && cells.length) {
    notes.push(
      "Cache hits over the batch: " +
        cells
          .map((c) => {
            const l = CONTENTION_LABELS[c.tool];
            const label = l
              ? [l.label, l.tag].filter(Boolean).join(", ")
              : c.tool;
            return `${label} ${c.stats!.hits!.toLocaleString("en-US")}`;
          })
          .join("; ") +
        ". The scheduler holds identical compilations until the first finishes, so the other jobs hit the store instead of repeating the work.",
    );
  }

  const tile: Tile | null = scheduled
    ? {
        id: scenario.scenario,
        name: scenario.scenario,
        value: seconds(scheduled.wall_duration_ns),
        note: !unscheduled
          ? ""
          : !decisive
            ? "level with the scheduler off"
            : unscheduled.wall_duration_ns > scheduled.wall_duration_ns
              ? `${fine(unscheduled.wall_duration_ns - scheduled.wall_duration_ns)} sooner than scheduler off`
              : `${fine(scheduled.wall_duration_ns - unscheduled.wall_duration_ns)} later than scheduler off`,
        ahead: decisive && fastest === scheduled,
      }
    : null;

  return {
    card: {
      id: scenario.scenario,
      name: scenario.scenario,
      title: copy?.title ?? scenario.scenario,
      caption: copy?.caption ?? scenario.description,
      rows,
      verdict,
      notes,
      skipped: scenario.skipped.join("; "),
      provenance: provenance(scenario),
    },
    tile,
  };
}

function provenance(scenario: BenchmarkScenario) {
  return scenario.workflow_run && scenario.runner
    ? {
        runner: scenario.runner,
        url: `https://github.com/jdx/mr-boxington/actions/runs/${scenario.workflow_run}`,
      }
    : null;
}

const built = computed(() =>
  (data?.scenarios ?? []).map((scenario) =>
    scenario.kind === "contention"
      ? contentionCard(scenario)
      : buildCard(scenario),
  ),
);
const cards = computed(() => built.value.map((b) => b.card));
const tiles = computed(() =>
  built.value.map((b) => b.tile).filter((t): t is Tile => t !== null),
);

const versionList = computed(() => {
  if (!data) return "";
  return (
    Object.entries(data.versions)
      .filter(([, value]) => value)
      // `cargo -V` already says "cargo"; only mbx reports a bare number.
      .map(([n, value]) => (value!.startsWith(n) ? value! : `${n} ${value}`))
      .join(", ")
  );
});
</script>

<style scoped>
.bench {
  margin: 24px 0 32px;
}

/* ---------- at a glance ---------- */

.bench-summary {
  display: grid;
  gap: 12px;
  grid-template-columns: repeat(auto-fit, minmax(130px, 1fr));
  margin-bottom: 28px;
}
.bench-tile {
  background: var(--vp-c-bg-soft);
  border: 1px solid var(--vp-c-divider);
  border-radius: 12px;
  color: inherit;
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding: 14px 16px 13px;
  text-decoration: none;
  transition: border-color 0.2s;
}
.bench-tile:hover {
  border-color: var(--vp-c-brand-1);
}
.bench-tile-name {
  color: var(--vp-c-text-2);
  font-size: 11px;
  font-weight: 600;
  letter-spacing: 0.08em;
  text-transform: uppercase;
}
.bench-tile-value {
  color: var(--vp-c-text-1);
  font-family: var(--mbx-display);
  font-size: 30px;
  font-weight: 700;
  letter-spacing: -0.02em;
  line-height: 1.1;
}
.bench-tile.is-ahead .bench-tile-value {
  color: var(--vp-c-brand-1);
}
.bench-tile-note {
  color: var(--vp-c-text-2);
  font-size: 13px;
  line-height: 1.4;
}

/* ---------- cards ---------- */

.bench-card {
  border: 1px solid var(--vp-c-divider);
  border-radius: 12px;
  margin: 20px 0;
  overflow: hidden;
  scroll-margin-top: 80px;
}
.bench-head {
  align-items: flex-start;
  display: flex;
  gap: 20px;
  justify-content: space-between;
  padding: 18px 20px 14px;
}
.bench-title {
  border: 0;
  font-size: 18px;
  font-weight: 650;
  letter-spacing: -0.01em;
  line-height: 1.3;
  margin: 0;
  padding: 0;
}
.bench-kicker {
  color: var(--vp-c-brand-1);
  display: block;
  font-family: var(--vp-font-family-mono);
  font-size: 12px;
  font-weight: 600;
  letter-spacing: 0.06em;
  margin-bottom: 2px;
  text-transform: uppercase;
}
.bench-caption {
  color: var(--vp-c-text-2);
  font-size: 14px;
  line-height: 1.5;
  margin: 6px 0 0;
  max-width: 62ch;
}
.bench-axis {
  color: var(--vp-c-text-3);
  flex: none;
  font-size: 11px;
  letter-spacing: 0.04em;
  padding-top: 4px;
  text-transform: uppercase;
  white-space: nowrap;
}

/* ---------- rows ---------- */

.bench-table {
  border-collapse: collapse;
  display: table;
  margin: 0;
  table-layout: auto;
  width: 100%;
}
.bench-table tr {
  background: transparent;
  border: 0;
}
.bench-table td,
.bench-table th {
  background: transparent;
  border: 0;
  border-top: 1px solid var(--vp-c-divider);
  font-size: 14px;
  padding: 12px 10px;
  text-align: left;
  vertical-align: middle;
}
.bench-table td:first-child,
.bench-table th:first-child {
  padding-left: 20px;
}
.bench-table td:last-child,
.bench-table th:last-child {
  padding-right: 20px;
}
.bench-table tr.is-mbx {
  background: color-mix(in srgb, var(--vp-c-brand-soft) 45%, transparent);
}
.bench-sr-only {
  clip: rect(0 0 0 0);
  height: 1px;
  overflow: hidden;
  position: absolute;
  width: 1px;
}
.bench-tool {
  max-width: 260px;
  width: 1%;
}
.bench-tool-line {
  align-items: center;
  display: flex;
  gap: 6px;
  white-space: nowrap;
}
.bench-sub {
  color: var(--vp-c-text-3);
  display: block;
  font-family: var(--vp-font-family-sans);
  font-size: 12px;
  font-weight: 400;
  line-height: 1.4;
  margin-top: 3px;
  white-space: nowrap;
}
.bench-time .bench-sub {
  font-family: var(--vp-font-family-mono);
  font-variant-numeric: tabular-nums;
}
.bench-tool-name {
  color: var(--vp-c-text-1);
  font-family: var(--vp-font-family-mono);
  font-weight: 600;
}
.bench-tag {
  border: 1px solid var(--vp-c-divider);
  border-radius: 999px;
  color: var(--vp-c-text-2);
  font-size: 10px;
  font-weight: 600;
  letter-spacing: 0.05em;
  line-height: 1;
  padding: 3px 6px;
  text-transform: uppercase;
  white-space: nowrap;
}
.bench-tag.is-fastest {
  background: color-mix(in srgb, var(--vp-c-brand-1) 14%, transparent);
  border-color: color-mix(in srgb, var(--vp-c-brand-1) 40%, transparent);
  color: var(--vp-c-brand-1);
}
.bench-bar-cell {
  min-width: 200px;
  width: 100%;
}
.bench-track {
  height: 14px;
  position: relative;
  width: 100%;
}
.bench-bar {
  background: var(--vp-c-text-3);
  border-radius: 0 3px 3px 0;
  display: block;
  height: 8px;
  left: 0;
  opacity: 0.55;
  position: absolute;
  top: 3px;
}
.bench-bar.is-mbx {
  background: var(--vp-c-brand-1);
  opacity: 1;
}
/* The whisker spans the fastest and slowest run; ticks mark its ends. */
.bench-range {
  border-top: 2px solid var(--vp-c-text-1);
  display: block;
  height: 0;
  position: absolute;
  top: 6px;
}
.bench-range::before,
.bench-range::after {
  background: var(--vp-c-text-1);
  content: "";
  height: 8px;
  position: absolute;
  top: -5px;
  width: 2px;
}
.bench-range::before {
  left: 0;
}
.bench-range::after {
  right: 0;
}
.bench-time {
  color: var(--vp-c-text-1);
  font-family: var(--vp-font-family-mono);
  font-variant-numeric: tabular-nums;
  font-weight: 650;
  text-align: right !important;
  white-space: nowrap;
  width: 1%;
}

/* ---------- card footer ---------- */

.bench-foot {
  border-top: 1px solid var(--vp-c-divider);
  padding: 14px 20px 16px;
}
.bench-foot:empty {
  display: none;
}
.bench-verdict {
  color: var(--vp-c-text-1);
  font-size: 14px;
  line-height: 1.5;
  margin: 0;
}
.bench-footnote {
  color: var(--vp-c-text-2);
  font-size: 13px;
  line-height: 1.5;
  margin: 6px 0 0;
}
.bench-provenance {
  color: var(--vp-c-text-2);
  font-size: 13px;
  line-height: 1.5;
}
.bench-empty {
  border: 1px solid var(--vp-c-divider);
  border-radius: 8px;
  padding: 16px 20px;
}
.bench-empty p {
  margin: 0;
}

@media (max-width: 720px) {
  .bench-head {
    display: block;
    padding: 16px 16px 12px;
  }
  .bench-axis {
    display: block;
    margin-top: 10px;
  }
  .bench-table,
  .bench-table tbody {
    display: block;
  }
  .bench-table tr {
    display: grid;
    gap: 0 12px;
    grid-template-columns: minmax(0, 1fr) auto;
    padding: 12px 16px;
  }
  .bench-table tr + tr {
    border-top: 1px solid var(--vp-c-divider);
  }
  .bench-table td,
  .bench-table th {
    border: 0;
    max-width: none;
    min-width: 0;
    padding: 0;
    width: auto;
  }
  .bench-tool {
    grid-column: 1;
    grid-row: 1;
  }
  .bench-tool-line {
    flex-wrap: wrap;
    white-space: normal;
  }
  .bench-sub {
    white-space: normal;
  }
  .bench-time {
    grid-column: 2;
    grid-row: 1;
  }
  .bench-bar-cell {
    grid-column: 1 / -1;
    grid-row: 2;
    margin: 8px 0 0;
  }
  .bench-foot {
    padding: 12px 16px 14px;
  }
}
</style>
