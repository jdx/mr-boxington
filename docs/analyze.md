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

In a workspace where `api` depends on `engine` and `cli` depends on `api`,
editing `engine` produces:

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
the costliest crates, and what would remove it. Use
[`mbx explain --last`](/cli/explain) when you need the key details of a single
crate.

## Causes

A crate whose own key did not change, but which consumes an artifact that did,
is charged to the crate where the change started. This follows as many
dependency levels as the recordings show, so an edit to a crate with many
dependents appears as one cause.

| Cause | Meaning |
| --- | --- |
| `inputs of <crate> changed` | The crate's sources changed, or a dependency that did not recompile in this build |
| `compiler arguments changed` | Different flags, profile, or features than the last recording; a `changed:` line counts each flag |
| `environment changed` | A variable that is part of the key had a different value; the `changed:` line names it |
| `the Rust toolchain changed` | Every key includes the compiler, so each crate rebuilds once |
| `the mbx key format changed` | A new mbx version computes keys differently |
| `the linker changed` | A native link's key names its linker |
| `results missing for keys built before` | The key matched an earlier recording, but its result had been evicted or was absent from the remote |
| `misses with no earlier recording` | Nothing recorded explains the miss; the store may be new or its history expired |
| `first build of these compilations` | No earlier recording and no key to look up; the result was stored for the next build |
| `not cacheable: <reason>` | mbx bypassed the compilation; see [caching limits](/limits) |

Cargo moves a crate's metadata hash, output file names, and `--extern` paths
whenever a dependency changes. Those arguments are treated as consequences of
the dependency change and never listed as the flag that changed.

Compilations with nothing to cache, such as Cargo's compiler queries, are
listed on one final line and not counted as uncached work.

## What it compares against

Each compilation is compared with the most recent earlier recording of the
same compilation unit, from this workspace or another checkout of the same
project, the same baseline `mbx explain --last` uses. Recordings come from
[session history](/tui#recording), which keeps a week of builds, at most 256
of them.

## Critical path

After the causes, the report lists the chain of units the build waited on.
Walking back from the unit that finished last, each step follows the
dependency that became ready last. Build-script runs are units too, so a slow
`build.rs` appears where the crates that read its output waited for it.

```text
critical path: 531.3ms, 100% of the 531.3ms between the first unit starting and the last finishing
121.4ms  api build script (compile)
313.1ms  api build script
 35.1ms  api, 3.7ms of it waiting to start
 61.7ms  cli

only one unit running for 483.5ms: api build script 308.0ms, api build script (compile) 86.0ms and cli 58.1ms
```

Each step is credited with the time it added to the chain, and the steps add
up to the path. A crate that Cargo starts once its dependency's metadata is
ready is credited from its own start. Waiting to start is time between the
last recorded dependency becoming ready and the unit starting: Cargo waiting
for a free job, or a dependency the recording does not name. Steps under one
percent of the path are folded into one line.

The last line reports how long exactly one unit was running, and which units
ran alone. Nothing else was building during that time.

## Limits

- Cause times are compiler wall time. Compilations overlap, so their total is
  not the build's duration; the critical path is the part the build waited on.
- Build-script runs appear on the critical path but not in the cause ranking,
  which covers compilations.
- Dependents are matched by crate name. A crate built for both the host and
  the target shares one verdict.
- Bypasses recorded without a compile time, such as rustdoc and C compiler
  bypasses, show a dash in place of a time.
- A build that reached the per-session size limit is analyzed only as far as
  it was recorded. Raise [`events_max_size`](/configuration#events-max-size)
  to record all of it.
