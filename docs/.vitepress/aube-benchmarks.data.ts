import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { loadBenchmark } from "./benchmark-loader";

const resultsPath = resolve(dirname(fileURLToPath(import.meta.url)), "../../benchmarks/results-aube.json");

export default {
  watch: [resultsPath],
  load: () => loadBenchmark(resultsPath, "aube"),
};
