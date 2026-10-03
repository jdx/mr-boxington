---
description: Compare Cargo, mbx, and kache on two pinned Rust projects with documented trials and workload limitations.
---
# Benchmarks

<script setup>
import { data as aube } from './.vitepress/aube-benchmarks.data'
</script>

These benchmarks compare mbx with plain Cargo and
[kache](https://github.com/kunobi-ninja/kache) on two pinned Rust projects,
built with `cargo build --locked`. hk shows a change to one application crate;
aube shows changes propagating through a workspace. Published results come
from GitHub Actions. A tool is labeled fastest only when its lead exceeds the
observed variation between trials.

## hk: one application crate changes

[jdx/hk](https://github.com/jdx/hk) is a mid-size Rust CLI with C dependencies.
Its next-commit build recompiles one crate while restoring its dependencies.
This is the workload featured in the landing-page showreel.

<BenchmarkResults />

## aube: changes propagate through a workspace

[jdx/aube](https://github.com/jdx/aube) is a 15-crate Rust workspace with C
dependencies. Its next commit changes two libraries and rebuilds their
dependents, exposing the work that whole-compilation caching cannot reuse.

<BenchmarkResults :results="aube" prefix="aube-" />

Below each project's charts, a line names the mbx version, Rust version, and
CI run that produced its results. Historical runs can use different mbx
releases, so compare tools within one project's run. A refresh measures both
projects with the same mbx release.

## Read the results {#reading-the-cards}

Each scenario has three independent trials per tool. A trial starts with a
fresh clone and empty store, then performs the scenario's warm-up and measured
builds. The bar shows the median; the whisker spans the fastest and slowest
trial. A tool is marked fastest only when its lead exceeds both tools' trial
ranges. Otherwise, the card reports no clear winner.

The Cargo row means different things in different scenarios, so the card tags it.
In the new-worktree and commit scenarios it is the uncached baseline: the cold
build a fresh checkout or CI runner does without a cache. In the edit scenario
it is the incremental rebuild the caches have to keep up with. The warm
scenario has no Cargo row, because with an empty `target/` Cargo would repeat
the build that warmed the store. The six-parallel-jobs scenario has no Cargo
row either, because every row there runs mbx.

## The scenarios

### Warm build

A first build warms the store, then `target/` is wiped and the same commit
builds again. This is a runner restoring its cache and building something it
has already seen.

### New worktree

A build at one commit warms the local store. Then the same commit builds in a
new Git worktree with no target directory of its own. Each tool uses its
default target placement, and the benchmark harness does not copy the first
worktree's target directory. The scenario tests reuse across checkout paths,
including dependency target seeding when the tool and Cargo support it. Cargo's
cold build provides the uncached baseline.
[Start new checkouts from existing units](/managed-targets#start-new-checkouts-from-existing-units)
describes how mbx seeds a new checkout.

Results published before this scenario was added do not include it, so a
project whose current run predates it has no worktree card. The line under
each project's charts names the run that produced its cards.

### Next commit

The store is warmed at one commit and the build runs at the next. hk's next
commit changes its application crate. For aube, the next commit changes two
library crates, so they and the workspace crates that depend on them compile
again while the rest of the graph is restored. Cargo's row is a cold build,
since with an empty `target/` that is all it can do.

### Local edit

A full build runs first. Then one line changes in hk's `src/main.rs` or in
aube's `aube-util`, which most of aube's workspace depends on, and the project
rebuilds in the same `target/` with `CARGO_INCREMENTAL=1`. mbx overrides that,
as it does by default, and uses learned incremental reuse instead. This measures
the edit/build loop, including cache bookkeeping and incremental compilation.
Two details make the comparison useful:

- The harness unsets `CI` for every tool. mbx switches
  [learned incremental reuse](/incremental#learned-incremental-reuse) off in
  CI because fresh runners have no earlier edit state to reuse.
- The first edit establishes incremental state; the second supplies the
  headline timing. Cargo's full build already wrote its incremental state,
  while mbx builds an edited crate's learned incremental state on the first
  edit and reuses it afterwards. The card also shows what that first edit
  cost, since you wait for it once per fresh build.

### Six parallel jobs

Six overlapping Rust CI jobs from an empty store: default and
all-targets/all-features variants of `cargo check`, Clippy, and test
compilation. Every row runs mbx; this scenario compares mbx with itself and
has no Cargo or kache row. The sequential row runs the jobs in turn in one
`target/` with the scheduler on. The two parallel rows give each job its own
`target/`, as separate CI steps would, and differ only in whether the
[machine-wide scheduler](/scheduling#machine-wide-compile-scheduling) is on.

Cargo bounds the compilers it starts itself and knows nothing about the Cargo
process beside it; the scheduler gives every process one pool of permits and
holds identical compilations until the first finishes. Peak compilers and
lowest free memory show whether a faster batch shared the machine or
oversubscribed it.

## Fairness controls {#keeping-it-fair}

- The registry is fetched once, before any timed build, so no timed build
  includes crate downloads.
- Every trial starts from a fresh clone, an empty store, and a new `target/`.
  Nothing carries over between tools or between runs.
- Like your own build, the harness uses whichever Rust toolchain the runner
  provides. The `toolchain` field in each results file records which release
  produced the numbers. Every trial in a run uses that one compiler, so the
  tools are compared fairly against each other. A Rust upgrade between runs
  changes every cache key at once, which reads as a cache that stopped
  working. Check that field before treating a drop across runs as a
  regression.
- The harness sets `CARGO_INCREMENTAL=0`, as CI does, in every scenario except
  the edit scenario. It clears any inherited `RUSTC_WRAPPER` or
  `RUSTC_WORKSPACE_WRAPPER`, and the run fails if the Cargo baseline turns out
  to run under mbx, for example through the Cargo shim `mbx setup` installs.
- Both caches run local-only, since a remote would measure the network. The
  harness runs kache with `KACHE_LOCAL_ONLY=1`. It does not clear an mbx
  remote cache; published results are local-only because the CI runner that
  produces them configures none.
- The harness rejects a run that does not exercise the intended cache
  behavior. A run fails when:
  - a warm build restored nothing, or ran no faster than the build that
    seeded it
  - mbx wrote no build report (`MBX_STATS_REPORT`) for a new-worktree build
  - an edit rebuild compiled nothing
  - the scheduled contention batch went past its permits, or the unscheduled
    one never did

## Run the benchmarks yourself {#running-it-yourself}

```sh
mise run bench
```

That builds mbx, clones aube, and runs the warm, new-worktree, commit, and
edit scenarios once each. kache is included when it is on `PATH` and noted as
skipped otherwise.

The harness does not turn off an mbx remote cache, so with one configured the
mbx rows include network restores. Set `MBX_REMOTE_URL` to an empty value for
the run; it overrides `remote.url` in your
[configuration file](/configuration):

```sh
MBX_REMOTE_URL= mise run bench
```

`mise run bench:refresh` runs CI's full measurement against the mbx built from
your checkout: both projects across every scenario, three trials each, written
to `benchmarks/results.json` (hk) and `benchmarks/results-aube.json` (aube).

CI runs the same harness against the latest release instead. The
[bench-refresh workflow](https://github.com/jdx/mr-boxington/actions/workflows/bench-refresh.yml)
checks weekly and reruns when the recorded mbx version differs from the latest
release. It measures that released version and opens a pull request with the
results. On `main`, a manual run with `force` also opens or updates that PR,
even when the recorded version already matches. Set `dry_run` to measure
without proposing an update, or `source` to measure the selected source ref
instead of the latest release; `source` implies `dry_run`.

## What this does not measure

These results describe the two pinned workloads. A project with a different
dependency shape, such as heavy proc macros, a large C component, or many
small leaf crates, will see different ratios. Three trials expose some
variation; their ranges are not confidence intervals, and a “fastest” label is
a display heuristic, not a statistical significance test. The benchmark is
Linux-only, and [Caching limits](/limits) covers what changes on macOS and
Windows.

Instruction-counted measurements of mbx's own startup path, and cold and warm
correctness runs against this workspace, live in
[`benchmarks/`](https://github.com/jdx/mr-boxington/tree/main/benchmarks).
