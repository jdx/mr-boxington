# Performance and correctness measurements

`measure_shim.py` compares a warmed release `mbx-rustc` invocation with the
same trivial compiler invoked directly. It alternates order and gates the
median paired overhead at 2 ms; raw samples and p95 are retained for diagnosing
runner noise.

`measure_builds.py` runs this repository's multi-crate workspace with a cold
target, then recreates that same target and runs from the warmed local cache.
With `--verify`, a third fresh-target build sets `MBX_VERIFY=1` and fails unless
at least one result was qualified with zero divergences. The fixed target path
is intentional: rustc embeds paths in artifacts, so changing the path would
test the separately documented cross-checkout limitation instead.

Both scripts write versioned JSON, Markdown summaries, mbx statistics reports,
and build logs. CI uploads the directory as an artifact for trend analysis.
