// The published benchmark run, reduced to the claims the reel and the landing
// page make from it. Each claim is null unless the run backs it, and every
// scene and chapter text has a line that claims no number for that case, so
// a failed, malformed or inconclusive run leaves the figures out instead of
// drawing one the benchmarks page would not stand behind.

// Structural subset of docs/.vitepress/benchmarks.data.ts, so the reel reads
// the published benchmark run instead of carrying its own numbers.
interface Cell {
  tool: string;
  /** The median run, or the only one where the runs are absent. */
  wall_duration_ns: number;
  trials?: number;
  wall_durations_ns?: number[];
  peak_compilers?: number;
  permits?: number | null;
  stats?: { lookups?: number; hits?: number; misses?: number };
}
interface Results {
  subject: string;
  toolchain?: string;
  versions?: Record<string, string | null>;
  passed: boolean;
  scenarios: { scenario: string; results: Cell[] }[];
}

/** Same checkout, empty target/, warm store: the `warm` scenario's mbx row. */
export interface WarmFact {
  /** Compilations restored from the store. */
  hits: number;
  /** Compilations looked up in it. */
  lookups: number;
  /** The median run's wall clock, in seconds. */
  seconds: number;
  /** Runs the median is taken over; 1 for a single run. */
  trials: number;
}

/**
 * The next push, the `commit` scenario: plain Cargo against mbx with the
 * store warmed at the parent commit. Only published when the two tools' runs
 * are separated(), so a scene that has it may draw the bars.
 */
export interface CommitFact {
  /** Cargo's median run, in seconds. */
  cargo: number;
  /** mbx's median run, in seconds. */
  mbx: number;
  /** Every run, in seconds, each median among its own. */
  runs: { cargo: readonly number[]; mbx: readonly number[] };
  /** Runs per tool, at least 2. */
  trials: number;
  /** Compilations mbx restored in its median run. */
  hits: number;
  /** Compilations it looked up. */
  lookups: number;
  /** Compilations rustc ran for it: `hits + misses == lookups`. */
  misses: number;
}

/**
 * Six builds at once, the `contention` scenario: the most compilers seen
 * running at once with mbx's scheduler on (the `mbx` row) and off
 * (`mbx-unscheduled`). Only published when the scheduled peak is the lower
 * and the two batches' times are separated(). The times stay off screen.
 */
export interface ContentionFact {
  /** Peak compilers with the scheduler on. */
  scheduled: number;
  /** Peak compilers with the scheduler off. */
  unscheduled: number;
  /** Runs per batch, at least 2. */
  trials: number;
}

export interface ReelFacts {
  /** Benchmark subject, e.g. "hk"; "" when the run names none fit to print. */
  subject: string;
  /** The Rust release every timed build used, e.g. "1.94.0". */
  toolchain: string | null;
  /** Versions the run recorded: `mbx` is the release it measured, e.g. "1.17.0". */
  versions: { mbx: string | null };
  warm: WarmFact | null;
  commit: CommitFact | null;
  contention: ContentionFact | null;
}

const seconds = (ns: number) => ns / 1e9;
/** A whole, nonnegative counter small enough to be represented exactly. */
const count = (n: unknown): n is number => typeof n === "number" && Number.isSafeInteger(n) && n >= 0;
/** A positive, finite wall-clock duration. */
const duration = (n: unknown): n is number => typeof n === "number" && Number.isFinite(n) && n > 0;
/** A bare release, "1.94.0" or "1.96.0-nightly", short enough to print. */
const release = (v: unknown): string | null =>
  typeof v === "string" && /^\d{1,4}\.\d{1,4}\.\d{1,6}(?:-[0-9A-Za-z.]{1,24})?$/.test(v) ? v : null;
/** A project name fit for a label, "hk". */
const name = (v: unknown): string => (typeof v === "string" && /^[A-Za-z0-9][\w.-]{0,39}$/.test(v) ? v : "");

/**
 * The published median of repeated runs: the middle one, or the lower middle
 * of an even count, as benchmarks/real_world.py's median_cell picks it.
 */
export function median(runs: readonly number[]): number {
  const sorted = [...runs].sort((a, b) => a - b);
  return sorted[(sorted.length - 1) >> 1];
}

/** How far one tool moved across its own runs. */
const spread = (runs: readonly number[]) => Math.max(...runs) - Math.min(...runs);

/**
 * Whether two tools' runs clearly differ, by the benchmarks page's rule
 * (BenchmarkResults.vue): both were repeated, and the gap between their
 * medians is wider than either one's own spread. Anything closer is noise,
 * and the reel draws no comparison from it.
 */
export function separated(a: readonly number[], b: readonly number[]): boolean {
  if (a.length < 2 || b.length < 2) return false;
  return Math.abs(median(a) - median(b)) > Math.max(spread(a), spread(b));
}

