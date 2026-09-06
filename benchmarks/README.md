# Performance and correctness measurements

Use the benchmark that matches the question you are investigating. Run these
commands from the repository root after `mise install`.

| Question | Tool or task | Output |
| --- | --- | --- |
| Did mbx startup get more expensive? | `mise run perf` | Instruction counts and wall time from `tak` |
| How does mbx compare on a real project? | `mise run bench` | One trial of warm, commit, and edit scenarios |
| How are the website's results refreshed? | `mise run bench:refresh` | Three trials of every scenario in `results.json` |
| Are cached builds correct in this workspace? | `measure_builds.py --verify` | Versioned reports, logs, and verification results |

## Startup cost

[`tak.toml`](../tak.toml) measures a no-op process-startup control and the
hermetic `mbx --help` path. `tak` gates deterministic instruction counts and
reports wall time without treating noisy shared-runner timing as a hard gate.

```sh
mise run perf
mise run perf:record
```

Both tasks build the benchmark subject first. `perf:record` appends results to
local `refs/notes/tak` history. CI compares against shared history when a
baseline exists; an initial run seeds that history.

## Cold, warm, and verified builds

`measure_builds.py` builds this workspace with a cold target, recreates the same
target, and builds from the warmed local cache. Add `--verify` for a third
fresh-target build with `MBX_VERIFY=1`; it fails unless at least one result was
qualified with zero divergences.

```sh
mise run build
python3 benchmarks/measure_builds.py \
  --mbx target/mbx-bootstrap/mbx \
  --workspace . \
  --output target/build-measurements \
  --verify
```

The fixed target path is intentional. Compiler artifacts can embed source
paths, so changing the path would test the documented cross-checkout byte
differences as well as correctness.

The script writes versioned JSON and Markdown summaries, mbx statistics, and
build logs. Large target and cache working trees are temporary. CI uploads the
reports and a `tak history` snapshot; trusted main-branch runs publish the
performance series to `refs/notes/tak`.

## Real-world comparison

`real_world.py` clones a pinned commit of [jdx/hk](https://github.com/jdx/hk) and
compares plain Cargo, mbx, and kache. kache is included when it is on `PATH` and
reported as skipped otherwise.

```sh
mise run bench
mise run bench:refresh
```

The scenarios measure a warm store with a fresh target, the next commit, an
in-place source edit, and six overlapping check, Clippy, and test-compilation
jobs. The edit scenario runs with `CI` unset and times the second edit, after
each tool has established its incremental state. The first edit's cost is also
reported. Parallel jobs receive separate targets so Cargo's target lock does
not serialize them.

Each trial starts from a fresh clone and empty store. The registry is fetched
outside timed builds, the toolchain is pinned, inherited wrappers are cleared,
and caches run locally. Validity checks reject runs that did not exercise the
intended behavior, such as a warm build with no restores or a Cargo baseline
that accidentally used an mbx shim. Contention checks verify the permit limit
was exercised; wall-time ordering remains a reported result.

The website shows the median and the full range of three trials. It marks a
tool fastest only when its lead exceeds either tool's range. See the
[benchmark guide](../docs/benchmarks.md) for scenario definitions and limits.

The [refresh workflow](../.github/workflows/bench-refresh.yml) checks weekly,
reruns when the published results predate mbx on `main`, and opens a pull
request with updated data. Do not hand-edit timings to match an expected result.
