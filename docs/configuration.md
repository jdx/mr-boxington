# Configuration

mbx loads environment variables over `.mbx.toml` at the resolved Cargo
workspace root, then `mbx/config.toml` in the platform configuration directory,
then defaults. This honors platform and XDG overrides through the operating
system's configuration-directory lookup. Unknown TOML keys are rejected, so a
misspelled setting is an error rather than a silent no-op.

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
cc = true
savings = "quips"        # or "plain", "off"

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
url = "https://cache.example.com"  # or "s3://bucket/prefix"
namespace = "acme/backend"
mode = "read-write"
# s3_endpoint = "https://<account>.r2.cloudflarestorage.com"
# s3_region = "auto"

[http]
timeout = "30s"
download_timeout = "10m"
retries = 3
```

## Workspace policy

A repository may check in a `.mbx.toml` containing only the three build-policy
switches below:

```toml
incremental = false
share_out_dir = false
cc = true
```

Environment variables still win. Machine paths, remote-cache configuration,
credentials, diagnostics, target placement, and garbage collection are not
accepted from a repository-owned file. mbx reports an error instead of applying
an unsafe or misspelled workspace setting.

## Build-script C and C++

`cc = true` (`MBX_CC`, on by default) points build scripts at a caching `CC`
and `CXX` for the duration of an mbx command, so the C that sys-crates compile
is cached alongside their Rust. A build that already sets `CC`, `CXX`,
`TARGET_CC`, or `TARGET_CXX` keeps the compiler it chose and is left alone.

The shims never change a compilation: anything mbx cannot model exactly runs
the real compiler and publishes nothing. See [limits](/limits) for what is
admitted, and `mbx explain` for what a particular build bypassed.

## Verify mode

`MBX_VERIFY=1` compiles and consults the cache side by side and compares the
results. It is deliberately expensive — use it to qualify correctness, not for
everyday builds.

The build reports what it found:

```text
mbx[cache]: qualification: 24 verified, 0 diverged
```

Run it in the checkout that filled the cache, so that what is being measured
is whether a compilation reproduces itself rather than whether two checkouts
embed the same paths. Anything above zero divergences is a modeling bug worth
reporting; `MBX_BYPASS_LOG` and `mbx explain` show what was left out.

This is how to qualify an experimental setting such as
[`MBX_CACHE_LINKS`](/limits#native-linking-is-not-cached) on your own
workload before relying on it.

## The savings line

`savings` controls the one-line report of accumulated savings after a build
(`MBX_SAVINGS` from the environment). `quips` — the default — draws the line
from a pool of dry one-liners, `plain` states the same facts in the register
of the other `mbx[...]` lines, and `off` keeps the totals without printing
anything.

## Incremental builds

`MBX_INCREMENTAL=1` stops mbx from forcing `CARGO_INCREMENTAL=0` locally. This
can help an edit/rebuild loop, but an incremental workspace artifact changes
the inputs of crates above it and reduces reuse across worktrees. CI always
disables it because a fresh runner has no incremental state to reuse.

## Learned incremental reuse

The crate you are editing misses the cache on every build, because its content
is new every time. After three consecutive compilations whose sources changed,
mbx compiles that crate with its own incremental state rather than from
scratch, and keeps the result out of the shared cache — an incremental artifact
describes one checkout's edit history, not its source, so it is never something
another checkout should restore. The build reports those compilations as
`incremental` and says how many were held back.

What it watches is the crate's own sources, not its cache key. A key also
covers the artifacts a crate links against, so rebuilding one dependency
changes the key of everything above it; watching that would send a whole
dependency cone incremental because one crate at the bottom was edited, and
stop all of it publishing. Crates above the one you are editing keep compiling
and publishing normally.

Unchanged sources are never churn, however the compilation got here. A miss on
them means something else lost the result — a wiped `target/`, a first build in
a new checkout — so it compiles normally and publishes for everyone.

The record of what a crate last compiled belongs to one checkout, and lives in
`mbx-incremental/` inside that build's target directory alongside the
incremental state itself. A sibling worktree keeps its own, so one developer's
edit loop cannot mark a crate hot for a worktree that is merely building it. A
managed target reclaims both along with everything else it holds, and each
crate's incremental state is discarded once it passes 1 GiB.

CI never does this, for the same reason it never compiles incrementally: there
is no earlier state to build on. `MBX_LEARNED_INCREMENTAL=0` turns it off, and
`MBX_INCREMENTAL=1` supersedes it by handing the whole decision back to
cargo.

## Sizes and durations

Sizes accept SI and IEC units. `20GB` and `20GiB` are different values. Durations
accept values such as `30s`, `15m`, and `1h`.

## Settings

Every setting, generated from the same usage-rs declaration mbx uses at
runtime.

<!-- The line range skips the generated header and file-precedence preamble;
     the top of this page describes precedence more completely. -->
<!--@include: ./cli/configuration.md{9,}-->
