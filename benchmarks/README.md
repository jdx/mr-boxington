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
situations CI actually hits: a cold store, a warm store with a fresh target,
the next commit on the branch, a second checkout at a different path, and a
compiler change. The last of those is a correctness cell rather than a timing
-- it fails unless a different rustc leaves almost every predicted compilation
unlooked-up, which is the shape the hk benchmark hit when a runner image
rolled a new Rust. Not zero hits: actions that do not depend on rustc, such as
a build script's C objects, legitimately survive a Rust change.

The `contention` scenario is the odd one out: it starts four CI-shaped jobs at
once -- `clippy --all-targets`, `check --all-targets`, `test --no-run`, and
`build` -- against a cold store, and measures the machine rather than the
cache. Cargo bounds only the compilers it starts itself, so four jobs
oversubscribe by four times, which is what runs a link into an out-of-memory
kill. The cells are sampled from outside every build: the most real compilers
alive at once, and the least memory the machine had left. It runs mbx twice,
once with the machine-wide scheduler and once with `MBX_SCHEDULER=0`, because
the comparison worth making is against the same binary.

The comparison is kept fair by fetching the registry once outside every timed
build, pinning the toolchain (hk does not pin one itself), giving each cell
its own store and target, clearing any inherited `RUSTC_WRAPPER`, and running
both caches local-only. Validity gates reject a run whose warm builds restored
nothing or were no faster than cold, because those numbers would still render.
The contention gates are the same idea from the other side: the scheduled
batch must stay inside its permits, and the unscheduled one must exceed them,
since a bound nothing pushed against proves nothing.

`mise run bench` runs the everyday subset; `mise run bench:refresh` runs
everything and rewrites `results.json`, which the documentation site reads.
The refresh needs `MBX_BENCH_ALTERNATE_TOOLCHAIN` set to a second installed
Rust, and fails without it: a skipped guard is not a passed guard, and a run
that could not check compiler invalidation must not publish as though it had.
kache is used when it is on `PATH` and skipped with a note otherwise. The
[bench-refresh workflow](../.github/workflows/bench-refresh.yml) runs it
weekly, only when the published numbers predate the mbx on `main`, and opens a
pull request rather than publishing directly.
