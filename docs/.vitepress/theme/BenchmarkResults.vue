<template>
  <div v-if="!data" class="mbx-bench-empty">
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
  <div v-else class="mbx-bench">
    <section
      v-for="scenario in data.scenarios"
      :key="scenario.scenario"
      class="mbx-bench-scenario"
    >
      <header class="mbx-bench-header">
        <div>
          <h3 :id="scenario.scenario">{{ scenario.scenario }}</h3>
          <p class="mbx-bench-caption">{{ scenario.description }}</p>
        </div>
        <span
          v-if="scenario.timed && scenario.kind !== 'contention'"
          class="mbx-bench-direction"
        >
          lower is better
        </span>
      </header>
      <p
        v-if="!scenario.timed && scenario.results.length"
        class="mbx-bench-guard"
      >
        Guard held: of
        {{ scenario.results[0].stats?.predictions_loaded ?? 0 }} predicted
        compilations, a different compiler let mbx look up
        {{ scenario.results[0].stats?.lookups ?? 0 }} — the ones that do not
        depend on rustc at all.
      </p>
      <div
        v-else-if="scenario.kind === 'contention' && scenario.results.length"
        class="mbx-bench-chart"
        role="list"
        aria-label="Contention benchmark results"
      >
        <div
          v-for="cell in contentionRows(scenario)"
          :key="cell.tool"
          class="mbx-bench-row"
          :class="{ 'is-subject': cell.tool === 'mbx' }"
          role="listitem"
        >
          <div class="mbx-bench-tool">
            <code>{{ cell.label }}</code>
            <span v-if="cell.badge" class="mbx-bench-fastest">{{ cell.badge }}</span>
          </div>
          <div class="mbx-bench-bar-track" aria-hidden="true">
            <span
              class="mbx-bench-bar"
              :class="{ 'is-subject': cell.tool === 'mbx' }"
              :style="{ width: `${cell.width}%` }"
            />
          </div>
          <strong class="mbx-bench-seconds">{{ cell.seconds }}</strong>
          <div class="mbx-bench-meta">
            <span v-if="cell.comparison">{{ cell.comparison }}</span>
            <span>{{ cell.compilers }}</span>
            <span>{{ cell.memory }}</span>
          </div>
        </div>
      </div>
      <div
        v-else-if="scenario.results.length"
        class="mbx-bench-chart"
        role="list"
        :aria-label="`${scenario.scenario} benchmark results`"
      >
        <div
          v-for="cell in rows(scenario)"
          :key="cell.tool"
          class="mbx-bench-row"
          :class="{ 'is-subject': cell.tool === 'mbx' }"
          role="listitem"
        >
          <div class="mbx-bench-tool">
            <code>{{ cell.tool }}</code>
            <span v-if="cell.fastest" class="mbx-bench-fastest">fastest</span>
          </div>
          <div class="mbx-bench-bar-track" aria-hidden="true">
            <span
              class="mbx-bench-bar"
              :class="{ 'is-subject': cell.tool === 'mbx' }"
              :style="{ width: `${cell.width}%` }"
            />
          </div>
          <strong class="mbx-bench-seconds">{{ cell.seconds }}</strong>
          <div class="mbx-bench-meta">
            <span v-if="cell.comparison">{{ cell.comparison }}</span>
            <span v-if="cell.hits !== '—'">{{ cell.hits }} cache hits</span>
          </div>
        </div>
      </div>
      <p v-if="scenario.skipped.length" class="mbx-bench-skipped">
        Not measured: {{ scenario.skipped.join("; ") }}
      </p>
    </section>

    <p class="mbx-bench-provenance">
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
import type { BenchmarkScenario } from "../benchmarks.data";

// Bars are scaled within a scenario, never across them: each card answers
// which tool was faster on that workload, not how the workloads compare to
// each other. The bar has its own fixed-width track beside the duration label,
// so the slowest result fills the track and every other result is proportional.
function barWidth(duration: number, slowest: number) {
  return Math.max(2, (duration / slowest) * 100);
}

