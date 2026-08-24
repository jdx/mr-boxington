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

The complete settings reference—including TOML keys, environment variables,
types, defaults, choices, and environment-only diagnostics—is generated from
the same usage-rs declaration mbx uses at runtime. See
[CLI and configuration reference](/cli#configuration).

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
