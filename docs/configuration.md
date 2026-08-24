# Configuration

mbx loads environment variables over `~/.config/mbx/config.toml`, then falls
back to defaults. Unknown TOML keys are rejected so misspelled settings do not
silently do nothing.

## Example

```toml
# ~/.config/mbx/config.toml
cache_dir = "/var/cache/mbx"
incremental = false
share_out_dir = false

[gc]
auto = true
max_size = "20GiB"
interval = "1h"

[target]
views = true

[remote]
url = "https://cache.example.com"
namespace = "acme/backend"
mode = "read-write"

[http]
timeout = "30s"
download_timeout = "10m"
retries = 3
```

## Settings

| Environment | TOML key | Default | Purpose |
| --- | --- | --- | --- |
| `MBX_CACHE_DIR` | `cache_dir` | platform cache directory | Cache root |
| `MBX_STATS_REPORT` | `stats_report` | unset | Write a JSON build report |
| `MBX_INCREMENTAL` | `incremental` | `false` | Let local workspace members compile incrementally |
| `MBX_SHARE_OUT_DIR` | `share_out_dir` | `false` | Share eligible compilations that read `OUT_DIR` |
| `MBX_GC_AUTO` | `gc.auto` | `true` | Sweep after a build when due |
| `MBX_GC_MAX_SIZE` | `gc.max_size` | `20GiB` | Action-store budget |
| `MBX_GC_INTERVAL` | `gc.interval` | `1h` | Minimum interval between automatic sweeps |
| `MBX_TARGET_VIEWS` | `target.views` | `true` | Let mbx place eligible target directories |
| `MBX_TARGET_ROOT` | `target.root` | `<cache>/targets` | Managed target root |
| `MBX_REMOTE_URL` | `remote.url` | unset | Remote cache URL |
| `MBX_REMOTE_NAMESPACE` | `remote.namespace` | unset | Remote namespace; required with a URL |
| `MBX_REMOTE_TOKEN` | `remote.token` | unset | Bearer token |
| `MBX_REMOTE_TOKEN_FILE` | `remote.token_file` | unset | File containing a bearer token |
| `MBX_REMOTE_OIDC_AUDIENCE` | `remote.oidc_audience` | unset | CI OIDC audience |
| `MBX_REMOTE_MODE` | `remote.mode` | `read-write` | `read-write`, `read-only`, or `write-only` |
| `MBX_HTTP_TIMEOUT` | `http.timeout` | `30s` | Connect and request timeout |
| `MBX_HTTP_DOWNLOAD_TIMEOUT` | `http.download_timeout` | `10m` | Blob download timeout |
| `MBX_HTTP_RETRIES` | `http.retries` | `3` | Request retries |

## Diagnostic environment variables

These are environment-only:

| Environment | Purpose |
| --- | --- |
| `MBX_VERIFY` | Compile and consult the cache, then compare outputs |
| `MBX_BYPASS_LOG` | Append the full reason for every bypassed compilation |

`MBX_VERIFY=1` is deliberately slower. It qualifies correctness; it is not a
normal build mode.

## Incremental builds

`MBX_INCREMENTAL=1` stops mbx from forcing `CARGO_INCREMENTAL=0` locally. This
can help an edit/rebuild loop, but an incremental workspace artifact changes
the inputs of crates above it and reduces reuse across worktrees. CI always
disables it because a fresh runner has no incremental state to reuse.

## Sizes and durations

Sizes accept SI and IEC units. `20GB` and `20GiB` are different values. Durations
accept values such as `30s`, `15m`, and `1h`.
