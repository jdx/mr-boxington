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

export function factsFromBenchmarks(data: Results | null | undefined): ReelFacts | null {
  if (!data) return null;
  const cell = (scenario: string, tool: string) =>
    data.scenarios.find((s) => s.scenario === scenario)?.results.find((c) => c.tool === tool);
  const warm = cell("warm", "mbx");
  const cargo = cell("commit", "cargo");
  const mbx = cell("commit", "mbx");
  return {
    subject: data.subject,
    warm:
      warm?.stats?.hits !== undefined && warm.stats.lookups
        ? {
            hits: warm.stats.hits,
            lookups: warm.stats.lookups,
            seconds: seconds(warm.wall_duration_ns),
          }
        : null,
    commit:
      cargo && mbx
        ? { cargo: seconds(cargo.wall_duration_ns), mbx: seconds(mbx.wall_duration_ns) }
        : null,
  };
}
