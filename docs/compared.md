# How mbx compares

## sccache

[sccache](https://github.com/mozilla/sccache) is the established compiler
cache, and it aims wider: it caches C, C++, and CUDA alongside Rust and can
distribute compilation across machines. mbx caches only what a cargo build
runs — rustc, the C and C++ its build scripts compile, and optionally its
native links — and spends that narrower scope on problems sccache does not
attempt:

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

If you need C/C++ caching outside cargo builds, CUDA, or distributed
compilation, sccache is the right tool. Both wrap rustc through
`RUSTC_WRAPPER`, so they cannot be combined for the same build.

## kache

[kache](https://github.com/kunobi-ninja/kache) is the closest tool to mbx,
and the comparison comes with a debt: parts of mbx's design were inspired by
kache. No code is shared between the projects, but the influence is real and
worth acknowledging. Like mbx, it is a content-addressed `RUSTC_WRAPPER`
cache built for sharing compilations across worktrees, with C/C++ compiler
shims, S3-compatible remotes, and executable caching on Linux and macOS. It
also publishes a scheduled benchmark workflow that builds Firefox, LLVM, and
other large projects cold and warm — a level of public verification mbx does
not offer today. The differences are in the mechanics:

- **No daemon.** `kache init` installs an OS service by default (there is a
  `--no-service` opt-out). mbx starts an in-process agent for each command
  and exits with it; there is nothing to install, restart, or leave running.
- **Managed `target/` vs. deduplicated `target/`.** kache hardlinks outputs
  into place, so each checkout's `target/` shares disk with the store but
  stays where it is, pruned by `kache gc`. mbx owns the directories it
  creates: outputs live once in the store, appear in each checkout by
  reflink, and directories whose checkout is gone are collected
  automatically.
- **Restores never hardlink.** A hardlinked output shares an inode with the
  cache — the two names are the same file. mbx restores by reflink, a
  copy-on-write clone that diverges on first write, and falls back to a
  plain byte copy where the filesystem cannot clone, never to a hardlink.
- **CI write policy and remote auth.** kache's remotes are S3-compatible
  buckets reached through the standard AWS credential chain, and its README
  states no policy on what may publish from pull requests — whatever the
  credentials allow, any build can write. mbx's client refuses to publish
  from pull requests and unprotected branches and disables caching entirely
  on tag builds, and its remote is a namespaced protocol server with
  deny-by-default token and OIDC grants rather than a bucket the build
  writes to directly.
- **C/C++ scope.** kache's shims sit on `PATH`, so C/C++ built outside cargo
  (CMake, for example) is in scope. mbx caches the C and C++ that cargo
  build scripts compile through the `cc` crate — it follows the cargo build
  rather than the compiler, so a standalone C project is out of scope.
- **Executable caching.** Both cache linked binaries on Linux and macOS. In
  mbx it is opt-in (`MBX_CACHE_LINKS=1`) and experimental, and the key
  includes the resolved linker, startup objects, libc, and SDK rather than
  dep-info alone.

If your repository mixes cargo with substantial C/C++ under other build
systems, or you want published benchmark numbers to check claims against,
kache is worth evaluating. Both tools wrap rustc through `RUSTC_WRAPPER`, so
they cannot be combined for the same build.

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
