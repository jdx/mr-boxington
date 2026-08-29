import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const configDir = dirname(fileURLToPath(import.meta.url));
const resultsPath = resolve(configDir, "../../benchmarks/results.json");

export interface BenchmarkCell {
  tool: string;
  wall_duration_ns: number;
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

export interface BenchmarkScenario {
  scenario: string;
  description: string;
  /** False for the compiler-change guard, which asserts rather than races. */
  timed: boolean;
  /**
   * "build" times one build per tool; "contention" runs several at once and
   * reports what the machine did. Absent on runs published before the
   * contention scenario existed, which are all "build".
   */
  kind?: "build" | "contention";
  results: BenchmarkCell[];
  skipped: string[];
}

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
      if (results.schema !== 1 || !results.passed) return null;
      return results;
    } catch {
      // No published run yet, or a file this page does not understand.
      return null;
    }
  },
};
