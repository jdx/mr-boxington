# Benchmarks

The subject is [jdx/hk](https://github.com/jdx/hk), a mid-size Rust CLI with C
dependencies, pinned to a fixed commit. mbx's own workspace is too small to
measure anything useful against. Every scenario is one CI actually hits.

The build scenarios run the same `cargo build --locked` through plain Cargo,
[mbx](/), and [kache](https://github.com/kunobi-ninja/kache), where each tool
can make a meaningful comparison. Timings are wall clock around one build.
When Cargo appears, it is an uncached baseline measured in that same scenario;
cache rows compare only with that result. The warm scenario omits Cargo
because a fresh `target/` gives it nothing to reuse. The contention scenario
compares sequential and parallel lint strategies and measures the machine
instead of a single build.

Every timed scenario runs three times per tool. The card shows the middle run
and the range across all three, and it names a tool fastest only when the gap
to the next one is larger than the range one of them covered on its own. A
scenario that cannot clear that bar says so and names nobody.

<BenchmarkResults />

## What each scenario reproduces

### warm

A store warmed by a first build of the same commit, then a fresh `target/`.
This is the common CI shape: a runner restores a cache, then builds a commit
it has already seen. cargo is not run, because with a wiped `target/` it would
repeat that first build.

### commit

The store is warmed at one commit and the build runs at the next one.
Push-to-push CI: most of the dependency graph is unchanged, a few crates are
not. cargo's baseline here is a cold build, because that is what cargo does
with an empty `target/`.

### edit

A full build, one line changed in the subject's own source, and a rebuild in
the same `target/` with incremental compilation on. This is the local loop
rather than a CI job, and it is the shape where a cache has the least to offer
and the most to get in the way: almost nothing needs rebuilding, so anything
the cache spends on bookkeeping is the whole difference. Cargo is the thing to
beat here, not a control.

### worktree

Not a timing. The store is warmed in one checkout and the build reruns in a
second checkout at a different path. It passes when the second build restores
outputs from the first, which is what says absolute paths did not enter the
keys. A cache that keys on paths rebuilds everything here, and the seconds
would not tell you which happened.

### toolchain

Not a timing. The store is warmed on the pinned Rust and the build reruns on a
different one. The run fails unless almost none of the predicted compilations
were looked up. A compiler change invalidates every invocation digest at once,
and the failure this guards against is a cache that claims a hit anyway. A
handful of hits are expected: a build script's C object is compiled by the C
compiler, which did not change, so those actions survive.

This also explains a warm build that reports no hits after a runner image
picked up a new Rust. The store is not broken; the compiler changed.

### contention

Six overlapping Rust CI jobs from an empty store: default and all-targets/all-
features variants of `cargo check`, Clippy, and test compilation. The
`sequential` row is context, with the commands sharing one target directory.
The parallel rows use separate targets so Cargo's target lock does not
serialize them, matching separate CI steps. The scheduler comparison is
between those two parallel rows.

All three rows use the same mbx binary. The `mbx` and `mbx-unscheduled` rows
run the parallel shape with the
[machine-wide scheduler](/configuration#machine-wide-compile-scheduling) on
and off, which isolates what mbx contributes from parallelism itself. Cargo
bounds only the compilers it starts and knows nothing about the Cargo process
beside it; the scheduler gives both processes one machine-wide permit pool and
deduplicates identical work in flight. The wall clock shows whether the switch
beats the sequential baseline. Peak compilers and lowest free memory show
whether it got there by sharing the machine or by oversubscribing it.

## How the comparison is kept fair

- The registry is fetched once, before any timed build, into a shared
  `CARGO_HOME`. No cell is timed while it downloads crates.
- Trials share nothing. Each one clones the subject again and starts from its
  own empty store, so the second run measures the same scenario as the first.
- The toolchain is pinned per subject. hk does not pin one itself, and a
  runner-image Rust bump changes every invocation digest at once, which would
  land in the series as a step change that looks like a cache that stopped
  working.
- Each cell gets its own store and `target/`, created fresh. No tool benefits
  from another's leftovers.
- `CARGO_INCREMENTAL=0` is set, matching CI, and any `RUSTC_WRAPPER` inherited
  from the caller is cleared so the uncached baseline is uncached.
- Both caches run local-only. A remote would measure a network.
- Every scenario's rows come from one CI run, so the tools within it are
  comparable. A scenario refreshed separately carries its own run link;
  otherwise the provenance line below the cards applies.

## Validity gates

A benchmark that measured nothing still renders numbers, so the run fails and
nothing publishes unless:

- the warm and cross-worktree builds report cache hits and restored output
  files; a fast build that restored nothing was fast for some other reason;
- each warm build beat the cold build that seeded it, which is the same tool
  in the same checkout on the same machine minutes earlier;
- the edit rebuild actually compiled something, since an edit that never
  reached the compiler renders as a very fast rebuild rather than as a
  broken run;
- no Cargo baseline ran under mbx. Clearing the wrapper variables does not
  prove they were the only way in, and a `cargo` that is itself an mbx shim
  produces a baseline that quietly uses the cache it is the control for;
- the toolchain-change build ran, loaded predictions, and then looked up
  almost none of them; a guard that was skipped is not a guard that passed;
- the contention run sampled the machine, saw compilers running, and kept the
  scheduled batch inside its permits, and the unscheduled batch went past
  them; a bound nothing pushed against proves nothing.

A run that fails these is not rendered here at all. The page shows its empty
state instead.

## Running it yourself

```sh
mise run bench
```

That builds mbx, clones the pinned subject, and runs the warm, next-commit,
and edit scenarios once each. kache is included when it is on `PATH` and
skipped with a note when it is not. `mise run bench:refresh` is what CI runs:
every scenario, three trials of each timed one, writing
`benchmarks/results.json`. It needs `MBX_BENCH_ALTERNATE_TOOLCHAIN` set to a
second installed Rust, which is what the compiler-change guard switches to.
`--trials` sets the repeat count; one trial publishes a timing with no range
beside it, and the page will not name a fastest tool from it.

The numbers on this page are refreshed by the
[bench-refresh workflow](https://github.com/jdx/mr-boxington/actions/workflows/bench-refresh.yml),
which runs weekly, only when the published numbers were measured with an older
mbx than the one on `main`, and opens a pull request instead of publishing
directly. Nobody's laptop numbers reach this page.

## What this does not measure

Everything hk does not do. hk is a mid-size CLI, and a project with a very
different dependency shape (heavy proc macros, a large C component, many small
leaf crates) will see different ratios. It is also Linux-only:
[the limits page](/limits) covers what changes on macOS and Windows.

For instruction-counted measurements of mbx's own startup path, and
cold/warm correctness runs against this workspace, see
[`benchmarks/`](https://github.com/jdx/mr-boxington/tree/main/benchmarks).
