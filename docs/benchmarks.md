# Benchmarks

mbx is measured against plain Cargo and
[kache](https://github.com/kunobi-ninja/kache) on [jdx/hk](https://github.com/jdx/hk),
a mid-size Rust CLI with C dependencies, pinned to one commit and built with
`cargo build --locked`. Each scenario is a shape CI or a developer actually
hits. The numbers come from a GitHub Actions run, never a laptop, and the page
will not name a fastest tool when the gap is inside run-to-run noise.

<BenchmarkResults />

## Reading the cards

Every timed scenario runs three times per tool from a fresh clone and an
empty store. The bar is the middle run and the whisker through it spans the
fastest and slowest. A tool is marked fastest only when its lead over the next
one is wider than either tool's own whisker; otherwise the card says so and
names nobody.

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
`target/` with incremental compilation on. Almost nothing recompiles, so the
cache's own bookkeeping is most of what shows up. Two details keep it honest:

- `CI` is unset for every tool. mbx switches
  [learned incremental reuse](/configuration#learned-incremental-reuse) off
  when it sees that variable, on the reasoning that a fresh runner never edits
  code. With it set, every edit recompiled the crate in full.
- The first edit after a build is discarded and the second is timed. Cargo's
  own build already wrote its incremental state, while mbx builds an edited
  crate's [private state](/configuration#learned-incremental-reuse) on the
  first edit and reuses it afterwards. The card shows what that first edit
  cost, since a developer waits for it once per fresh build.

### Six parallel jobs

Six overlapping Rust CI jobs from an empty store: default and
all-targets/all-features variants of `cargo check`, Clippy, and test
compilation. The sequential row runs them in turn in one `target/`. The two
parallel rows give each job its own `target/`, as separate CI steps would,
and differ only in whether the
[machine-wide scheduler](/configuration#machine-wide-compile-scheduling) is
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
- A run that measured nothing is discarded rather than rendered. That is any
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
runs weekly, only when the published numbers were measured with an older mbx
than the one on `main`, and opens a pull request rather than publishing
directly.

## What this does not measure

Anything hk does not do. A project with a very different dependency shape,
such as heavy proc macros, a large C component, or many small leaf crates,
will see different ratios. The benchmark is Linux-only, and
[limits](/limits) covers what changes on macOS and Windows.

Instruction-counted measurements of mbx's own startup path, and cold and warm
correctness runs against this workspace, live in
[`benchmarks/`](https://github.com/jdx/mr-boxington/tree/main/benchmarks).
