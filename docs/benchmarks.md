---
description: Compare Cargo, mbx, and kache on a pinned Rust project with documented trials and workload limitations.
---
# Benchmarks

mbx is measured against plain Cargo and
[kache](https://github.com/kunobi-ninja/kache) on
[jdx/hk](https://github.com/jdx/hk), a mid-size Rust CLI with C dependencies,
pinned to one commit and built with `cargo build --locked`. The scenarios cover
work a developer or CI runner may repeat. Published results come from GitHub
Actions. The page labels a tool fastest only when its lead exceeds the observed
variation between trials.

<BenchmarkResults />

## Reading the results {#reading-the-cards}

Each scenario has three independent trials per tool. A trial starts with a
fresh clone and empty store, then performs the scenario's warm-up and measured
builds. The bar shows the median; the whisker spans the fastest and slowest
trial. A tool is marked fastest only when its lead exceeds both tools' trial
ranges. Otherwise, the card reports no clear winner.

The Cargo row means something different in each scenario, so the card tags it.
In the commit scenario it is the uncached build CI does without a cache. In
the edit scenario it is the incremental rebuild the caches have to keep up
with. The warm scenario has no Cargo row, because with an empty `target/`
Cargo would repeat the build that warmed the store.

## The scenarios

### Warm build

A first build warms the store, then `target/` is wiped and the same commit
builds again. This is a runner restoring its cache and building something it
has already seen.

### Next commit

The store is warmed at one commit and the build runs at the next. Most of the
dependency graph is unchanged and a few crates are not. Cargo's row is a cold
build, since with an empty `target/` that is all it can do.

### Local edit

A full build, then one line of hk's own source changed and rebuilt in the same
`target/` with incremental compilation on. This measures the edit/build loop,
including cache bookkeeping and incremental compilation. Two details make the
comparison useful:

- `CI` is unset for every tool. mbx switches
  [learned incremental reuse](/incremental#learned-incremental-reuse) off
  in CI because fresh runners have no earlier edit state to reuse.
- The first edit establishes incremental state; the second supplies the
  headline timing. Cargo's
  own build already wrote its incremental state, while mbx builds an edited
  crate's [private state](/incremental#learned-incremental-reuse) on the
  first edit and reuses it afterwards. The card shows what that first edit
  cost, since a developer waits for it once per fresh build.

### Six parallel jobs

Six overlapping Rust CI jobs from an empty store: default and
all-targets/all-features variants of `cargo check`, Clippy, and test
compilation. The sequential row runs them in turn in one `target/`. The two
parallel rows give each job its own `target/`, as separate CI steps would,
and differ only in whether the
[machine-wide scheduler](/scheduling#machine-wide-compile-scheduling) is
on. Cargo bounds the compilers it starts itself and knows nothing about the
Cargo process beside it; the scheduler gives every process one pool of permits
and holds identical compilations until the first finishes. Peak compilers and
lowest free memory show whether a faster batch shared the machine or
oversubscribed it.

## Keeping it fair

- The registry is fetched once, before any timed build. No cell is timed
  while it downloads crates.
- Every trial starts from a fresh clone, an empty store, and a new `target/`.
  Nothing carries over between tools or between runs.
- The toolchain is pinned. hk does not pin one, and a runner-image Rust bump
  would change every cache key at once and look like a cache that stopped
  working.
- `CARGO_INCREMENTAL=0` matches CI everywhere except the edit scenario. Any
  inherited `RUSTC_WRAPPER` is cleared, and the run fails if the Cargo
  baseline turns out to be an mbx shim.
- Both caches run local-only. A remote would measure the network.
- Validity checks reject runs that do not exercise the intended cache behavior. That is any
  run where a warm build restored nothing or was no faster than the build that
  seeded it, where the edit rebuild compiled nothing, or where the scheduled
  contention batch went past its permits or the unscheduled one never did.

## Running it yourself

```sh
mise run bench
```

That builds mbx, clones hk, and runs the warm, commit, and edit scenarios once
each. kache is included when it is on `PATH` and noted as skipped otherwise.
`mise run bench:refresh` is what CI runs: every scenario, three trials each,
written to `benchmarks/results.json`.

The
[bench-refresh workflow](https://github.com/jdx/mr-boxington/actions/workflows/bench-refresh.yml)
checks weekly and reruns when the recorded mbx version differs from the latest
release. It measures that released version and opens a pull request with the
results. Manual runs can force a refresh or measure source changes without
publishing them.

## What this does not measure

These results describe the pinned hk workload. A project with a very different dependency shape,
such as heavy proc macros, a large C component, or many small leaf crates,
will see different ratios. Three trials expose some variation; their ranges
are not confidence intervals, and a “fastest” label is a display heuristic,
not a statistical significance test. The benchmark is Linux-only, and
[limits](/limits) covers what changes on macOS and Windows.

Instruction-counted measurements of mbx's own startup path, and cold and warm
correctness runs against this workspace, live in
[`benchmarks/`](https://github.com/jdx/mr-boxington/tree/main/benchmarks).
