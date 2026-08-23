# mr boxington

> **Please ignore this project.** It is an experiment and is not intended for
> others to use yet. Nothing here is stable or supported, and the protocol may
> change without notice. Releases exist so CI can install a binary, not as a
> promise that any of it keeps working.

`mbx` is a build cache for Rust projects. It wraps cargo, caches individual
rustc compilations in a content-addressed store shared by every project and
worktree on your machine, and can share those compilations with CI and
teammates through a remote cache.

See [PLAN.md](PLAN.md) for the design and the road to v1.

## What it does

- **Caches each rustc action once.** A compilation you have done before is
  restored instead of repeated, whatever directory you are in.
- **Deduplicates across checkouts.** Cache keys hold no absolute paths, so a
  second worktree of the same dependency graph builds largely warm — measured at
  65% of actions on mise, with the shortfall explained under limits below. Each
  keeps its own `target/` directory, so there is no cargo lock contention
  between them.
- **Collects garbage.** The store has a size budget, and a build sweeps it back
  under that budget on its own -- something cargo has never done for `target/`.
  A checkout that no longer exists loses its artifacts before a live project
  does.
- **Shares through a remote.** CI and teammates can restore from a
  [self-hosted server](https://github.com/jdx/mbx-cache); an ephemeral runner
  with an empty store pulls only the actions its build needs.

It is a cargo wrapper, not a cargo replacement. Cargo keeps resolution, feature
unification, build planning, and linking.

## Install

Every release attaches one archive per target, holding the `mbx` binary and
nothing else, so extracting it into a directory on `PATH` is the whole install.
The Linux binaries are statically linked against musl and need nothing from the
host.

```sh
curl -fsSL https://github.com/jdx/mr-boxington/releases/latest/download/mbx-x86_64-unknown-linux-musl.tar.gz | tar -xzf - -C ~/.local/bin
```

Targets: `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`,
`aarch64-apple-darwin`, `x86_64-apple-darwin`, and `x86_64-pc-windows-msvc`
(a `.zip`). `SHA256SUMS` on the release covers all of them.

Asset names carry no version, so pin one in CI by swapping `latest/download`
for a tag: `releases/download/v0.1.0/mbx-x86_64-unknown-linux-musl.tar.gz`.
`mbx --version` reports which build you have.

Building from source works too: `cargo build --release`.

Releases are cut by release-plz; see [RELEASING.md](RELEASING.md).

## Usage

```sh
mbx build build                  # == cargo build
mbx build test --all-features    # == cargo test --all-features
```

`mbx build` passes everything after it to cargo unchanged. When the cache did
anything at all, it summarises that on stderr afterwards — a build with no cache
activity prints nothing:

```
cache: 12 hits, 3 misses, 12 prefetched; 4.1 MiB downloaded, 0 B uploaded, 1.2 MiB stored locally
cache could not look up 4 compilations: no usable dep-info from an earlier build and no prediction to derive an action key from
cache bypassed 7 compilations: 5 unsupported-crate-type, 2 unsupported-search-path
```

The last line accounts for compilations mbx declined to cache, grouped by
reason. It is the honest counterpart to the hit rate: a build can hit every
action it looked up and still spend most of its time on work that never entered
the cache.

The line above it is the other half of that honesty, and it dominates a genuinely
cold build. An action key comes from a dep-info file an earlier build left
behind, or from a prediction recorded by one. Without a usable key from either
-- no dep-info at all, or dep-info that does not yield one -- mbx has nothing to
look up and compiles without making the lookup. Those compilations are stored
afterwards, so they are not bypasses, and they are not misses either: a miss is
a lookup that found nothing. Counting them as neither is what once made a cold
build report `0 hits, 0 misses` while writing a gigabyte into the store.

Store management:

```sh
mbx cache dir       # where the store lives
mbx cache stats     # what it holds
mbx gc              # sweep to the configured budget
mbx gc --max-size 20GiB
```

A build sweeps the store itself when one is due -- at most once per
`MBX_GC_INTERVAL`, and only reporting when it evicted something:

```text
gc: evicted 128 objects and 3 action results (1.2 GiB); 18.9 GiB remain
```

A store inside its budget loses no cached compilations; a sweep that finds one
there still tidies its own bookkeeping, dropping the records of checkouts that
have gone. When a store is over budget, the objects that go first are the ones
no checkout on disk still needs: `mbx build` records the checkout it ran in, and
a checkout that has been deleted stops protecting what only it used. Worktrees
of one `Cargo.lock` share their cached compilations, so removing one of those
releases nothing while a sibling remains -- the sibling genuinely still needs
them.

Evicting an object that is still in use costs a recompile and nothing else, so
`gc` is always safe to run.

## Configuration

Environment variables win over `~/.config/mbx/config.toml`, which wins over the
defaults.

| Environment | Config key | Default | Meaning |
| --- | --- | --- | --- |
| `MBX_CACHE_DIR` | `cache_dir` | platform cache dir | store root |
| `MBX_REMOTE_URL` | `remote.url` | unset | remote cache base URL |
| `MBX_REMOTE_NAMESPACE` | `remote.namespace` | unset | required with a URL |
| `MBX_REMOTE_TOKEN` | `remote.token` | unset | bearer token |
| `MBX_REMOTE_TOKEN_FILE` | `remote.token_file` | unset | token read from a file |
| `MBX_REMOTE_OIDC_AUDIENCE` | `remote.oidc_audience` | unset | CI OIDC audience |
| `MBX_REMOTE_MODE` | `remote.mode` | `read-write` | or `read-only`, `write-only` |
| `MBX_SHARE_OUT_DIR` | `share_out_dir` | off | share compilations that read `OUT_DIR` |
| `MBX_STATS_REPORT` | `stats_report` | unset | write a JSON report here |
| `MBX_INCREMENTAL` | `incremental` | off | let cargo compile workspace members incrementally |
| `MBX_GC_AUTO` | `gc.auto` | `true` | sweep the store after a build |
| `MBX_GC_MAX_SIZE` | `gc.max_size` | `20GiB` | size the store is swept back to |
| `MBX_GC_INTERVAL` | `gc.interval` | `1h` | how often a build may sweep |
| `MBX_HTTP_TIMEOUT` | `http.timeout` | `30s` | connect and read timeout |
| `MBX_HTTP_DOWNLOAD_TIMEOUT` | `http.download_timeout` | `10m` | blob downloads |
| `MBX_HTTP_RETRIES` | `http.retries` | `3` | request retries |
| `MBX_VERIFY` | — | unset | compile anyway and compare against the cache |
| `MBX_BYPASS_LOG` | — | unset | append the full reason for every uncached compilation to this file |

```toml
# ~/.config/mbx/config.toml
[remote]
url = "https://cache.example.com"
namespace = "acme/backend"
```

## Remote caching

Point mbx at a server and it reads from it; whether it *writes* depends on
where it is running. Outside a trusted CI context — a push to a protected
branch on GitHub Actions or GitLab CI — `read-write` degrades to `read-only`,
and `write-only` disables the remote entirely. Pull requests never write, so a
fork cannot poison the cache. Tag and release builds do not use the cache at
all.

In GitHub Actions, `MBX_REMOTE_OIDC_AUDIENCE` authorizes writes through the
runner's OIDC token, so there is no long-lived secret to store:

```yaml
permissions:
  contents: read
  id-token: write
env:
  MBX_REMOTE_URL: https://cache.example.com
  MBX_REMOTE_NAMESPACE: acme/backend
  MBX_REMOTE_OIDC_AUDIENCE: mbx-cache
```

## How it works

`mbx build` starts a session that installs a rustc shim, runs an in-process
cache agent behind a unix socket (a current-user-only named pipe on Windows),
and points cargo at the shim through `RUSTC_WRAPPER`. Cargo then invokes the
shim for every compilation; the shim analyses the invocation, looks up the
action, and either restores its outputs or compiles and publishes them. The
agent exits with the build — there is no daemon.

Anything the analysis does not model exactly — an unrecognised flag, a path
that maps to no known root, incremental compilation, linking — bypasses the
cache and runs the real compiler. Correctness comes before hit rate, always.

Set `MBX_VERIFY=1` to compile *and* consult the cache, comparing the two. It is
slower than either, and it is how the cache is qualified.

## Status and limits

Working today: local caching, cross-checkout reuse, remote push and pull,
automatic garbage collection.

Not yet:

- **Plain `cargo build` gets nothing.** A shim outside `mbx build` has no
  session to talk to, and per-process prefetch manifests cannot be shared
  across the many rustc processes cargo spawns, so `build.rustc-wrapper` and an
  `mbx setup` command are waiting on that. Use `mbx build`.
- **A crate whose compilation consumes `OUT_DIR` does not share across
  checkouts by default.** That covers any crate with a build script that
  generates code included via `include!(concat!(env!("OUT_DIR"), …))`. `OUT_DIR`
  is an absolute path and an input to the compilation, so two checkouts produce
  different keys. Crates whose build scripts only emit cfgs or link directives
  share normally.

  `MBX_SHARE_OUT_DIR=1` lifts this. It does two things, and needs both:
  it passes `--remap-path-prefix` so rustc records the cache placeholder rather
  than the real path — which covers debug info, spans, and diagnostics — and
  then, before publishing, it reads the outputs and only uses the shared key if
  none of them carries the value anyway. The second half is not redundant: a
  crate that keeps `env!("OUT_DIR")` in a string constant puts the real path in
  its artifact, where no remapping reaches it, and such a compilation stays
  keyed to its own checkout.

  The cost is that generated sources appear in debug info as
  `${target}/debug/build/…` instead of a path a debugger can open, which is why
  this is opt-in. The residual gap is a value a crate *derives* from `OUT_DIR`
  rather than embedding — its length, say, or a hash of it. Reading the outputs
  cannot see that, so it is not covered.

  On this repository's own dependency graph (285 crates), a second checkout hit
  139 of the 147 compilations it looked up by default, and all 147 with the
  option on.
- **Linking is not cached**, so binaries and dylibs always link.
- **Incremental compilation is off by default** inside `mbx build`. Cargo builds
  dependencies non-incrementally anyway, which is where the cache earns its
  keep, and an incremental compilation is never cacheable.

  `MBX_INCREMENTAL=1` stops forcing it off and hands the decision back to cargo,
  which trades cache reuse for a faster edit-rebuild loop. Members you just
  edited were going to miss regardless, so incremental recompiles them faster
  than the cache can — but a member built incrementally emits a different rlib
  than a cached one, and extern rlibs enter the action key by content, so every
  crate above it in the graph misses too. Worth it when you rebuild one leaf
  crate repeatedly; not worth it when a second worktree of the same workspace is
  what you want warm. CI ignores the setting: a fresh runner has no earlier
  build to build on, so there is nothing to gain and reuse to lose. Since the
  setting stops overriding `CARGO_INCREMENTAL` rather than setting it, a `0`
  already in your environment still wins, and `mbx` says so rather than leaving
  you to wonder.
- **Eviction order is an approximation.** Within what a sweep is willing to
  evict, the oldest goes first by access time -- which `relatime` coarsens to
  roughly a day and `noatime` suppresses altogether, leaving the order to fall
  back on when an object was stored. Recording which checkouts are still here
  is what keeps that from mattering much: a wrong order inside the set nothing
  has abandoned costs a recompile.
- **The budget covers cached objects and their results**, not the whole cache
  directory. Prediction manifests, checkout records, and remote download
  staging sit outside it, so the directory is always somewhat larger than the
  number you set.

## License

MIT
