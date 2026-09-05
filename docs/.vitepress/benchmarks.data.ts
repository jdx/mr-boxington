import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const configDir = dirname(fileURLToPath(import.meta.url));
const resultsPath = resolve(configDir, "../../benchmarks/results.json");

export interface BenchmarkCell {
  tool: string;
  /** The median trial's wall clock, or the only one where trials is absent. */
  wall_duration_ns: number;
  /** How many times the scenario was repeated for this tool. */
  trials?: number;
  /** Every trial's wall clock, so the page can see its own noise floor. */
  wall_durations_ns?: number[];
  /**
   * Edit scenario only: the discarded first edit after a build. Not the loop,
   * which is why it is not the timing, but it is what the loop cost to get
   * into, and a developer waits for it once per fresh build.
   */
  warmup_wall_duration_ns?: number;
  /** Contention only: most real compilers seen running at once, machine-wide. */
  peak_compilers?: number;
  /** Contention only: lowest memory the machine had left, where reported. */
  min_available_bytes?: number | null;
  /** Contention only: the bound the scheduled cell was given, else null. */
  permits?: number | null;
  stats?: {
    lookups?: number;
    hits?: number;
    predictions_loaded?: number;
    restored_output_files?: number;
  };
}

interface BenchmarkScenarioBase {
  scenario: string;
  description: string;
  /**
   * What this scenario's cargo row is, since it is not the same thing in
   * every one: a control with no cache to help it in the commit scenario, the
   * incremental rebuild the caches have to beat in the edit one. Absent on
   * runs published before the edit scenario existed, which are all controls.
   */
  baseline?: string;
  /**
   * "build" times one build per tool; "contention" compares sequential and
   * parallel commands and reports what the machine did. Absent on runs
   * published before the contention scenario existed, which are all "build".
   */
  kind?: "build" | "contention";
  results: BenchmarkCell[];
  skipped: string[];
}

/** A separately refreshed scenario must identify both its run and runner. */
export type BenchmarkScenario = BenchmarkScenarioBase &
  (
    | { workflow_run: string; runner: string }
    | { workflow_run?: never; runner?: never }
  );

export interface BenchmarkResults {
  schema: number;
  subject: string;
  revision: string;
  toolchain: string;
  platform: string;
  runner: string;
  workflow_run: string | null;
  versions: Record<string, string | null>;
  passed: boolean;
  scenarios: BenchmarkScenario[];
}

export default {
  // Rebuild the page when a refreshed run lands, rather than serving the
  // numbers that happened to be on disk when the dev server started.
  watch: [resultsPath],
  load(): BenchmarkResults | null {
    try {
      const results = JSON.parse(
        readFileSync(resultsPath, "utf8"),
      ) as BenchmarkResults;
      // A run that failed its own validity checks measured something other
      // than what the page would claim; the empty state is the honest render.
      // 2 added per-trial timings; 1 renders as a single trial per cell.
      if (![1, 2].includes(results.schema) || !results.passed) return null;
      return results;
    } catch {
      // No published run yet, or a file this page does not understand.
      return null;
    }
  },
};
