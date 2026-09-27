import { readFileSync } from "node:fs";
import type { BenchmarkResults } from "./benchmarks.data";

/** Reject failed runs and files belonging to a different workload. */
export function loadBenchmark(path: string, subject: string): BenchmarkResults | null {
  try {
    const results = JSON.parse(readFileSync(path, "utf8")) as BenchmarkResults;
    return [1, 2].includes(results.schema) && results.passed && results.subject === subject
      ? results
      : null;
  } catch {
    return null;
  }
}