function rows(scenario: BenchmarkScenario) {
  const slowest = Math.max(
    ...scenario.results.map((cell) => cell.wall_duration_ns),
    1,
  );
  const fastest = Math.min(
    ...scenario.results.map((cell) => cell.wall_duration_ns),
  );
  // A Cargo result is meaningful only inside the scenario that measured it.
  // In the commit case it is deliberately an uncached control: Cargo has no
  // portable store to seed from the parent commit, and its target is fresh.
  const baseline = scenario.results.find((cell) => cell.tool === "cargo");
  return scenario.results.map((cell) => {
    const seconds = cell.wall_duration_ns / 1e9;
    const ratio = baseline
      ? baseline.wall_duration_ns / cell.wall_duration_ns
      : null;
    return {
      tool: cell.tool,
      fastest: cell.wall_duration_ns === fastest,
      seconds:
        seconds >= 60
          ? `${Math.floor(seconds / 60)}m ${(seconds % 60).toFixed(0)}s`
          : `${seconds.toFixed(1)}s`,
      width: barWidth(cell.wall_duration_ns, slowest),
      comparison:
        !baseline
          ? null
          : cell === baseline
            ? "uncached baseline"
            : ratio! >= 1
              ? `${ratio!.toFixed(2)}× faster than cargo`
              : `${(1 / ratio!).toFixed(2)}× slower than cargo`,
      hits: cell.stats?.hits ?? "—",
    };
  });
}

// The contention scenario asks a different question than the others -- not
// whether parallel lint beat its sequential baseline and what the machine
// looked like while both Cargo processes ran -- so it gets machine-wide
// columns rather than the ordinary cache-build comparison.
function contentionRows(scenario: BenchmarkScenario) {
  const slowest = Math.max(
    ...scenario.results.map((cell) => cell.wall_duration_ns),
    1,
  );
  const parallelControl = scenario.results.find(
    (cell) => cell.tool === "mbx-unscheduled",
  );
  return scenario.results.map((cell) => {
    const seconds = cell.wall_duration_ns / 1e9;
    const available = cell.min_available_bytes;
    const delta = parallelControl
      ? (cell.wall_duration_ns - parallelControl.wall_duration_ns) / 1e9
      : null;
    return {
      tool: cell.tool,
      label:
        cell.tool === "mbx-sequential"
          ? "sequential"
          : cell.tool === "mbx-unscheduled"
            ? "parallel · scheduler off"
            : "parallel · mbx scheduled",
      badge:
        cell.tool === "mbx-sequential"
          ? "context"
          : cell.tool === "mbx-unscheduled"
            ? "parallel control"
            : null,
      seconds:
        seconds >= 60
          ? `${Math.floor(seconds / 60)}m ${(seconds % 60).toFixed(0)}s`
          : `${seconds.toFixed(1)}s`,
      width: barWidth(cell.wall_duration_ns, slowest),
      comparison:
        delta === null || cell.tool !== "mbx"
          ? null
          : `${Math.abs(delta).toFixed(1)}s ${delta >= 0 ? "slower" : "faster"} than scheduler off`,
      compilers: cell.permits
        ? `${cell.peak_compilers ?? 0} of ${cell.permits} compiler slots used`
        : `${cell.peak_compilers ?? 0} peak compilers`,
      // Only Linux reports it, and only Linux runs the published benchmark.
      // Zero is a real reading -- a machine that ran itself out -- and it is
      // the single most interesting cell on the page, so it must not be
      // rounded away into the same dash that means "never measured".
      memory:
        available === null || available === undefined
          ? "—"
          : `${(available / 1e9).toFixed(1)} GB minimum free`,
    };
  });
}

const versionList = computed(() => {
  if (!data) return "";
  return (
    Object.entries(data.versions)
      .filter(([, value]) => value)
      // `cargo -V` already says "cargo"; only mbx reports a bare number.
      .map(([name, value]) =>
        value!.startsWith(name) ? value! : `${name} ${value}`,
      )
      .join(", ")
  );
});
</script>

