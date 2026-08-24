# mbx v1 plan

mbx ("Mr Boxington") is a standalone build cache for Rust projects. It wraps
cargo, caches individual rustc compilations in a content-addressed store shared
across every project and git worktree on a machine, and optionally shares those
compilations with CI and teammates through a remote cache server.

It is extracted from mise's experimental Rust action cache. The protocol,
action model, and rustc analysis were designed and proven there; mbx gives them
a standalone home. mise will eventually consume mbx and drop its embedded copy.

## Problems mbx solves

1. **Cold builds repeat work.** Every project and worktree compiles the same
   dependency graph from scratch. mbx caches each rustc action once and
   restores it everywhere else, locally and from a remote cache.
2. **Target directories fill disks.** Cargo never garbage-collects and never
   deduplicates across checkouts. mbx stores artifacts once in a
   content-addressed store, materializes them into target dirs via
   reflink/hardlink, and evicts with a byte-budget LRU that a build runs on its
   own schedule (`mbx gc`, or automatically). Eviction prefers what no checkout
   on disk still needs, so deleting a worktree releases what only it used, and
   mbx can place the target directories themselves so the outputs of a checkout
   that is gone are collected rather than stranded.
3. **Git worktrees build cold.** Fingerprints embed absolute paths, so a fresh
   worktree rebuilds everything. mbx's action keys use path mapping, so a new
   worktree starts mostly warm while keeping its own target dir (no cargo lock
   contention, no incremental-artifact thrashing). Mostly, not entirely: see
   the note on `OUT_DIR` under path mapping.
4. **CI caching is coarse.** Tarball-the-target-dir caching is slow,
   size-capped, and all-or-nothing. mbx restores exactly the actions a build
   needs, in batched blob packs, from an ephemeral runner's empty store.

## Non-goals

- Replacing cargo. Cargo keeps resolution, feature unification, build
  planning, and linking. mbx is a `RUSTC_WRAPPER` plus an orchestrating
  wrapper command.
- Caching linking or non-rlib/metadata artifacts (future work).
- Competing with rustc incremental for workspace members. Cargo builds
  dependencies non-incrementally by default; mbx caches those. Incremental
  compilations bypass the cache by design, and `MBX_INCREMENTAL=1` opts into
  that trade rather than trying to cache them.
- A daemon. The agent lives for the duration of one cached Cargo command.

## Architecture

```text
mbx <cargo subcommand> [cargo args]
  └─ session (tempdir)
     ├─ shim install: hardlink/copy of the mbx binary named `mbx-rustc`
     ├─ agent: in-process, serves a unix socket / windows named pipe
     │    ├─ local CAS + action cache (~/.cache/mbx)
     │    └─ optional remote cache client (blob packs, OIDC/token auth)
     ├─ env injection: RUSTC_WRAPPER=<session>/mbx-rustc, MBX_SOCKET,
     │    MBX_STAGING_DIR, MBX_BUILD, MBX_WORKSPACE_ROOT, MBX_TARGET_DIR,
     │    CARGO_INCREMENTAL=0, MBX_PREVIOUS_RUSTC_WRAPPER chaining
     ├─ run cargo
     └─ finish: flush staged uploads, print stats, optional JSON report
```

Crates:

- `mbx-cache-core` — action-cache protocol, canonical JSON digests, local CAS,
  remote HTTP client (blob packs, retries, OIDC/token), cache agent.
  Source of truth for all protocol constants.
- `mbx-cache-rustc` — conservative rustc invocation analysis: argv parsing
  with an explicit allowlist, dep-info-driven input discovery, action key
  construction, path mapping. Anything not modeled bypasses the cache.
- `mbx` — the CLI: session/agent orchestration, the rustc shim, config,
  store management commands.

