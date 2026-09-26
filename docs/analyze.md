---
description: Rank where a build's uncached compiler time went, and see what would remove each cause.
---
# Analyzing a build

`mbx analyze` reads the most recent recorded build of the current workspace
and groups its uncached compiler time by cause, largest first. It runs nothing.

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

## Limits

- Times are compiler wall time. Compilations overlap, so the total is not the
  build's duration, and the largest cause is not necessarily the one the build
  waited on.
- Dependents are matched by crate name. A crate built for both the host and
  the target shares one verdict.
- Bypasses recorded without a compile time, such as rustdoc and C compiler
  bypasses, show a dash in place of a time.
- A build that reached the per-session size limit is analyzed only as far as
  it was recorded. Raise [`events_max_size`](/configuration#events-max-size)
  to record all of it.
