# How mbx compares

## sccache

[sccache](https://github.com/mozilla/sccache) is the established compiler
cache, and it aims wider: it caches C, C++, and CUDA alongside Rust and can
distribute compilation across machines. mbx only caches rustc, and spends that
narrower scope on problems sccache does not attempt:

- **No daemon.** sccache runs a background server that builds talk to. mbx
  starts an in-process agent for each command and exits with it; there is
  nothing to start, restart, or leave running.
- **Managed `target/` directories.** mbx stores outputs once in a
  content-addressed store, reflinks them into each checkout's `target/`, and
  collects directories whose checkout is gone. sccache caches compilations but
  leaves every `target/` to grow on its own.
- **Keys built for worktrees.** Workspace, registry, toolchain, and sysroot
  paths are mapped to placeholders before they enter a key, so equivalent
  checkouts share compilations even when their absolute paths differ.
- **Per-build accounting.** Every build reports hits, misses, lookups it could
  not attempt, and deliberate bypasses, so a low hit rate has a visible cause
  rather than a global counter.
- **CI write policy.** The client itself refuses to publish remote objects
  from pull requests, unprotected branches, and tag builds, on top of whatever
  the server enforces.

If you need C/C++ caching or distributed compilation, sccache is the right
tool. Both wrap rustc through `RUSTC_WRAPPER`, so they cannot be combined for
the same build.

## Tarball CI caches

Actions such as `actions/cache` over `target/` (or `Swatinem/rust-cache`)
save and restore the whole directory as one archive. That is simple and needs
no extra tooling, but the archive is all-or-nothing: one changed crate still
uploads and downloads everything, the entry grows until it hits the platform's
size cap, and stale artifacts accumulate inside it.

mbx restores exactly the actions a build needs from a store it prunes itself.
[`jdx/mr-boxington-action`](https://github.com/jdx/mr-boxington-action) uses
GitHub Actions cache as the transport for that store, so it composes the
per-action granularity with the platform cache you already have. See
[GitHub Actions](/github-actions).

## Cargo's incremental compilation

Incremental compilation speeds up recompiling the crate you are editing inside
one checkout. It does nothing across checkouts, worktrees, or CI runners, and
its artifacts are checkout-specific by design. mbx solves the complementary
problem — the dependency graph, everywhere — and leaves the inner loop to
rustc. The two interact: see
[incremental builds](/configuration#incremental-builds).
