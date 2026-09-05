# Performance and correctness measurements

[`tak.toml`](../tak.toml) measures a no-op process-startup control and the
hermetic `mbx --help` startup path. `tak` gates deterministic instruction counts
and reports wall time without using noisy shared-runner timing as a hard gate.
The pinned `mise run perf` task builds the subject and runs both benchmarks;
`mise run perf:record` appends the result to the local `refs/notes/tak` history.
PR CI compares against its base once shared history exists. The first merged
main run seeds that history, so the adoption PR reports measurements without a
fabricated regression baseline.

`measure_builds.py` runs this repository's multi-crate workspace with a cold
target, then recreates that same target and runs from the warmed local cache.
With `--verify`, a third fresh-target build sets `MBX_VERIFY=1` and fails unless
at least one result was qualified with zero divergences. The fixed target path
is intentional: rustc embeds paths in artifacts, so changing the path would
test the separately documented cross-checkout limitation instead.

`measure_builds.py` writes versioned JSON and Markdown summaries plus mbx
statistics reports and build logs. The large target and cache working trees
are temporary and are not retained. CI uploads the correctness measurements
and a `tak history` snapshot as artifacts; trusted main-branch runs publish the
shared performance series to `refs/notes/tak`.

`real_world.py` measures somebody else's project instead of this one. It
clones a pinned checkout of [jdx/hk](https://github.com/jdx/hk) and runs the
same `cargo build --locked` under raw cargo, mbx, and kache across the
situations CI actually hits: a warm store with a fresh target, the next commit
on the branch, and a one-line edit rebuilt in place with incremental
compilation on. The edit scenario is the local loop rather than a CI job, and
the one where a cache has the most to get in the way: almost nothing needs
rebuilding, so its bookkeeping is the whole difference.

Each timed scenario runs `--trials` times per tool, three in CI. Trials share
nothing, so the second measures the same scenario as the first. The published
cell is the middle trial, with every trial's timing carried beside it: without
knowing how far one tool moved between its own runs, a reader cannot tell a
result from the machine, and the site refuses to name a fastest tool when the
gap between the top two is inside that range.

Two scenarios assert instead of racing, and publish no timing at all. The
cross-worktree scenario warms a store in one checkout and rebuilds in a second
at a different path; it passes when the second build restores the first one's
outputs, which is what says absolute paths did not enter the keys. The
compiler-change scenario fails unless a different rustc leaves almost every
predicted compilation unlooked-up, which is the shape the hk benchmark hit
when a runner image rolled a new Rust. Not zero hits: actions that do not
depend on rustc, such as a build script's C objects, legitimately survive a
Rust change. Both were once timing cards, and neither was ever a race: on a
fast runner their tools finish within a second of each other, and the seconds
said nothing the hit counts did not say better.

The `contention` scenario is the odd one out: it stacks six overlapping check,
Clippy, and test-compilation jobs on one runner. It measures sequential context,
native-step parallelism with mbx scheduling, and the same parallel shape with
`MBX_SCHEDULER=0`, all from cold stores. The parallel commands get separate
targets so Cargo's target lock cannot serialize them; the sequential reference
keeps its shared target. The cells are sampled from outside every build: the
most real compilers alive at once, and the least memory the machine had left.

The comparison is kept fair by fetching the registry once outside every timed
build, pinning the toolchain (hk does not pin one itself), giving each cell
its own store and target, clearing any inherited `RUSTC_WRAPPER`, and running
both caches local-only. Validity gates reject a run whose warm builds restored
nothing or were no faster than the build that seeded them, whose edit rebuild
compiled nothing, or whose cargo baseline turns out to have run under mbx
after all: clearing the wrapper variables does not prove they were the only
way in, and a `cargo` that is itself an mbx shim would flatter the control at
mbx's expense with nothing in the numbers to show it.
The contention gates verify that the scheduled batch stays inside its permits
and the unscheduled one exceeds them, since a bound nothing pushed against
proves nothing. Wall-time ordering remains a reported benchmark result rather
than a validity condition because a single shared-runner sample is noisy.

`mise run bench` runs the everyday subset once through; `mise run
bench:refresh` runs everything, three trials of each timed scenario, and
rewrites `results.json`, which the documentation site reads.
The refresh needs `MBX_BENCH_ALTERNATE_TOOLCHAIN` set to a second installed
Rust, and fails without it: a skipped guard is not a passed guard, and a run
that could not check compiler invalidation must not publish as though it had.
kache is used when it is on `PATH` and skipped with a note otherwise. The
[bench-refresh workflow](../.github/workflows/bench-refresh.yml) runs it
weekly, only when the published numbers predate the mbx on `main`, and opens a
pull request rather than publishing directly.
