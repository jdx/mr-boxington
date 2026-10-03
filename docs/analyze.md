---
description: Rank where a build's uncached compiler time went, and see what would remove each cause.
---
# Analyzing a build

`mbx analyze` reads the most recent recorded build of the current workspace,
groups its uncached compiler time by cause, largest first, and shows the chain
of units the build waited on. It runs nothing.

```sh
cargo check
# edit engine/src/lib.rs
cargo check
mbx analyze
```

In a workspace where `api` depends on `engine` and `cli` depends on `api`, the
report after editing `engine` begins like this:

```text
last recorded build: cargo check
compiler time: 47.7ms in 3 uncached compilations

uncached compiler time by cause

 47.7ms  inputs of engine changed (3)
         engine 17.1ms
         then 2 crates that depend on it rebuilt: cli 17.2ms and api 13.4ms
         Every crate that depends on engine recompiled after it changed. Code
         that changes often costs less in a crate few others depend on.
```

Each cause lists its compiler time, the number of compilations charged to it,
the costliest crates, and what would remove it. The
[critical path](#critical-path) follows the causes. Use
[`mbx explain --last`](/cli/explain) for the key details of a single crate.

## Causes

Each uncached compilation is charged to one cause. A crate that rebuilt only
because an artifact it consumes changed is charged to the crate where the
change started. mbx follows that chain through as many dependency levels as
the recordings show, so an edit to a crate with many dependents appears as one
cause.

| Cause | Meaning |
| --- | --- |
| `inputs of <crate> changed` | The crate's sources changed, or the crate is a dependency that changed but did not recompile in this build; a crate edited in the same build as its dependency counts here too |
| `compiler arguments changed` | Different flags, profile, or features than the last recording; a `changed:` line counts each flag |
| `environment changed` | A variable that is part of the key had a different value; the `changed:` line names it |
| `the Rust toolchain changed` | Every key includes the compiler, so each crate rebuilds once |
| `the mbx key format changed` | A new mbx version computes keys differently |
| `the linker changed` | A native link's key names its linker |
| `other key details changed` | A part of the key outside the categories above changed (a `changed:` line names it), or the key changed with no changed source or dependency artifact behind it; `mbx explain --last` shows each crate's key details |
| `results missing for keys built before` | The key matched an earlier recording, but its result had been evicted or was absent from the remote |
| `keys built before, but not looked up` | An earlier build produced the same key, but this build had no [prediction](/how-it-works#prediction-and-dep-info) naming it |
| `misses with no earlier recording` | Nothing recorded explains the miss; the store may be new or its history expired |
| `first build of these compilations` | No earlier recording and no key to look up |
| `not cacheable: <reason>` | mbx bypassed the compilation; see [caching limits](/limits) |

Cargo moves a crate's metadata hash, output file names, and `--extern` paths
whenever a dependency changes. mbx treats those arguments as consequences of
the dependency change and never lists them as the flag that changed.

Compilations with nothing to cache, such as Cargo's compiler queries, are
listed on one final `expected, nothing to cache:` line. They count as uncached
work only when they took compiler time.

A crate you are editing keeps private [incremental state](/incremental)
instead of publishing its result, so a later build does not restore it from
the cache even when the report names it as a first build.

## What it compares against

mbx compares each compilation with this workspace's most recent earlier
recording of the same compilation unit. When this workspace has none, mbx uses
the most recent one from another checkout of the same project.
`mbx explain --last` uses the same baseline. Recordings come from
[session history](/tui#recording), which keeps a week of builds, at most 256
of them.

## Critical path

After the causes, the report lists the chain of units the build waited on. A
unit is one rustc compilation or one build-script run; a C compiler that a
build script starts runs inside its build script's unit. Walking back from the
unit that finished last, each step follows the dependency that became ready
last. Because build-script runs are units, a slow `build.rs` appears where the
crates that read its output waited for it. A build-script run is recorded as a
unit only while
[`build_script_execution`](/configuration#build-script-execution) is on, which
is the default.

```text
critical path: 531.3ms, 100% of the 531.3ms between the first unit starting and the last finishing
121.4ms  api build script (compile)
313.1ms  api build script
 35.1ms  api, 3.7ms of it waiting to start
 61.7ms  cli
         The build could not finish sooner than this chain; other work overlapped
         with it. Shortening a step, or removing a dependency between two steps,
         shortens the build.

only one recorded unit running for 483.5ms: api build script 308.0ms, api build script (compile) 86.0ms and cli 58.1ms
```

Each step is credited with the time it added to the chain, and the steps add
up to the path. A crate that Cargo starts once its dependency's metadata is
ready is credited from its own start. Waiting to start is the time between the
last recorded dependency becoming ready and the unit starting: Cargo waiting
for a free job, or a dependency the recording does not name. A step shows its
wait when the wait is at least a tenth of the step. Consecutive steps under
one percent of the path are folded into one `shorter steps` line.

The last line reports how long exactly one recorded unit was running, and
which units ran alone.

## Wrapper phase traces

mbx times each rustc compilation, C/C++ cache attempt, and build-script run
in phases: startup, key construction, cache lookup, blob transfer, restore,
store, prediction recording, compiler execution, and scheduler waits.
`MBX_STATS_REPORT` includes the phase totals as `wrapper_phases_ns`. The
durations are cumulative across wrappers and exclusive: nested work is
subtracted from its parent phase. They do not add up to build wall time
because compilers run in parallel. Work without a phase is reported as
`unattributed`.

To inspect a saved build in Perfetto, export its [session file](/tui#recording):

```sh
mbx cache trace "$(mbx cache dir)/sessions/v1/<session>.jsonl" > trace.json
```

Open `trace.json` in [Perfetto](https://ui.perfetto.dev). Each wrapper process
has its own lane, with nested phases under the invocation. Trace spans are
bounded to 512 per wrapper; totals continue accumulating after that limit.

What the timings cover:

- Telemetry delivery itself is excluded.
- A rustc compilation that bypasses the cache is still timed through its
  scheduler wait and compiler run.
- A build-script run is timed through the script, including an uncached
  fallback, while `build_script_execution` is on. The script's own run has no
  phase, so its time counts as `unattributed`.
- A declined C/C++ cache attempt stops timing before the compiler runs without
  the cache.
- mbx does not time rustdoc invocations.

## Limits

- Cause times are compiler wall time. Compilations overlap, so their total is
  not the build's duration; the critical path is the part the build waited on.
- Build-script runs appear on the critical path but not in the cause ranking,
  which covers compilations.
- mbx identifies units from the hashes Cargo puts in compiler arguments and
  paths: a compilation's `-C extra-filename`, the `--extern` artifacts it
  reads, and build-script directories (`build/<package>-<hash>`, or
  `build/<package>/<hash>` from Cargo 1.100). A rustc compilation without a
  Cargo hash of its own, such as one run by hand, is not on the path.
- Dependents are matched by crate name. A crate built for both the host and
  the target shares one verdict.
- Bypasses recorded without a compiler time, such as rustdoc and C compiler
  bypasses, show a dash in place of a time.
- A build that reached the per-session size limit is analyzed only as far as
  it was recorded. Raise [`events_max_size`](/configuration#events-max-size)
  to record all of it.
