---
description: Compare mbx with sccache, kache, archive-based CI caching, and Cargo incremental compilation.
---
# How mbx compares

Choose a cache around the work you want to reuse: compiler invocations,
whole CI directories, or edits within one checkout. mbx focuses on Cargo
workflows, with shared compiler results, managed targets, and a scheduler for
simultaneous builds.

| Your main need | Start by evaluating |
| --- | --- |
| Cargo worktrees, bounded local storage, and concurrent builds | mbx |
| A broader compiler set or distributed compilation | [sccache](#sccache) |
| A persistent cache service and hardlinked local outputs | [kache](#kache) |
| Restore Cargo state between GitHub Actions jobs | [Archive-based caching](#tarball-ci-caches) |
| Recompile the crate you are editing within one checkout | [Incremental compilation](#cargos-incremental-compilation) |

For measured performance, use [Benchmarks](/benchmarks). Feature comparisons
do not predict speed on a particular project.

## sccache

[sccache](https://github.com/mozilla/sccache) is an established compiler cache
with a broader compiler scope, including CUDA, and support for distributed
compilation. It can store cached results locally or in remote storage.

mbx's Cargo integration combines several responsibilities:

- An agent starts and stops with each command, so configuration changes apply
  to the next build without restarting a persistent service.
- Portable keys map workspace, target, registry, toolchain, and sysroot roots
  automatically. Matching work can be restored across checkouts.
- [Managed targets](/managed-targets) add cleanup policies for checkout outputs
  as well as a budget for cached objects.
- [Parallel builds](/scheduling) share compiler permits across independent
  Cargo processes using the same cache root.
- [Build reports](/cache-results) separate misses, unavailable lookups, and
  intentional bypasses for each command.

sccache and mbx both use `RUSTC_WRAPPER`. They cannot cache the same Rust
invocation together; mbx defers to an existing wrapper. Follow the
[migration guide](/cookbook/migrate#from-sccache) when switching.

## kache

[kache](https://github.com/kunobi-ninja/kache) directly inspired mbx. Both use
content-addressed compiler results to share work across checkouts and support
Rust plus C and C++ workflows. The projects do not share code.

kache uses hardlinks for local output sharing and offers a persistent service.
mbx starts an agent per command and tries to reflink outputs into place.
Reflinks share data blocks until a file is modified; writes to a restored
output do not modify the cache object. On filesystems without clone support,
mbx copies the bytes, so the disk savings depend on the filesystem.

mbx also manages the lifetime of target directories it creates and coordinates
simultaneous builds through a shared compiler pool. These features address
worktree cleanup and contention as well as compilation reuse. Explicit
`CARGO_TARGET_DIR` values remain under your control.

For remote sharing, mbx supports both [S3-compatible buckets](/remote-cache)
and a [protocol server](/cache-server). The server can enforce separate read
and write grants by namespace. Bucket deployments need equivalent restrictions
in their storage permissions. Client-side write policy is an additional guard,
not an authorization boundary.

Evaluate both with the same source, toolchain, targets, and filesystem. The
[published benchmark method](/benchmarks#keeping-it-fair) describes how this
project keeps comparisons consistent.

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