<style scoped>
.mbx-bench-scenario {
  background:
    linear-gradient(
      145deg,
      color-mix(in srgb, var(--vp-c-bg-soft) 96%, var(--vp-c-brand-1) 4%),
      var(--vp-c-bg-soft)
    );
  border: 1px solid color-mix(in srgb, var(--vp-c-divider) 80%, var(--vp-c-brand-1) 20%);
  border-radius: 14px;
  margin: 28px 0;
  overflow: hidden;
}
.mbx-bench-header {
  align-items: flex-start;
  display: flex;
  gap: 20px;
  justify-content: space-between;
  padding: 20px 22px 18px;
}
.mbx-bench-header h3 {
  border: 0;
  margin: 0;
  padding: 0;
}
.mbx-bench-caption {
  color: var(--vp-c-text-2);
  font-size: 14px;
  line-height: 1.5;
  margin: 5px 0 0;
}
.mbx-bench-direction {
  border: 1px solid var(--vp-c-divider);
  border-radius: 999px;
  color: var(--vp-c-text-2);
  flex: none;
  font-size: 11px;
  font-weight: 600;
  letter-spacing: 0.06em;
  line-height: 1;
  padding: 6px 9px;
  text-transform: uppercase;
}
.mbx-bench-chart {
  border-top: 1px solid var(--vp-c-divider);
}
.mbx-bench-row {
  align-items: center;
  display: grid;
  gap: 8px 18px;
  grid-template-columns: minmax(120px, 0.8fr) minmax(180px, 2fr) auto;
  padding: 16px 22px;
}
.mbx-bench-row + .mbx-bench-row {
  border-top: 1px solid var(--vp-c-divider);
}
.mbx-bench-row.is-subject {
  background: color-mix(in srgb, var(--vp-c-brand-soft) 58%, transparent);
}
.mbx-bench-tool {
  align-items: center;
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
}
.mbx-bench-tool code {
  background: transparent;
  color: var(--vp-c-text-1);
  font-size: 14px;
  font-weight: 650;
  padding: 0;
}
.mbx-bench-fastest {
  background: color-mix(in srgb, var(--vp-c-brand-1) 14%, transparent);
  border: 1px solid color-mix(in srgb, var(--vp-c-brand-1) 32%, transparent);
  border-radius: 999px;
  color: var(--vp-c-brand-1);
  font-size: 10px;
  font-weight: 700;
  letter-spacing: 0.06em;
  line-height: 1;
  padding: 4px 6px;
  text-transform: uppercase;
}
.mbx-bench-bar-track {
  background: color-mix(in srgb, var(--vp-c-divider) 58%, transparent);
  border-radius: 999px;
  display: block;
  height: 12px;
  overflow: hidden;
  width: 100%;
}
.mbx-bench-bar {
  background: var(--vp-c-text-3);
  border-radius: inherit;
  display: block;
  height: 100%;
  min-width: 3px;
}
.mbx-bench-bar.is-subject {
  background: linear-gradient(90deg, var(--vp-c-brand-2), var(--vp-c-brand-1));
  box-shadow: 0 0 14px color-mix(in srgb, var(--vp-c-brand-1) 32%, transparent);
}
.mbx-bench-seconds {
  color: var(--vp-c-text-1);
  font-family: var(--vp-font-family-mono);
  font-size: 14px;
  font-variant-numeric: tabular-nums;
  justify-self: end;
  white-space: nowrap;
}
.mbx-bench-meta {
  color: var(--vp-c-text-2);
  display: flex;
  flex-wrap: wrap;
  font-size: 11px;
  gap: 6px;
  grid-column: 2 / 4;
}
.mbx-bench-meta span {
  background: color-mix(in srgb, var(--vp-c-bg) 72%, transparent);
  border: 1px solid var(--vp-c-divider);
  border-radius: 999px;
  line-height: 1;
  padding: 5px 7px;
}
.mbx-bench-guard,
.mbx-bench-skipped,
.mbx-bench-provenance {
  color: var(--vp-c-text-2);
  font-size: 13px;
}
.mbx-bench-guard,
.mbx-bench-skipped {
  border-top: 1px solid var(--vp-c-divider);
  margin: 0;
  padding: 16px 22px;
}
.mbx-bench-empty {
  border: 1px solid var(--vp-c-divider);
  border-radius: 8px;
  padding: 16px 20px;
}
.mbx-bench-empty p {
  margin: 0;
}
@media (max-width: 640px) {
  .mbx-bench-header {
    display: block;
    padding: 18px 16px 16px;
  }
  .mbx-bench-direction {
    display: inline-block;
    margin-top: 12px;
  }
  .mbx-bench-row {
    gap: 10px 14px;
    grid-template-columns: minmax(0, 1fr) auto;
    padding: 15px 16px;
  }
  .mbx-bench-tool {
    grid-column: 1;
    grid-row: 1;
  }
  .mbx-bench-seconds {
    grid-column: 2;
    grid-row: 1;
  }
  .mbx-bench-bar-track {
    grid-column: 1 / -1;
    grid-row: 2;
  }
  .mbx-bench-meta {
    grid-column: 1 / -1;
    grid-row: 3;
    margin-top: -2px;
  }
}
</style>
