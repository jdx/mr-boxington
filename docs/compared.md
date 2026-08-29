# How mbx compares

::: info Standing on earlier work
mbx runs on Cargo, follows the compiler-caching trail established by sccache,
and is deeply inspired by kache, the project that most directly shaped it. See
the [acknowledgements](/acknowledgements) for a fuller account of those debts.
:::

## sccache

[sccache](https://github.com/mozilla/sccache) is the established compiler
cache, and it aims wider: it caches CUDA alongside Rust, C, and C++, and can
distribute compilation across machines. mbx caches rustc, the C and C++ that
cargo build scripts compile, the C and C++ of builds outside cargo through
[`mbx exec`](/standalone-builds), and native links. That scope is narrower,
and mbx spends the difference on the parts of caching that are not the cache:
what it costs to operate, what it does to your disk, and what it is safe to
let CI write.

- **No daemon.** sccache builds talk to a background server. It starts
  itself, but it outlives the build, keeps the configuration it was started
  with — so changing a setting means remembering to restart it — and a build
  that cannot reach it fails unless you have opted into falling back. mbx
  starts an in-process agent for each command and exits with it: nothing
  outlives the build, so there is no stale configuration and nothing to
  restart.
- **`target/` stops growing.** sccache caches compilations, but each
  checkout's `target/` is still yours to hold and yours to clean. mbx stores
  outputs once in a content-addressed store, reflinks them into each
  checkout's `target/`, and collects the directories whose checkout is gone,
  so a dozen worktrees cost roughly what one does.
- **Several Cargo builds at once.** sccache hands a GNU make jobserver down
  to the compilers it spawns, which keeps one build from oversubscribing the
  machine. mbx budgets across builds rather than inside one: Cargo commands
  started independently — clippy beside tests, or two lint configurations —
  draw their compilers from a single machine-wide CPU and memory pool, and a
  compilation identical to one already running anywhere on the machine waits
  for that one instead of burning a core on it. See [machine-wide compile
  scheduling](/configuration#machine-wide-compile-scheduling).
- **Sharing between checkouts is the default, not a setting.** sccache
  matches on absolute paths until you set `SCCACHE_BASEDIRS` to the
  directories to strip, and a path you forget to list is a cache miss you
  never hear about. mbx derives the placeholders itself — workspace, target,
  registry, toolchain, and sysroot — so a new worktree is warm without being
  told where anything lives.
- **A hit rate you can act on.** Every build reports hits, misses, lookups it
  could not attempt, and deliberate bypasses, so a disappointing build points
  at its own cause instead of moving a global counter.
- **A remote that limits what CI can write.** sccache's backends are buckets and
  services reached with a credential; what holds that credential can write,
  and usually delete. mbx's remote is a [cache server](/cache-server) with
  deny-by-default grants, separate read and write namespace patterns,
  immutable blobs, and no deletion endpoint, so a job that should never
  publish cannot, and one that may publish still cannot rewrite or remove
  what an earlier build wrote.

If you need CUDA, distributed compilation, or C and C++ on Windows and MSVC,
sccache is the right tool. Both wrap rustc through `RUSTC_WRAPPER`, so they
cannot be combined for the same build.

## kache

[kache](https://github.com/kunobi-ninja/kache) is the closest tool to mbx and
the project that most directly inspired it. No code is shared between the
projects, but the influence is substantial and worth saying plainly. mbx began
as the Rust cache inside the [mise](https://github.com/jdx/mise) task runner
and was extracted into its own CLI to chase three things: less to operate, a
better day-to-day experience, and something safe to switch on in a public
repository that takes fork pull requests. The differences below are mostly
those three goals.

Like mbx, kache is a content-addressed `RUSTC_WRAPPER` cache built for sharing
compilations across worktrees, with C and C++ compiler shims, S3-compatible
and filesystem remotes, and executable caching on Linux and macOS. kache has
also been around longer and has more real-world history behind it. It is the
more proven choice today; if maturity is the deciding factor, that is its most
important advantage over mbx.

- **Nothing to install or keep running.** `kache init` installs an OS service
  by default, with a `--no-service` opt-out. mbx starts an in-process agent
  with each command and exits with it: nothing to bring back after a reboot,
  nothing to restart when it misbehaves, and no state on the machine that
  outlives the build.
- **Nobody has to remember to prune.** kache hardlinks outputs into place, so
  a checkout's `target/` shares disk with the store but stays where it is,
  cleaned up by `kache gc` when someone runs it. mbx owns the directories it
  creates: outputs live once in the store, appear in each checkout by
  reflink, and a `target/` whose checkout is gone is collected without being
  asked. Reflinks are copy-on-write clones that diverge the moment something
  writes to them, so a build that scribbles into `target/` cannot damage the
  cache the way a shared inode can. Where the filesystem cannot clone, mbx
  copies the bytes — still never a hardlink.
- **Several Cargo builds at once.** Cargo plans one build at a time, so two
  started together each size themselves to the whole machine and both finish
  late. kache does not stop you running them, but it does not coordinate
  them either: it starts each compiler as Cargo asks for it, with no shared
  budget and nothing that notices two builds compiling the same crate at the
  same moment. mbx's shims sit in front of every compiler on the machine and
  hand out permits from one memory-aware pool, so a lint job and a test job
  run side by side without overloading the box, and a compilation identical
  to one already running anywhere waits for it instead of repeating it.
  Either way, give each command its own `CARGO_TARGET_DIR`: Cargo's target
  directory lock is what serializes them otherwise. See [machine-wide
  compile scheduling](/configuration#machine-wide-compile-scheduling).
- **A public repository can share one cache.** This is the difference that
  shaped mbx most. kache's remotes are S3-compatible buckets or a filesystem
  path, and a bucket has one question to ask: does this credential write? So
  whatever can publish can also overwrite or remove what an earlier build
  published, and one leaked credential is the whole cache. mbx's remote is a
  protocol server, which puts those rules somewhere they can be expressed:
  grants are deny-by-default with separate read and write namespace patterns;
  CI authenticates with a short-lived OIDC token pinned to a repository and
  its immutable numeric owner ID, narrowable to a `ref` or `environment`,
  instead of a stored secret; blobs are immutable and results commit
  atomically; and there is no deletion endpoint, so a writer adds results and
  can never rewrite or remove one. Fork pull requests hold no credentials at
  all and fall back to a read-only platform cache — see [fork
  PRs](/cookbook/fork-prs). mbx's client refuses to publish from pull
  requests, unprotected branches, and tag builds too, but that is a
  convenience that catches a misconfigured grant early; the server is what
  makes the guarantee.
- **Caching C and C++ is opt-in per command.** Both put shims on `PATH` so
  make, CMake, or anything else that resolves its compiler there gets cached.
  kache installs them alongside its service, so once they are on `PATH` every
  build on the machine goes through the cache; mbx puts them on `PATH` for a
  single [`mbx exec`](/standalone-builds) command, so a build is cached when
  it asks to be and untouched otherwise. kache covers Windows;
  mbx shims only the plain Unix driver names.
- **Linked binaries.** Both cache them on Linux and macOS. mbx keys on the
  resolved linker, startup objects, libc, and SDK rather than dep-info alone,
  because a binary linked against a different libc or SDK is a different
  binary. On a host mbx cannot describe that precisely, it links normally
  rather than handing back one it cannot vouch for.

If you value a longer track record, build on Windows, or want the C and C++
shims installed once rather than wrapping the commands that should use them,
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
[GitHub Actions](/github-action).

## Cargo's incremental compilation

Incremental compilation speeds up recompiling the crate you are editing inside
one checkout. It does nothing across checkouts, worktrees, or CI runners, and
its artifacts are checkout-specific by design. mbx solves the complementary
problem — the dependency graph, everywhere — and leaves the inner loop to
rustc. The two interact: see
[incremental builds](/configuration#incremental-builds).
