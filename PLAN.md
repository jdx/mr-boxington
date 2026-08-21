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
   reflink/hardlink, and evicts with a byte-budget LRU (`mbx gc`).
3. **Git worktrees build cold.** Fingerprints embed absolute paths, so a fresh
   worktree rebuilds everything. mbx's action keys use path mapping, so a new
   worktree is warm from its first build while keeping its own target dir (no
   cargo lock contention, no incremental-artifact thrashing).
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
  compilations bypass the cache by design.
- A daemon. The agent lives for the duration of one `mbx build`.

## Architecture

```text
mbx build [-- <cargo args>]
  └─ session (tempdir)
     ├─ shim install: hardlink/copy of the mbx binary named `mbx-rustc`
     ├─ agent: in-process, serves a unix socket / windows named pipe
     │    ├─ local CAS + action cache (~/.cache/mbx)
     │    └─ optional remote cache client (blob packs, OIDC/token auth)
     ├─ env injection: RUSTC_WRAPPER=<session>/mbx-rustc, MBX_SOCKET,
     │    MBX_STAGING_DIR,
     │    MBX_TASK, CARGO_INCREMENTAL=0, MBX_PREVIOUS_RUSTC_WRAPPER chaining
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

- **Session mode** (under `mbx build`): talks to the agent over the socket;
  gets prefetch, batched uploads, and staged blob packs.
- **Standalone mode** (no `MBX_SOCKET`): reads/writes the local store
  directly and optionally performs synchronous remote reads. This makes
  `build.rustc-wrapper = "<path>/mbx-rustc"` in `.cargo/config.toml` useful for
  plain `cargo build` without the wrapper. `mbx setup` installs this.

On any unsupported invocation, error, or bypass condition the shim execs the
real rustc transparently. Correctness beats hit rate everywhere.

### Cache identity

mise keyed its session manifests by task definition. mbx defines its own
identity (v1): a canonical JSON of `{version, workspace_root_marker, command}`
where `workspace_root_marker` is path-mapped so identical projects in
different worktrees share manifests. The identity only namespaces
prefetch manifests — action keys themselves come from `mbx-cache-rustc` and
are identity-independent. Bumping the identity version invalidates manifests,
not cached actions.

## Protocol

Same protocol as mise action-cache v1, renamed. The wire rename table:

| mise name | mbx name |
| --- | --- |
| `application/vnd.mise.cache-*` media types | `application/vnd.mbx.cache-*` |
| `mise-cache-protocol` header | `mbx-cache-protocol` |
| `mise-cache-namespace` header | `mbx-cache-namespace` |
| `MISEPK01` blob-pack magic | `MBXPACK1` |
| `MISE_CACHE_*` env vars | `MBX_*` (client), `MBX_CACHE_*` (server) |
| `mise-cache-rustc` shim stem | `mbx-rustc` |

Protocol version stays 1; nothing in the wild speaks the old names.

## Command surface (v1)

- `mbx build [-- <cargo args>]` — run cargo under a cache session.
- `mbx gc [--max-size <bytes|human>]` — LRU-evict the store to a byte budget.
- `mbx cache dir` / `mbx cache stats` — store location and contents summary.
- `mbx setup` — install the persistent standalone shim and wire
  `build.rustc-wrapper` in `~/.cargo/config.toml`. If that key already names
  another wrapper, such as sccache, setup reports it and changes nothing:
  silently displacing a wrapper the user chose is worse than doing nothing.
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

### Concurrency

Sessions do not coordinate. Several `mbx build` runs and an `mbx gc` can share
one store: manifest updates take a file lock, CAS writes are content-addressed
and idempotent, and an eviction racing a restore turns that action into a miss.
The failure mode is a recompile, never a wrong result.

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
   from mise: own config, own identity, inlined utils, standalone shim mode.
6. `feat: mbx CLI` — `build`, `gc`, `cache`, `setup` commands.
7. `docs: usage + integration tests` — real README (experimental banner),
   cold/warm/second-checkout integration tests.

Server repo follow-up (separate stack in `jdx/mbx-cache`): rebrand crate,
env vars, and wire constants to match `mbx-cache-core` exactly.

## Future work (post-v1)

- **Predictive prefetch by default** — download the predicted action set for
  the whole dep graph in parallel at build start (the prediction plumbing
  already exists in the agent).
- **Target-dir views** — manage `CARGO_TARGET_DIR` placement; GC roots per
  checkout so deleting a worktree releases its artifacts.
- **Deferred materialization** — leave artifacts in the CAS until read
  (biggest win for `cargo check`-heavy workflows).
- **Link caching** — cache bin/dylib link outputs (hardest correctness
  surface, deliberately last).
- **Shared protocol crate** between client and server.
- **Signed provenance** for shared caches (sigstore), enabling org-level and
  eventually public warm caches.
- **RE-API bridge** for existing Bazel-ecosystem cache servers.
- **Shim overhead benchmark** in CI with a hard warm-exec budget (mise
  enforced 2ms; keep that bar).
- **mise integration** — mise consumes mbx and removes its embedded copy.
