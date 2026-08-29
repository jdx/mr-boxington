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
      <h3 :id="scenario.scenario">{{ scenario.scenario }}</h3>
      <p class="mbx-bench-caption">{{ scenario.description }}</p>
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
      <table
        v-else-if="scenario.kind === 'contention' && scenario.results.length"
        class="mbx-bench-table"
      >
        <thead>
          <tr>
            <th scope="col">Tool</th>
            <th scope="col">Batch time</th>
            <th scope="col" class="mbx-bench-numeric">vs. sequential</th>
            <th scope="col" class="mbx-bench-numeric">Peak compilers</th>
            <th scope="col" class="mbx-bench-numeric">Lowest free memory</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="cell in contentionRows(scenario)" :key="cell.tool">
            <th scope="row">{{ cell.tool }}</th>
            <td>
              <span class="mbx-bench-bar-track" aria-hidden="true">
                <span
                  class="mbx-bench-bar"
                  :class="{ 'is-subject': cell.tool === 'mbx' }"
                  :style="{ width: `${cell.width}%` }"
                />
              </span>
              <span class="mbx-bench-seconds">{{ cell.seconds }}</span>
            </td>
            <td class="mbx-bench-numeric">{{ cell.relative }}</td>
            <td class="mbx-bench-numeric">{{ cell.compilers }}</td>
            <td class="mbx-bench-numeric">{{ cell.memory }}</td>
          </tr>
        </tbody>
      </table>
      <table v-else-if="scenario.results.length" class="mbx-bench-table">
        <thead>
          <tr>
            <th scope="col">Tool</th>
            <th scope="col">Build time</th>
            <th scope="col" class="mbx-bench-numeric">vs. cold cargo</th>
            <th scope="col" class="mbx-bench-numeric">Hits</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="cell in rows(scenario)" :key="cell.tool">
            <th scope="row">{{ cell.tool }}</th>
            <td>
              <span class="mbx-bench-bar-track" aria-hidden="true">
                <span
                  class="mbx-bench-bar"
                  :class="{ 'is-subject': cell.tool === 'mbx' }"
                  :style="{ width: `${cell.width}%` }"
                />
              </span>
              <span class="mbx-bench-seconds">{{ cell.seconds }}</span>
            </td>
            <td class="mbx-bench-numeric">{{ cell.relative }}</td>
            <td class="mbx-bench-numeric">{{ cell.hits }}</td>
          </tr>
        </tbody>
      </table>
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

// Every scenario compares against the same number: what plain cargo costs
// with nothing cached. Only the cold scenario runs cargo -- with a wiped
// target/ the others would just repeat it -- so a per-scenario baseline would
// leave the warm and cross-worktree rows with nothing to compare to, which is
// exactly the comparison a reader wants.
const coldCargo = computed(
  () =>
    data?.scenarios
      .find((scenario) => scenario.scenario === "cold")
      ?.results.find((cell) => cell.tool === "cargo") ?? null,
);

// Bars are scaled within a scenario, never across them: each table answers
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
  const baseline = coldCargo.value;
  return scenario.results.map((cell) => {
    const seconds = cell.wall_duration_ns / 1e9;
    return {
      tool: cell.tool,
      seconds:
        seconds >= 60
          ? `${Math.floor(seconds / 60)}m ${(seconds % 60).toFixed(0)}s`
          : `${seconds.toFixed(1)}s`,
      width: barWidth(cell.wall_duration_ns, slowest),
      relative:
        // Two decimals: a cache that costs 3% on a cold build rounds to
        // "1.0x" at one, which reads as free.
        !baseline || cell === baseline
          ? "—"
          : `${(baseline.wall_duration_ns / cell.wall_duration_ns).toFixed(2)}×`,
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
  const baseline = scenario.results.find(
    (cell) => cell.tool === "mbx-sequential",
  );
  return scenario.results.map((cell) => {
    const seconds = cell.wall_duration_ns / 1e9;
    const available = cell.min_available_bytes;
    return {
      tool: cell.tool,
      seconds:
        seconds >= 60
          ? `${Math.floor(seconds / 60)}m ${(seconds % 60).toFixed(0)}s`
          : `${seconds.toFixed(1)}s`,
      width: barWidth(cell.wall_duration_ns, slowest),
      relative:
        !baseline || cell === baseline
          ? "—"
          : `${(baseline.wall_duration_ns / cell.wall_duration_ns).toFixed(2)}×`,
      compilers: cell.permits
        ? `${cell.peak_compilers ?? 0} / ${cell.permits} permits`
        : `${cell.peak_compilers ?? 0}`,
      // Only Linux reports it, and only Linux runs the published benchmark.
      // Zero is a real reading -- a machine that ran itself out -- and it is
      // the single most interesting cell on the page, so it must not be
      // rounded away into the same dash that means "never measured".
      memory:
        available === null || available === undefined
          ? "—"
          : `${(available / 1e9).toFixed(1)} GB`,
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
  margin: 32px 0;
}
.mbx-bench-caption {
  color: var(--vp-c-text-2);
  font-size: 14px;
  margin-top: -8px;
}
.mbx-bench-table {
  display: table;
  width: 100%;
}
.mbx-bench-table th[scope="row"] {
  font-family: var(--vp-font-family-mono);
  font-weight: 500;
}
.mbx-bench-numeric {
  text-align: right;
}
.mbx-bench-bar-track {
  border-radius: 3px;
  display: inline-block;
  height: 10px;
  margin-right: 8px;
  vertical-align: middle;
  width: 60%;
}
.mbx-bench-bar {
  background: var(--vp-c-divider);
  border-radius: inherit;
  display: block;
  height: 100%;
}
.mbx-bench-bar.is-subject {
  background: var(--vp-c-brand-1);
}
.mbx-bench-seconds {
  font-family: var(--vp-font-family-mono);
  font-size: 13px;
  white-space: nowrap;
}
.mbx-bench-guard,
.mbx-bench-skipped,
.mbx-bench-provenance {
  color: var(--vp-c-text-2);
  font-size: 13px;
}
.mbx-bench-empty {
  border: 1px solid var(--vp-c-divider);
  border-radius: 8px;
  padding: 16px 20px;
}
.mbx-bench-empty p {
  margin: 0;
}
</style>
