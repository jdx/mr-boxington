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
