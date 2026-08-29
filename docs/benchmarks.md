# Benchmarks

These numbers come from building somebody else's project, not this one. A
compiler cache measured against its own workspace tends to flatter itself: the
workspace is small, the author knows which shapes it avoids, and nothing in it
was written without the cache in mind. So the subject here is
[jdx/hk](https://github.com/jdx/hk) — a mid-size Rust CLI with C dependencies,
pinned to a fixed commit — and every scenario is one CI actually hits.

Most scenarios run the same `cargo build --locked` three ways: plain cargo,
[mbx](/), and [kache](https://github.com/kunobi-ninja/kache). Timings are wall
clock around one build. Every row is compared against one number — what plain
cargo costs with nothing cached — because that is the build a cache is
replacing, and it is the only scenario where running cargo says anything new.
The last scenario is the exception: it compares sequential and parallel lint
strategies and measures the machine rather than a single build.

<BenchmarkResults />

## What each scenario reproduces

**cold** — an empty store and a fresh `target/`. What a new machine, or a CI
job with no cache to restore, actually does. There is nothing to hit, so this
is where a cache can only cost time; a cache that is slower than cargo here is
charging rent on the first build.

**warm** — the store from the cold build, a fresh `target/`. This is the
common CI shape: a runner restores a cache, then builds a commit it has
already seen. cargo is not run, because with a wiped `target/` it would just
repeat its cold number.

**commit** — the store is warmed at one commit and the build runs at the next
one. Push-to-push CI: most of the dependency graph is unchanged, a few crates
are not. cargo's baseline here is a cold build, because that is what cargo
does with an empty `target/`.

**worktree** — the store is warmed in one checkout, and the timed build runs
in a second checkout at a different path. This is the claim that absolute
paths did not enter the keys. A cache that keys on paths reports a cold build
here.

**toolchain** — not a timing. The store is warmed on the pinned Rust and the
build reruns on a different one, and the run fails unless essentially none of
the predicted compilations were even looked up. A compiler change invalidates
every invocation digest at once; the failure mode worth guarding against is a
cache that claims a hit anyway. Not *zero* hits, though: a handful of actions
do not depend on rustc — a build script's C object is compiled by the C
compiler, which did not change — and those legitimately survive.

It also pins down the diagnosis for the opposite surprise. A warm build that
reports no hits after a runner image rolled a new Rust looks like a broken
store and is not one; it is this.

**contention** — the two Clippy configurations from mise's real lint job,
from a cold store: the default configuration and `--all-features
--all-targets`. The `mbx-sequential` row is the before picture, with both
commands sharing one target directory. The parallel rows use separate targets
so Cargo's target lock does not serialize them, matching GitHub Actions'
native `parallel` steps.

All three rows use the same mbx binary. The `mbx` and `mbx-unscheduled` rows
run the parallel shape with the
[machine-wide scheduler](/configuration#machine-wide-compile-scheduling) on
and off, which isolates what MBX contributes from parallelism itself. Cargo
bounds only the compilers *it* starts and knows nothing about the Cargo process
beside it; the scheduler gives both processes one machine-wide permit pool and
deduplicates identical work in flight. The wall clock shows whether the switch
beats the sequential baseline. Peak compilers and lowest free memory show
whether it got there by safely sharing the machine or merely oversubscribing
it.

## How the comparison is kept fair

- **The registry is fetched once**, before any timed build, into a shared
  `CARGO_HOME`. No cell is timed while it downloads crates.
- **The toolchain is pinned** per subject. hk does not pin one itself, and a
  runner-image Rust bump changes every invocation digest simultaneously — that
  reads as a cache that stopped working, and it would land in the series as a
  step change.
- **Each cell gets its own store and `target/`**, created fresh. No tool ever
  benefits from another's leftovers.
- **`CARGO_INCREMENTAL=0`**, matching what CI does, and any `RUSTC_WRAPPER`
  inherited from the caller is cleared so the uncached baseline is really
  uncached.
- **Both caches run local-only.** A remote would measure a network.
- **The published numbers come from one CI run**, so the tools are comparable
  to each other. Shared runners are noisy enough that comparing across runs is
  not meaningful; the provenance line above says which run and which machine.

## Validity gates

A benchmark that quietly measured nothing is worse than one that failed,
because its numbers still render. The run fails, and nothing publishes, unless:

- the warm and cross-worktree builds report cache hits *and* restored output
  files — a fast build that restored nothing was fast for some other reason;
- each warm build beat its own cold build;
- the toolchain-change build loaded predictions and then looked up almost
  none of them — and that it ran at all, because a guard that was skipped is
  not a guard that passed;
- the contention run sampled the machine, saw compilers running, and kept the
  scheduled batch inside its permits — *and* that the unscheduled batch went
  past them, because a bound nothing pushed against proves nothing;
- the scheduled parallel lint finished ahead of its sequential baseline.

A run that fails these is not rendered here at all — the page shows its empty
state rather than numbers it cannot stand behind.

## Running it yourself

```bash
mise run bench
```

That builds mbx, clones the pinned subject, and runs the cold, warm, and
next-commit scenarios. kache is included automatically when it is on `PATH`
and skipped with a note when it is not. `mise run bench:refresh` is what CI
runs: every scenario, writing `benchmarks/results.json`. It also needs
`MBX_BENCH_ALTERNATE_TOOLCHAIN` set to a second installed Rust, since that is
what the compiler-change guard switches to.

The numbers on this page are refreshed by the
[bench-refresh workflow](https://github.com/jdx/mr-boxington/actions/workflows/bench-refresh.yml),
which runs weekly, only when the published numbers were measured with an older
mbx than the one on `main`, and opens a pull request rather than publishing
directly. Nobody's laptop numbers reach this page.

## What this does not measure

Everything hk does not do. hk is a mid-size CLI; it is not Firefox, and a
project with a very different dependency shape — heavy proc macros, a large C
component, many small leaf crates — will see different ratios. It is also
Linux-only: [the limits page](/limits) covers what changes on macOS and
Windows.

For instruction-counted measurements of mbx's own startup path, and
cold/warm correctness runs against this workspace, see
[`benchmarks/`](https://github.com/jdx/mr-boxington/tree/main/benchmarks).