/**
 * A cell's runs in nanoseconds, with its published timing as their median.
 * A cell published before per-run timings is one run. Null when the timing
 * is missing or disagrees with the runs beside it.
 */
function runsOf(cell: Cell | undefined): number[] | null {
  const wall = cell?.wall_duration_ns;
  if (!duration(wall)) return null;
  const runs = cell?.wall_durations_ns;
  if (runs === undefined) return [wall];
  if (!Array.isArray(runs) || !runs.length || !runs.every(duration)) return null;
  if (cell?.trials !== undefined && cell.trials !== runs.length) return null;
  return median(runs) === wall ? runs : null;
}

/** Two tools' runs, when both are sound, repeated alike, and separated(). */
function compared(a: Cell | undefined, b: Cell | undefined): [number[], number[]] | null {
  const x = runsOf(a);
  const y = runsOf(b);
  return x && y && x.length === y.length && separated(x, y) ? [x, y] : null;
}

function warmFact(mbx: Cell | undefined): WarmFact | null {
  const runs = runsOf(mbx);
  const hits = mbx?.stats?.hits;
  const lookups = mbx?.stats?.lookups;
  // A warm build that restored nothing is not the one the scene shows.
  if (!runs || !count(hits) || !count(lookups) || hits === 0 || hits > lookups) return null;
  return { hits, lookups, seconds: seconds(median(runs)), trials: runs.length };
}

function commitFact(cargo: Cell | undefined, mbx: Cell | undefined): CommitFact | null {
  const runs = compared(cargo, mbx);
  const hits = mbx?.stats?.hits;
  const lookups = mbx?.stats?.lookups;
  const misses = mbx?.stats?.misses;
  // The annotation says how every lookup ended, so the counts must add up.
  if (!runs || !count(hits) || !count(lookups) || !count(misses)) return null;
  if (lookups === 0 || hits + misses !== lookups) return null;
  const [c, m] = runs.map((r) => r.map(seconds));
  return { cargo: median(c), mbx: median(m), runs: { cargo: c, mbx: m }, trials: c.length, hits, lookups, misses };
}

function contentionFact(on: Cell | undefined, off: Cell | undefined): ContentionFact | null {
  const runs = compared(on, off);
  const scheduled = on?.peak_compilers;
  const unscheduled = off?.peak_compilers;
  const permits = on?.permits;
  if (!runs || !count(scheduled) || !count(unscheduled) || scheduled === 0 || scheduled >= unscheduled) return null;
  // The pool bounded the machine: never more compilers than permits.
  if (permits !== undefined && permits !== null && !(count(permits) && scheduled <= permits)) return null;
  return { scheduled, unscheduled, trials: runs[0].length };
}

export function factsFromBenchmarks(data: Results | null | undefined): ReelFacts | null {
  // The loaders cast parsed JSON and drop failed runs; this runs in every
  // page's theme setup and in the renderer, so it checks again and leaves a
  // malformed run's numbers out instead of throwing.
  if (!data || typeof data !== "object" || data.passed !== true || !Array.isArray(data.scenarios)) return null;
  const cell = (scenario: string, tool: string) => {
    const results = data.scenarios.find((s) => s?.scenario === scenario)?.results;
    return Array.isArray(results) ? results.find((c) => c?.tool === tool) : undefined;
  };
  const facts: ReelFacts = {
    subject: name(data.subject),
    toolchain: release(data.toolchain),
    versions: { mbx: release(data.versions?.mbx) },
    warm: warmFact(cell("warm", "mbx")),
    commit: commitFact(cell("commit", "cargo"), cell("commit", "mbx")),
    contention: contentionFact(cell("contention", "mbx"), cell("contention", "mbx-unscheduled")),
  };
  // A run the reel can claim nothing from is no run to it.
  return facts.warm || facts.commit || facts.contention ? facts : null;
}

/** A time as the chart and the benchmarks page print it: seconds to a tenth. */
export const tenths = (s: number): string => s.toFixed(1);

/**
 * The next push's saving, for the chart's delta and the page: the raw
 * medians' difference, rounded once to the tenth it prints at. 18.877 −
 * 9.229 = 9.648 prints 9.6, where the rounded readouts would give 9.7. Null
 * unless mbx finished at least a printable tenth sooner.
 */
export function delta(c: CommitFact): number | null {
  const d = Math.round((c.cargo - c.mbx) * 10) / 10;
  return d > 0 ? d : null;
}

/**
 * The next push's annotation under the mbx bar: its lookups restored, and
 * those rustc compiled, left out when it compiled none.
 */
export function annotation(c: CommitFact): string {
  const restored = `${c.hits} of ${c.lookups} restored`;
  return c.misses > 0 ? `${restored}, ${c.misses} compiled` : restored;
}

/** "median of 3" for repeated runs, as the source lines print it; null for one run. */
export const medianOf = (trials: number): string | null => (trials > 1 ? `median of ${trials}` : null);
