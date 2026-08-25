# Configuration

mbx loads environment variables over `.mbx.toml` at the resolved Cargo
workspace root, then `mbx/config.toml` in the platform configuration directory,
then defaults. This honors platform and XDG overrides through the operating
system's configuration-directory lookup. Unknown TOML keys are rejected so
misspelled settings do not silently do nothing.

## Disk-scaled defaults

mbx bounds its own disk use without being configured. The two size budgets
default to a share of the disk holding the cache — 5% for the action store and
10% for managed target directories — each from its floor (5GiB and 10GiB) up to
100GiB, rounded down to a whole 5GiB. Managed targets are also collected after
30 days unused.

Setting any of them outright overrides the scaling; `"none"` disables
`target.max_size`, `target.max_age`, and `gc.max_total_size`. See
[managed target directories](/managed-targets) for what collection removes.

## Example

Every value below is optional; these are shown set explicitly.

```toml
# <config directory>/mbx/config.toml
cache_dir = "/var/cache/mbx"
incremental = false
share_out_dir = false

[gc]
auto = true
max_size = "20GiB"       # default: 5% of the cache disk
max_total_size = "50GiB" # optional combined budget
interval = "1h"

[target]
views = true
max_size = "30GiB"       # default: 10% of the cache disk
max_age = "30d"          # default

[remote]
url = "https://cache.example.com"
namespace = "acme/backend"
mode = "read-write"

[http]
timeout = "30s"
download_timeout = "10m"
retries = 3
```

## Workspace policy

A repository may check in a `.mbx.toml` containing only the two build-policy
switches below:

```toml
incremental = false
share_out_dir = false
```

Environment variables still win. Machine paths, remote-cache configuration,
credentials, diagnostics, target placement, and garbage collection are not
accepted from a repository-owned file. mbx reports an error instead of applying
an unsafe or misspelled workspace setting.

## Settings

The complete settings reference—including TOML keys, environment variables,
types, defaults, choices, and environment-only diagnostics—is generated from
the same usage-rs declaration mbx uses at runtime. See
[CLI configuration reference](/cli/configuration).

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
