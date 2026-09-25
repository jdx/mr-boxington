import type { ReelFacts } from "./bible";

// Structural subset of docs/.vitepress/benchmarks.data.ts, so the reel reads
// the published benchmark run instead of carrying its own numbers.
interface Cell {
  tool: string;
  wall_duration_ns: number;
  stats?: { lookups?: number; hits?: number };
}
interface Results {
  subject: string;
  scenarios: { scenario: string; results: Cell[] }[];
}

const seconds = (ns: number) => ns / 1e9;
const finite = (n: unknown): n is number => typeof n === "number" && Number.isFinite(n);

export function factsFromBenchmarks(data: Results | null | undefined): ReelFacts | null {
  // The loader casts parsed JSON, and this runs in every page's theme setup,
  // so a malformed run leaves the numbers out instead of throwing.
  if (!data || !Array.isArray(data.scenarios)) return null;
  const cell = (scenario: string, tool: string) => {
    const results = data.scenarios.find((s) => s?.scenario === scenario)?.results;
    return Array.isArray(results) ? results.find((c) => c?.tool === tool) : undefined;
  };
  const warm = cell("warm", "mbx");
  const cargo = cell("commit", "cargo");
  const mbx = cell("commit", "mbx");
  const hits = warm?.stats?.hits;
  const lookups = warm?.stats?.lookups;
  return {
    subject: typeof data.subject === "string" ? data.subject : "",
    warm:
      warm && finite(hits) && finite(lookups) && lookups > 0 && finite(warm.wall_duration_ns)
        ? { hits, lookups, seconds: seconds(warm.wall_duration_ns) }
        : null,
    commit:
      finite(cargo?.wall_duration_ns) && finite(mbx?.wall_duration_ns)
        ? { cargo: seconds(cargo.wall_duration_ns), mbx: seconds(mbx.wall_duration_ns) }
        : null,
  };
}
