---
description: Read cache counters, measure reuse with fresh targets, and diagnose hits, misses, bypasses, and remote failures.
---
# Cache results

Use the build summary to see what mbx restored, compiled, or left uncached.
Counts describe the compiler actions and cached build-script runs observed by
mbx; they do not include work Cargo skipped because its outputs were already
fresh. Each lookup ends as one hit or one miss, so the two add up to the
lookup count. When verification selects a compilation (`MBX_VERIFY=1` or a
sample under `verify_sample_rate`), a lookup that finds a result counts as a
verification instead: the summary reports it as `verified`, and also as
`diverged` when the outputs differ. See
[Verify restored outputs](/troubleshooting#verify-mode).

```sh
mbx explain --last          # inspect the most recent recorded build
MBX_SUMMARY=full mbx build  # print a detailed report for a new build
```

| Outcome | Cache lookup? | Result stored? |
| --- | --- | --- |
| [Hit](#hit) | Found a result | Already stored |
| [Miss](#miss) | No matching result | After successful compilation |
| [Not looked up](#could-not-look-up) | No dep-info or matching prediction | After successful compilation |
| [Bypass](#bypass) | Skipped | No shared result |
| [Incremental](#incremental) | Only if looked up before switching | No; kept as private state |

## Measure cache reuse

Running the same command twice in the same target directory often measures
Cargo's freshness check: no compiler work is needed. To observe mbx reuse,
build equivalent source with the same toolchain, profile, and features into
two fresh target directories:

```sh
mbx build --target-dir target/cache-demo-first
mbx build --target-dir target/cache-demo-second
```

Use directory names that do not already contain build outputs. This keeps your
normal target intact and avoids deleting the shared cache. The second build
can restore work recorded by the first; unsupported actions still run.
These explicit targets are not managed, so remove the two example directories
when you finish. For repeatable timings, use the [benchmark harness](/benchmarks).

Compiler time avoided is summed across actions. It is not elapsed time saved;
always compare wall-clock build time as well as the counters.

## Hit

mbx derives the action key, finds a stored result, and restores its outputs.
The result can come from the local store or a configured remote.

## Miss

mbx derives the action key and looks it up, but finds no result. The compiler
runs, and mbx stores a successful result unless the compilation keeps
[private incremental state](#incremental).

## Not looked up {#could-not-look-up}

mbx has neither dep-info for this compilation nor a matching
[prediction](/how-it-works#prediction-and-dep-info), so it cannot derive the
action key. This is common on a cold build. mbx makes no lookup, so counting
the compilation as a miss would overstate the number of failed lookups. Unless
the compilation keeps [private incremental state](#incremental), mbx still
stores its result afterwards.

A crate that depends on a crate with private incremental state also counts
here, because mbx skips its lookup; see [Incremental](#incremental). The short
summary counts these compilations as `not looked up`. The full summary prints a
`could not look up` line with the reason, and its
[compiler time](#compiler-time) line labels them `unconsulted`, as
[`mbx tui`](/tui) does.

## Bypass

mbx recognizes that it cannot model the action exactly, so it runs the real
compiler without caching the result. The short summary's `bypassed` count leaves
out routine compiler probes (`compiler-query`, `standard-input`, and their `cc-`
counterparts). The full summary (`MBX_SUMMARY=full`) groups bypasses by reason;
set `MBX_BYPASS_LOG` to a file path for the per-action record.

To collect those records for one Cargo command, run it through `mbx explain`. It
groups identical causes and prints guidance for every bypass category:

```sh
mbx explain build --workspace
```

```text
cache explanation: 8 compilations bypassed the cache

compiler-query (2)
Expected: Cargo asks rustc for toolchain information; there is no compilation to cache.
  - rustc invocation is a compiler query, not a compilation (2 times)

incremental (5)
Set `MBX_INCREMENTAL=0`; mbx will then disable Cargo incremental state and cache the compilation.
  - incremental compilation cannot be combined with action caching (5 times)

standard-input (1)
Expected for Cargo probes: source supplied on standard input cannot be rediscovered later.
  - rustc invocation reads source from standard input
```

Categories marked expected are routine compiler probes with no output to
cache. Here the `incremental` group, from
[Cargo incremental mode](/incremental#cargo-incremental-mode), is the one the
build could act on. `mbx explain` preserves Cargo's exit status after printing
the explanation.

Actionable bypasses carry their remediation with the reason that produced
them, and `mbx explain` prints it in place of the category's general guidance.
For example, links rejected because of `split-debuginfo=packed` point to the
active Cargo profile or `RUSTFLAGS`, while C compilations affected by `CPATH`
name the environment variable to unset.

`mbx explain` also reports cacheability problems that prevent a compilation
from reaching mbx at all. If `CC`, `CXX`, `HOST_CC`, or `HOST_CXX` was already
set when the build began, the report names the variable and value and explains
that host C and C++ compiles are invisible to the cache. These are warnings,
not bypass counts, because mbx never observed the compiler invocations.

## Incremental

mbx compiles a crate with private incremental state, through
[learned incremental reuse](/incremental#learned-incremental-reuse) or
[eager incremental reuse](/incremental#eager-incremental-reuse). The result
never enters the shared cache. The short summary counts these compilations as
`incremental`, and the full summary says how many
`kept their own incremental state, so they were not stored`.

Whether these compilations also appear in another count depends on whether
mbx looked them up:

- A compilation that mbx looked up before switching to private state also
  counts as a miss.
- A crate that depends on a crate with private state keeps private state too,
  and counts as not looked up.
- A workspace crate you keep editing usually goes straight back to its private
  state without a lookup. The short summary counts it only as `incremental`;
  the full summary's [compiler time](#compiler-time) line and `mbx tui` list it
  as `unconsulted`.

This outcome is separate from the `incremental` bypass category shown under
[Bypass](#bypass), which reports Cargo incremental mode (`MBX_INCREMENTAL=1`).

## Remote failure

A request to the remote cache fails, and the build continues without that
result. An unreachable host, refused credentials, or an invalid response can
each cause this and reduce reuse. When a request fails during a build, mbx
falls back to compiling locally. Invalid configuration can still stop mbx at
startup, and explicit `mbx prefetch` and `mbx doctor` commands report
connection failures as errors.

The build summary counts remote failures separately, because a remote that
fails every request otherwise reports the same hits, misses, and bytes as an
empty one. The short summary includes the count inline as `remote failures`;
the full summary explains it:

```text
mbx[cache]: the remote cache failed 4 of its requests; this build ran without what it could not reach, and the warnings above say why
```

The warnings mbx prints as the build runs say what failed. The count also
appears as `remote_failures` in the
[JSON build report](#json-build-report) (`MBX_STATS_REPORT`), so CI can alert
on a cache that has quietly stopped serving.

## Interpret the hit rate {#reading-the-hit-rate}

A build can report a high hit rate among attempted lookups while spending most
of its time on compilations that were not looked up, were bypassed, or kept
private incremental state. Read all the summary counts together, and compare
wall-clock time when evaluating the cache. Set `MBX_SUMMARY=full` when the
one-line counts need a breakdown.

## Compiler time

The full summary reports real compiler time by outcome and an estimate of
the compiler time avoided by cache hits:

```text
mbx[cache]: compiler time: 4m 12s estimated avoided; 38.20s spent (161 miss in 31.00s, 7 unconsulted in 7.20s)
mbx[cache]: slowest uncached crates: syn 8.90s, regex-syntax 4.90s, serde_derive 3.90s
```

Times of a minute or more are reported in whole units, as above; shorter ones
keep their fraction.

The estimate comes from the duration recorded with the successful compilation
that populated the action prediction; older predictions without a timing hint
contribute zero. The `slowest uncached crates` line lists the five crates with
the largest cumulative uncached compiler time, so you can identify expensive
uncached work. Parallel compilations overlap, so this ranking does not directly
identify the [critical path](/analyze#critical-path).

The [JSON build report](#json-build-report) exposes the same data in
`estimated_compiler_duration_avoided_ns`, `compiler`, and
`slow_compilations`.

## JSON build report

Set `MBX_STATS_REPORT` (the `stats_report` setting) to a file path, and mbx
writes the build's counters there as JSON when the build ends:

```sh
MBX_STATS_REPORT=mbx-report.json mbx build
```

mbx writes the report even with `MBX_SUMMARY=off`. It includes the counts this
page describes, such as `hits`, `misses`, `unconsulted`,
`incremental_compilations`, `bypasses`, and `remote_failures`.
Check its `version` field before parsing it; see
[JSON output is versioned](/stability#json-output-is-versioned).

## Troubleshoot a low hit rate {#troubleshooting-a-low-hit-rate}

Run the build through `mbx explain` first, as shown under [Bypass](#bypass).

The usual causes, roughly in the order they show up:

- The store is cold. A first build has no dep-info to derive keys from, so
  `not looked up` can dominate. Compare equivalent builds with fresh targets,
  as described in [Measure cache reuse](#measure-cache-reuse).
- Cargo incremental mode is on. With `MBX_INCREMENTAL=1`, workspace members
  compile incrementally, those compilations bypass the cache, and the changed
  artifacts make crates above them miss too. See
  [Cargo incremental mode](/incremental#cargo-incremental-mode).
- A link could not be described. Native executables, tests, and proc macros
  are cached on Linux, macOS, and Windows, and self-contained WebAssembly
  targets everywhere, but native links with custom or unmodeled inputs still
  run. A rebuilt dylib can also change the keys of its downstream crates.
  `mbx explain` reports why the link bypassed. See
  [Native linking is cached only where the linker can be described](/limits#native-linking-is-cached-only-where-the-linker-can-be-described).
- The inputs differ. A different toolchain, feature set, profile, or
  `RUSTFLAGS` between two checkouts is a different key. When the target
  already holds dep-info for that compilation, mbx looks the new key up and
  reports a miss. Otherwise no recorded prediction matches the changed
  invocation, so the compilation counts as
  [not looked up](#could-not-look-up). That is the usual case in a new
  checkout, and it can look like a cold store. When a build loaded recorded
  predictions, made no lookups, and could not look up at least half as many
  compilations as they predicted, the full and CI summaries add a note that the
  compiler or its flags changed since the predictions were recorded.

  Run [`mbx analyze`](/analyze) to group either outcome by cause, such as a
  changed toolchain or changed compiler arguments. Run `mbx explain --last` to
  replay the most recent recorded build. For each missed crate, it lists the
  key details and inputs that changed since this checkout's most recent earlier
  recording of that compilation. When this checkout has none, it compares with
  the most recent recording from another checkout of the same project.
  [Session history](/tui#recording) stores hashes, not source contents or
  environment values.
- Build-script output differs. mbx can share Rust compilations that read
  `OUT_DIR` when the generated output matches across checkouts. A build script
  that embeds the checkout path in its output prevents that reuse. Sharing also
  depends on whether mbx can detect the reference and copy the output;
  `MBX_SHARE_OUT_DIR=0` disables it. See
  [Share compilations that read `OUT_DIR`](/limits#out-dir-sharing).
- A build chose its own C compiler, or is cross-compiling. Setting `CC`,
  `HOST_CC`, `CXX`, or `HOST_CXX` leaves host compilations outside mbx.
  Cross-compilations are cached when the build explicitly names a supported
  compiler through `CC_<target>`, `CXX_<target>`, `TARGET_CC`, or
  `TARGET_CXX`. Bypass kinds beginning `cc-` report anything the C adapter
  declined to model. See
  [C and C++ caching covers the host compilations mbx drives](/limits#c-and-c-caching-covers-the-host-compiles-mbx-drives).
- CI restored nothing. On GitHub Actions, check that the cache step restored
  an entry; a changed `cache-generation` or a fresh repository starts empty.
  With a remote cache configured, check the [remote failure](#remote-failure)
  count too: a remote that is failing every request reports the same zeros as
  one that is empty.

## Watch a build live {#watching-a-build-instead}

To see outcomes as mbx decides them, run [`mbx tui`](/tui) in another terminal.
It shows one row per compilation with the crate it belongs to, and reads the
builds that use the same cache directory, including ones already running.
