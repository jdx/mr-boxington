# How mbx compares

## sccache

[sccache](https://github.com/mozilla/sccache) is the established compiler
cache and predates mbx. It aims wider: it caches CUDA alongside Rust, C, and
C++, and can distribute compilation across machines. mbx caches rustc, the C
and C++ that cargo build scripts compile, the C and C++ of builds outside
cargo through [`mbx exec`](/standalone-builds), and native links. mbx spends
the narrower scope on what it costs to operate, what it does to your disk, and
what it is safe to let CI write.

- Nothing to restart after a settings change. sccache builds talk to a
  background server that outlives the build and keeps the configuration it
  was started with, and a build that cannot reach it fails unless you have
  opted into falling back. mbx starts an in-process agent for each command
  and exits with it, so a changed setting applies to the next build.
- `target/` stops growing. sccache caches compilations, but each checkout's
  `target/` is still yours to hold and yours to clean. mbx stores outputs once
  in a content-addressed store, reflinks them into each checkout's `target/`,
  and collects the directories whose checkout is gone, so a dozen worktrees
  cost roughly what one does.
- Several Cargo builds fit on one machine. sccache hands a GNU make jobserver
  down to the compilers it spawns, which keeps one build from oversubscribing
  the machine. mbx budgets across builds: Cargo commands started
  independently, such as clippy beside tests, draw their compilers from a
  single machine-wide CPU and memory pool, and a compilation identical to one
  already running anywhere on the machine waits for that one instead of
  repeating it. See [machine-wide compile
  scheduling](/configuration#machine-wide-compile-scheduling).
- A new worktree is warm without configuration. sccache matches on absolute
  paths until you set `SCCACHE_BASEDIRS` to the directories to strip, and a
  path you forget to list is a cache miss you never hear about. mbx derives
  the placeholders itself: workspace, target, registry, toolchain, and
  sysroot.
- A disappointing build points at its own cause. Every build reports hits,
  misses, lookups it could not attempt, and bypasses, instead of moving a
  global counter.
- CI can be given a cache it cannot damage. sccache's backends are buckets and
  services reached with a credential; what holds that credential can write,
  and usually delete. mbx's remote is a [cache server](/cache-server) with
  deny-by-default grants, separate read and write namespace patterns,
  immutable blobs, and no deletion endpoint. A job that should never publish
  cannot, and one that may publish cannot rewrite or remove what an earlier
  build wrote.

If you need CUDA or distributed compilation, sccache is the right tool. Both
wrap rustc through `RUSTC_WRAPPER`, so they cannot be combined for the same
build.

## kache

[kache](https://github.com/kunobi-ninja/kache) is the closest tool to mbx: it
predates mbx and directly inspired its design, though the projects share no
code. mbx began as the Rust cache inside the
[mise](https://github.com/jdx/mise) task runner and was extracted into its
own CLI for three things: less to operate, a better day-to-day experience,
and tight limits on what CI can write to a shared cache, which matters most
in a public repository that takes fork pull requests. The differences below
follow from those three goals.

Like mbx, kache is a content-addressed `RUSTC_WRAPPER` cache built for sharing
compilations across worktrees, with C and C++ compiler shims, S3-compatible
and filesystem remotes, and executable caching on Linux, macOS, and Windows.

- Nothing to install or keep running. `kache init` installs an OS service by
  default, with a `--no-service` opt-out. mbx starts an in-process agent with
  each command and exits with it: nothing to bring back after a reboot,
  nothing to restart, and no state on the machine that outlives the build.
- Nobody has to remember to prune. kache hardlinks outputs into place, so a
  checkout's `target/` shares disk with the store but stays where it is until
  someone runs `kache gc`. mbx owns the directories it creates: outputs live
  once in the store, appear in each checkout by reflink, and a `target/`
  whose checkout is gone is collected without being asked. Reflinks are
  copy-on-write clones that diverge the moment something writes to them, so a
  build that scribbles into `target/` cannot damage the cache the way a
  shared inode can. Where the filesystem cannot clone, mbx copies the bytes.
- Several Cargo builds fit on one machine. Cargo plans one build at a time,
  so two started together each size themselves to the whole machine and both
  finish late. kache does not stop you running them, but it does not
  coordinate them either: it starts each compiler as Cargo asks for it, with
  no shared budget and nothing that notices two builds compiling the same
  crate at the same moment. mbx's shims sit in front of every compiler on the
  machine and hand out permits from one memory-aware pool, so a lint job and
  a test job run side by side without overloading the box, and a compilation
  identical to one already running anywhere waits for it. Either way, give
  each command its own `CARGO_TARGET_DIR`: Cargo's target directory lock
  serializes them otherwise. See [machine-wide compile
  scheduling](/configuration#machine-wide-compile-scheduling).
- A public repository can share one cache. This is the difference that shaped
  mbx most. kache's remotes are S3-compatible buckets or a filesystem path,
  and a bucket has one question to ask: does this credential write? Whatever
  can publish can also overwrite or remove what an earlier build published,
  and one leaked credential is the whole cache. mbx's remote is a protocol
  server, which can express finer rules: grants are deny-by-default with
  separate read and write namespace patterns; CI authenticates with a
  short-lived OIDC token pinned to a repository and its immutable numeric
  owner ID, narrowable to a `ref` or `environment`, instead of a stored
  secret; blobs are immutable and results commit atomically; and there is no
  deletion endpoint, so a writer adds results and can never rewrite or remove
  one. Fork pull requests hold no credentials at all and fall back to a
  read-only platform cache; see [fork PRs](/cookbook/fork-prs). mbx's client
  also refuses to publish from pull requests, unprotected branches, and tag
  builds, which catches a misconfigured grant early; the server is what makes
  the guarantee.
- A build is cached only when it asks to be. Both tools put C and C++ shims on
  `PATH` so make, CMake, or anything else that resolves its compiler there
  gets cached. kache installs them alongside its service, so every build on
  the machine goes through the cache. mbx puts them on `PATH` for a single
  [`mbx exec`](/standalone-builds) command and leaves other builds untouched.
  Both cover Windows; mbx intercepts `cl.exe` there and the plain gcc/clang
  driver names on Unix.
- A restored binary matches the machine it runs on. Both tools cache linked
  binaries on Linux, macOS, and Windows. mbx keys on the resolved linker,
  startup objects, libc, and SDK as well as dep-info, because a binary linked
  against a different libc or SDK is a different binary. On a host mbx cannot
  describe that precisely, it links normally.

If you want the C and C++ shims installed once instead of wrapping the
commands that should use them, kache is worth evaluating. Both tools wrap
rustc through `RUSTC_WRAPPER`, so they cannot be combined for the same build.

## Tarball CI caches

Actions such as `actions/cache` over `target/` (or `Swatinem/rust-cache`)
save and restore the whole directory as one archive. That is simple and needs
no extra tooling, but the archive is all-or-nothing: one changed crate still
uploads and downloads everything, the entry grows until it hits the platform's
size cap, and stale artifacts accumulate inside it.

On GitHub-hosted runners,
[`jdx/mr-boxington-action`](https://github.com/jdx/mr-boxington-action)
restores the same pruned target tree those actions do, so its restore and
build times match rust-cache on Linux and macOS, and it then runs the build
through mbx: the
compile scheduler, the toolchain guard, and per-action reuse inside the job.
The per-action store is what the [cache server](/cache-server) transports,
where one entry can serve every job and branch instead of one archive per
job. See [GitHub Actions](/github-action).

## Cargo's incremental compilation

Incremental compilation speeds up recompiling the crate you are editing inside
one checkout. It does nothing across checkouts, worktrees, or CI runners, and
its artifacts are checkout-specific by design. mbx caches the dependency
graph everywhere and leaves the inner loop to rustc. The two interact: see
[incremental builds](/configuration#incremental-builds).
