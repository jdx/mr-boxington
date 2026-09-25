---
description: Compare mbx with sccache, kache, archive-based CI caching, and Cargo incremental compilation.
---
# How mbx compares

mbx wraps Cargo as well as compiler invocations. kache and sccache integrate
at the compiler layer: Cargo calls them through `RUSTC_WRAPPER`. All three can
reuse compiled work, but mbx also manages the surrounding Cargo workflow,
including target directories, build configuration, and command-level output.
Cargo still resolves dependencies and decides what needs building.

| Your main need | Start by evaluating |
| --- | --- |
| Compiler caching integrated with Cargo setup and target cleanup | mbx |
| A compiler cache configured underneath your existing Cargo commands | [kache](#kache) |
| A broader compiler set or distributed compilation | [sccache](#sccache) |
| Restore Cargo state between GitHub Actions jobs | [Archive-based caching](#tarball-ci-caches) |
| Recompile the crate you are editing within one checkout | [Incremental compilation](#cargos-incremental-compilation) |

For measured performance, use [Benchmarks](/benchmarks). Feature comparisons
do not predict speed on a particular project; check the versions and workloads
behind each result.

## What wrapping Cargo adds

With mbx, `mbx build` starts Cargo and configures the compiler wrappers for that
command. With kache, `kache init` configures Cargo's `rustc-wrapper`, then you
continue running `cargo build`.

```text
mbx:    mbx build → Cargo → mbx compiler wrapper → rustc
kache:  cargo build → kache compiler wrapper → rustc
```

This is an integration difference, not a requirement to change how you type
commands: [mbx setup and mise integration](/setup) can route plain `cargo`
commands through mbx too.

Starting at the Cargo command lets mbx bring these pieces together:

- **Target management.** [Managed targets](/managed-targets) place checkout
  outputs under a shared cache root and collect them when checkouts disappear
  or exceed disk and age budgets. Explicit target-directory settings remain
  under your control.
- **Build policy.** mbx configures wrappers and incremental behavior for each
  command. [Incremental reuse](/incremental) keeps private state for edited
  crates while unchanged work stays eligible for sharing.
- **A complete build session.** An agent lives for the command, with
  [Cargo progress and cache results](/cache-results) reported together.
- **Resource coordination.** [Parallel builds](/scheduling) share a CPU and
  memory budget across independent Cargo processes using the same cache root.
  This complements target management; compiler-level coordination is also
  available in kache.

## kache

[kache](https://github.com/kunobi-ninja/kache) directly inspired mbx. Both share
content-addressed compiler results across checkouts, and the projects do not
share code. The main distinction is where they integrate: kache's
[compiler wrapper](https://ninja.kunobi.com/docs/kache/how-it-works/architecture#interception-boundary)
does not wrap Cargo itself; mbx adds the Cargo-level behavior described above.

There is substantial overlap beyond caching Rust libraries. kache supports
Rust executables on Linux and macOS, C/C++, CUDA, S3-compatible remotes, and
filesystem remotes. It provides per-build reports, a live monitor, and
`why-miss` diagnostics. Cross-checkout reuse, remote storage, and reporting
are shared capabilities, rather than reasons on their own to choose mbx.

| Area | mbx | kache |
| --- | --- | --- |
| Process lifecycle | Agent starts and stops with each command | Optional persistent daemon handles remote work and maintenance; local caching works without it |
| Local restores | Prefers copy-on-write clones, then read-only hard links, then copies | Prefers copy-on-write clones; restricted hard links for immutable artifacts on Unix, copies otherwise |
| Concurrent compilations | Shared CPU and memory budget; deduplicates identical work in flight | Memory-weighted permits and machine-wide deduplication of identical work |
| Remote sharing | [S3-compatible buckets](/remote-cache) or an authenticated [cache server](/cache-server) | S3-compatible buckets or filesystem remotes |

These details follow kache's current
[architecture](https://ninja.kunobi.com/docs/kache/how-it-works/architecture) and
[daemon lifecycle](https://ninja.kunobi.com/docs/kache/daemon/lifecycle)
documentation, checked September 25, 2026. Its daemon restarts when its config
file changes; environment changes require a daemon started with the new
environment.

Evaluate kache when you want caching beneath Cargo or other supported build
systems. Evaluate mbx when you also want it to own Cargo setup, target cleanup,
and the build session. Compare both with the same source, toolchain, targets,
and filesystem; the [benchmark method](/benchmarks#keeping-it-fair) explains
how this project controls those variables.

## sccache

[sccache](https://github.com/mozilla/sccache) is an established compiler cache
with a broader compiler scope, including CUDA, and support for distributed
compilation. It can store cached results locally or in remote storage.
Distributed compilation runs compiler work on other machines; sharing a remote
cache reuses results that have already been built. Choose based on which of
those you need.

Like kache, sccache integrates with Cargo through `RUSTC_WRAPPER`. mbx adds the
Cargo workflow management described above. Both sccache and kache can be used
with build systems outside Cargo for the compilers they support.

Use one Rust compiler cache for a given invocation. mbx defers to an existing
`RUSTC_WRAPPER`, so leaving sccache or kache configured prevents mbx from caching
that Rust compilation. Follow the
[migration guide](/cookbook/migrate#from-sccache) when switching.

## Tarball CI caches

[`actions/cache`](https://github.com/actions/cache) saves selected paths as an
archive. [`Swatinem/rust-cache`](https://github.com/Swatinem/rust-cache) adds
Rust-specific keys and pruning. They preserve Cargo's target state so a job
can reuse outputs without asking a compiler cache for every artifact.

The default [`jdx/mr-boxington-action`](https://github.com/jdx/mr-boxington-action)
backend also transports a pruned Cargo target and registry archive. The build
then runs through mbx, gaining its compiler scheduling and per-action reuse
within the job. This is an archive transport, so it still pays archive restore
and save costs.

The action's `objects` mode transports mbx's recorded action closure instead.
A configured cache server or S3 bucket transfers individual objects, letting
builds fetch the work they need across different target layouts. Choose one
transport for the same cached data; stacking archive actions adds duplicate
work. See [GitHub Action](/github-action) for the tradeoffs and examples.

<span id="cargo-s-incremental-compilation"></span>

## Cargo's incremental compilation {#cargos-incremental-compilation}

Incremental compilation reuses parts of a crate between edits in one checkout.
A shared compiler cache reuses complete matching results across builds.

mbx uses both: it shares eligible compilations and automatically keeps private
incremental state for crates whose sources you are changing. That private state
is never published to the shared cache. Setting `MBX_INCREMENTAL=1` gives Cargo
control of workspace incremental compilation and can reduce cross-checkout
reuse. See [Incremental builds](/incremental) before changing the default.
