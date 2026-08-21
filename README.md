# mr boxington

> **Please ignore this project.** It is an experiment and is not intended for
> others to use yet. Nothing here is stable or supported, the protocol may
> change without notice, and there are no releases.

`mbx` is a build cache for Rust projects. It wraps cargo, caches individual
rustc compilations in a content-addressed store shared by every project and
worktree on your machine, and can share those compilations with CI and
teammates through a remote cache.

See [PLAN.md](PLAN.md) for the design and the road to v1.

## What it does

- **Caches each rustc action once.** A compilation you have done before is
  restored instead of repeated, whatever directory you are in.
- **Deduplicates across checkouts.** Cache keys hold no absolute paths, so a
  second worktree of the same dependency graph builds warm. Each keeps its own
  `target/` directory, so there is no cargo lock contention between them.
- **Collects garbage.** `mbx gc` evicts to a size budget, which cargo has never
  done for `target/`.
- **Shares through a remote.** CI and teammates can restore from a
  [self-hosted server](https://github.com/jdx/mbx-cache); an ephemeral runner
  with an empty store pulls only the actions its build needs.

It is a cargo wrapper, not a cargo replacement. Cargo keeps resolution, feature
unification, build planning, and linking.

## Usage

```sh
cargo build --release            # build mbx itself, for now
mbx build build                  # == cargo build
mbx build test --all-features    # == cargo test --all-features
```

`mbx build` passes everything after it to cargo unchanged. When the cache did
anything at all, it summarises that on stderr afterwards — a build with no cache
activity prints nothing:

```
cache: 12 hits, 3 misses, 12 prefetched; 4.1 MiB downloaded, 0 B uploaded, 1.2 MiB stored locally
```

Store management:

```sh
mbx cache dir       # where the store lives
mbx cache stats     # what it holds
mbx gc --max-size 20GB
```

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
| `MBX_STATS_REPORT` | `stats_report` | unset | write a JSON report here |
| `MBX_HTTP_TIMEOUT` | `http.timeout` | `30s` | connect and read timeout |
| `MBX_HTTP_DOWNLOAD_TIMEOUT` | `http.download_timeout` | `10m` | blob downloads |
| `MBX_HTTP_RETRIES` | `http.retries` | `3` | request retries |
| `MBX_VERIFY` | — | unset | compile anyway and compare against the cache |

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
garbage collection.

Not yet:

- **Plain `cargo build` gets nothing.** A shim outside `mbx build` has no
  session to talk to, and per-process prefetch manifests cannot be shared
  across the many rustc processes cargo spawns, so `build.rustc-wrapper` and an
  `mbx setup` command are waiting on that. Use `mbx build`.
- **Linking is not cached**, so binaries and dylibs always link.
- **Incremental compilation is disabled** inside `mbx build`. Cargo builds
  dependencies non-incrementally anyway, which is where the cache earns its
  keep.

## License

MIT
