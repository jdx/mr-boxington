# Configuration

mbx loads environment variables over `mbx/config.toml` in the platform
configuration directory, then falls back to defaults. This honors platform and
XDG overrides through the operating system's configuration-directory lookup.
Unknown TOML keys are rejected so misspelled settings do not silently do
nothing.

## Example

```toml
# <config directory>/mbx/config.toml
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

## Explicit remote prefetch

After a command has published its action manifest, another machine can warm
the same build without running Cargo:

```sh
mbx prefetch build --workspace --release
```

The workspace's `Cargo.lock` and complete Cargo argument list select the same
manifest as a normal mbx build. Prefetch requires a configured remote in
`read-only` or `read-write` mode, waits for every predicted action, and returns
an error when the manifest lookup fails.

## Incremental builds

`MBX_INCREMENTAL=1` stops mbx from forcing `CARGO_INCREMENTAL=0` locally. This
can help an edit/rebuild loop, but an incremental workspace artifact changes
the inputs of crates above it and reduces reuse across worktrees. CI always
disables it because a fresh runner has no incremental state to reuse.

## Sizes and durations

Sizes accept SI and IEC units. `20GB` and `20GiB` are different values. Durations
accept values such as `30s`, `15m`, and `1h`.