Server: [`jdx/mbx-cache`](https://github.com/jdx/mbx-cache) — self-hostable
remote cache (filesystem/S3 storage, memory/Postgres metadata, static and OIDC
grants, namespace isolation). It copies protocol constants from
`mbx-cache-core`; a shared protocol crate is future work.

### The shim

Cargo invokes `RUSTC_WRAPPER` hundreds of times per build, so the shim path is
dispatched on argv0 as the first statement of `main`, before any runtime,
argument parsing, or config loading. The shim is a hardlink (fallback copy) of
the running mbx binary named `mbx-rustc`; because shim and agent are always
the same binary, the agent handshake can require strict version equality.

Two modes:

- **Session mode** (under an mbx-wrapped Cargo command): talks to the agent over the socket;
  gets prefetch, batched uploads, and staged blob packs. This is the only mode
  today.
- **Standalone mode** (no `MBX_SOCKET`): would read and write the local store
  directly, making `build.rustc-wrapper` useful for plain `cargo build`. Blocked
  on cross-process prefetch manifests — see future work.

On any unsupported invocation, error, or bypass condition the shim execs the
real rustc transparently. Correctness beats hit rate everywhere.

### Path mapping

Cache keys hold no absolute paths. The session passes the workspace root and the
target directory to the shim, which maps them to `${workspace}` and `${target}`
placeholders along with `${cargo_home}`, `${rustup_home}`, and `${home}`.

Both roots have to be passed in rather than inferred: cargo compiles a
dependency with its working directory inside the registry, so the shim sees no
sign of the workspace whose target directory it is writing to. Mapping the
target directory first, ahead of the workspace that usually contains it, also
keeps keys stable when the target directory moves elsewhere.

A path that matches no root bypasses the cache rather than entering a key. The
roots come from `cargo metadata`, since a target directory can be set by flag,
environment, or cargo configuration, and `--manifest-path` can move the build
elsewhere entirely.

Environment *values* are a deliberate exception: they enter the key verbatim,
unmapped. The one that matters is `OUT_DIR`, which every crate that includes
build-script output consumes, and which differs per checkout. Mapping it lifts
cross-checkout sharing — measured at 65% of actions on mise — but a crate may
bake that path into its artifact, and no *input* distinguishes one that does
from one that does not.

`MBX_SHARE_OUT_DIR` (off by default) resolves that by not relying on the inputs
alone. It makes the compilation independent of the value with
`--remap-path-prefix`, so rustc records the placeholder wherever it records a
path itself, and then reads the outputs before publishing: a compilation whose
artifact still carries the value keeps its own checkout's key. Both halves are
load-bearing. Without the remap the outputs always carry the path, in rmeta and
in debug info, and nothing shares; without reading the outputs, a crate that
keeps `env!("OUT_DIR")` in a string constant would be shared wrongly.

What that does not cover is a value a crate derives from `OUT_DIR` instead of
embedding — its length, or a hash of it. That is why the default stays off, and
why flipping it is gated on verify-mode qualification over a real workspace
rather than on the measurement alone.

### Cache identity

mise keyed its session manifests by task definition. mbx defines its own
identity (v1): a canonical JSON of `{version, workspace, command}`, where
`workspace` is the digest of `Cargo.lock` rather than the checkout path, so
separate worktrees of one dependency graph share a manifest. A project with no
lockfile falls back to its directory name.

The identity only namespaces prefetch manifests — action keys come from
`mbx-cache-rustc` and are independent of it. A wrong identity costs a cold
prefetch, never a wrong result, so bumping the version invalidates manifests
and not cached actions.

## Protocol

Same protocol as mise action-cache v1, renamed. The wire rename table:

| mise name | mbx name |
| --- | --- |
| `application/vnd.mise.cache-*` media types | `application/vnd.mbx.cache-*` |
| `mise-cache-protocol` header | `mbx-cache-protocol` |
| `mise-cache-namespace` header | `mbx-cache-namespace` |
| `MISEPK01` blob-pack magic | `MBXPACK1` |
| `MISE_CACHE_TASK` env var | `MBX_BUILD` |
| `MISE_CACHE_*` env vars | `MBX_*` (client), `MBX_CACHE_*` (server) |
| `mise-cache-rustc` shim stem | `mbx-rustc` |

Protocol version stays 1; nothing in the wild speaks the old names.

## Command surface (v1)

- `mbx <cargo subcommand> [cargo args]` — run Cargo under a cache session.
- `mbx gc [--max-size <bytes|human>]` — LRU-evict the store to a byte budget,
  defaulting to the configured one. A cached Cargo command does the same when a sweep is
  due.
- `mbx cache dir` / `mbx cache stats` — store location and contents summary,
  including the managed target directories.
- `mbx setup` — install the persistent standalone shim and wire
  `build.rustc-wrapper` in `~/.cargo/config.toml`. If that key already names
  another wrapper, such as sccache, setup reports it and changes nothing:
  silently displacing a wrapper the user chose is worse than doing nothing.
  Depends on standalone shim mode, so it lands with it.
- `mbx-rustc` (argv0) — the shim; not invoked by humans.

## Configuration

Env vars first, optional `~/.config/mbx/config.toml` second, defaults last.

| Env | Config key | Default | Meaning |
| --- | --- | --- | --- |
| `MBX_CACHE_DIR` | `cache_dir` | `~/.cache/mbx` | store root |
| `MBX_REMOTE_URL` | `remote.url` | unset | remote cache base URL |
| `MBX_REMOTE_NAMESPACE` | `remote.namespace` | unset | required with url |
| `MBX_REMOTE_TOKEN` | `remote.token` | unset | bearer token |
| `MBX_REMOTE_TOKEN_FILE` | `remote.token_file` | unset | token from file |
| `MBX_REMOTE_OIDC_AUDIENCE` | `remote.oidc_audience` | unset | CI OIDC auth |
| `MBX_REMOTE_MODE` | `remote.mode` | `read-write` | or `read-only`/`write-only` |
| `MBX_STATS_REPORT` | `stats_report` | unset | write JSON stats to path |
| `MBX_SHARE_OUT_DIR` | `share_out_dir` | off | share compilations that read `OUT_DIR` |
| `MBX_GC_AUTO` | `gc.auto` | `true` | sweep the store after a build |
| `MBX_GC_MAX_SIZE` | `gc.max_size` | `20GiB` | size the store is swept back to |
| `MBX_GC_INTERVAL` | `gc.interval` | `1h` | how often a build may sweep |
| `MBX_TARGET_VIEWS` | `target.views` | `true` | let mbx place target directories |
| `MBX_TARGET_ROOT` | `target.root` | `<cache dir>/targets` | where it places them |
| `MBX_HTTP_TIMEOUT` | `http.timeout` | `30s` | connect/read timeout |
| `MBX_HTTP_DOWNLOAD_TIMEOUT` | `http.download_timeout` | `10m` | blob downloads |
| `MBX_HTTP_RETRIES` | `http.retries` | `3` | request retries |
| `MBX_VERIFY` | — | unset | verify mode: compile anyway, compare |

Write policy: outside a trusted CI writer context (protected-branch push on
GitHub Actions / GitLab CI), `read-write` degrades to `read-only` and
`write-only` disables the remote. Same policy as mise; release/tag builds
never touch the cache.

This client-side check is a convenience, not a boundary — a client can set any
`GITHUB_*` variable it likes. The server's per-namespace read and write grants
are what actually enforce who may publish.

Credentials are never sent in the clear: an authenticated remote must be HTTPS,
with an exception for loopback development servers. Redirects are not followed at
all, so a bearer token or OIDC assertion cannot be forwarded to a host the
operator did not configure.

### Target-dir views

Enabled by default, with `MBX_TARGET_VIEWS=0` as the opt-out.
`MBX_TARGET_VIEWS` places the default target directory at
`<target root>/v1/<digest of the workspace root>` and symlinks `target` to it,
keyed by path alone so one checkout keeps one target directory whatever it
builds. A record beside the directory names the checkout it belongs to -- beside
and not inside, since `cargo clean` empties the directory and an untraceable
target directory could never be collected. Placement declines whenever it would
overrule a flag, the environment, a cargo configuration, or an existing real
`target/`.

### Concurrency

Sessions do not coordinate. Several cached Cargo commands and an `mbx gc` can share
one store: manifest updates take a file lock, CAS writes are content-addressed
and idempotent, and an eviction racing a restore turns that action into a miss.
The automatic sweep is throttled by a stamp file rather than coordinated, so two
builds finishing together may both sweep; checkout records are written whole,
one file per checkout and identity, so there is nothing for them to race over.
The failure mode is a recompile, never a wrong result -- the automatic sweep
makes that race routine rather than rare, not worse.

## CI story

- mbx ships as a prebuilt static binary; installing it is one step, which
  breaks the bootstrap cycle that motivated extraction (mise CI cannot use
  mise-task caching to build mise itself).
- Ephemeral runners start with an empty store; everything restores from the
  remote in batched blob packs.
- Write auth via OIDC (`MBX_REMOTE_OIDC_AUDIENCE`) — no long-lived secrets;
  fork PRs are read-only by the trusted-writer policy.
- A scheduled verify-mode job (`MBX_VERIFY=1`) recompiles and compares against
  cache results — a standing correctness canary.

## Delivery plan (PR stack)

1. `docs: add v1 plan` — this document.
2. `chore: workspace scaffolding` — Cargo workspace, CI (build/test/clippy
   `-D warnings`, no allows), rustfmt, LICENSE (MIT), mise tasks.
3. `feat: mbx-cache-core` — port from mise with wire renames; tests included.
4. `feat: mbx-cache-rustc` — port; depends on core.
5. `feat: session + shim library` — port mise's session/shim glue, severed
   from mise: own config, own identity, inlined utils.
6. `feat: mbx CLI` — `build`, `gc`, and `cache` commands, plus the cold/warm and
   second-checkout integration tests.
7. `docs: usage` — real README behind an experimental banner.
8. `feat: release binaries` — tagged release workflow producing one
   self-contained archive per target, checksummed, so installing mbx in CI
   is a single download.
9. `chore: release-plz` — versioning, changelog, tagging, and crates.io
   publishing move to release-plz, matching the other jdx.dev CLIs. See
   [RELEASING.md](RELEASING.md).

Server repo follow-up (separate stack in `jdx/mbx-cache`): rebrand crate,
env vars, and wire constants to match `mbx-cache-core` exactly.

## Future work (post-v1)

- **Predictive prefetch by default** — download the predicted action set for
  the whole dep graph in parallel at build start (the prediction plumbing
  already exists in the agent).
- **Deferred materialization** — leave artifacts in the CAS until read
  (biggest win for `cargo check`-heavy workflows).
- **Decide the `CARGO_INCREMENTAL` default.** `MBX_INCREMENTAL=1` now opts into
  leaving members incremental (off by default, forced off in CI), so the
  measurement is finally possible; the default itself is still unsettled. It
  cannot be decided per crate: extern rlibs enter the action key by content, and
  an incrementally built member emits different rlib bytes, so turning it on for
  one member cold-misses its whole dependent subtree. Measure on a multi-member
  workspace — mise is one crate, so nearly all of its 65% is dependencies and
  the trade-off will not show there.
- **Standalone shim mode**, so `build.rustc-wrapper` helps plain `cargo build`
  and `mbx setup` becomes worth having. This needs cross-process prefetch
  manifests first: `begin_task` derives its run id from the process id, so each
  of the many rustc processes cargo spawns would get its own manifest and see
  none of the others' predictions. Without prediction the shim can only use a
  dep-info file left by an earlier build, which is exactly the case a cold
  target directory does not have — so the feature is not worth shipping until
  runs can be shared. Either make the run id deterministic per identity, or
  persist predictions as they are recorded instead of at commit.
- **Default `MBX_SHARE_OUT_DIR` on**, once a verify-mode run over a real
  workspace has qualified it. Blocked on the verify-mode gap below rather than
  on the feature itself.
- **Verify mode across checkouts.** `MBX_VERIFY=1` compares restored outputs to
  a fresh compilation byte for byte, which a cross-checkout restore of a
  *workspace member* cannot pass: cargo compiles members in place, so each
  checkout's artifact records its own source path, and the cache serves one
  checkout's bytes to the other. That is true today, without
  `MBX_SHARE_OUT_DIR`, and it is why the canary cannot currently qualify
  cross-checkout sharing. Remapping `${workspace}` the way `OUT_DIR` is now
  remapped would fix it, at the cost of every backtrace into your own code
  naming a placeholder — a much larger change in what a build produces, and its
  own decision.
- **Link caching** — cache bin/dylib link outputs (hardest correctness
  surface, deliberately last).
- **Shared protocol crate** between client and server.
- **Signed provenance** for shared caches (sigstore), enabling org-level and
  eventually public warm caches.
- **RE-API bridge** for existing Bazel-ecosystem cache servers.
- **Shim overhead benchmark** in CI with a hard warm-exec budget (mise
  enforced 2ms; keep that bar).
- **mise integration** — mise consumes mbx and removes its embedded copy.
