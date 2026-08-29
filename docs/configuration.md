# Configuration

mbx reads configuration from three places; the first value found wins:

1. Environment variables (`MBX_*`).
2. `.mbx.toml` at the resolved Cargo workspace root — the
   [build-policy switches](#workspace-policy) only.
3. `mbx/config.toml` in the platform configuration directory:
   - Linux: `~/.config/mbx/config.toml`, honoring `$XDG_CONFIG_HOME`
   - macOS: `~/Library/Application Support/mbx/config.toml`
   - Windows: `%APPDATA%\mbx\config.toml`

Anything still unset takes its default. Unknown TOML keys are rejected, so a
misspelled setting is an error rather than a silent no-op.

## Disk-scaled defaults

mbx bounds its own disk use without being configured. The two size budgets
default to a share of the disk holding the cache — 5% for the action store and
10% for managed target directories — each from its floor (5 GiB and 10 GiB) up
to 100 GiB, rounded down to a whole 5 GiB. Managed targets are also collected
after 30 days unused.

Setting any of them outright overrides the scaling; `"none"` disables
`target.max_size`, `target.max_age`, and `gc.max_total_size`. See
[managed target directories](/managed-targets) for what collection removes.

## Example

Every value below is optional; these are shown set explicitly.

```toml
# <config directory>/mbx/config.toml
cache_dir = "/var/cache/mbx"
incremental = false
share_out_dir = true
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

[scheduler]
enabled = true
cpus = 16                # default: logical CPUs
memory = "24GiB"         # default: 85% of physical memory
priority = "normal"      # or "low"
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

`share_out_dir = true` is the global default. A workspace may set it to false
when generated source paths must remain literal in debug information — for
C and C++ objects as well as Rust artifacts, since a build script's generated
headers reach both.

## Build-script C and C++

`cc = true` (`MBX_CC`, on by default) caches C and C++ compiled by Cargo build
scripts, such as the native code built by `*-sys` crates. No project changes
are required: for the duration of the mbx command, build scripts use mbx's
compiler wrappers.

mbx does not override a compiler the build already selected with `CC`, `CXX`,
`HOST_CC`, `HOST_CXX`, `TARGET_CC`, or `TARGET_CXX`. If mbx cannot safely
model a compiler call, it runs the real compiler without caching that call.
Use `mbx explain` to see why a build bypassed the cache, or read the full
[C and C++ limits](/limits#c-and-c-caching-covers-the-host-compiles-mbx-drives).

To cache C and C++ builds that run outside Cargo, put the build command after
`mbx exec`. See [cache C and C++ builds outside Cargo](/standalone-builds).

## Machine-wide compile scheduling

Cargo plans one build at a time: three simultaneous worktree builds each
believe they own the machine and multiply `-j`. mbx's shims sit in front of
every real compiler process across all of them, so multiple Cargo builds
running at the same time share the machine through one permit pool under the
cache directory (on by default; `MBX_SCHEDULER=0` turns it off). Cache hits
never wait, Cargo keeps its own dependency scheduling, and permits are released
by the kernel if a process dies, so a crashed build cannot wedge its siblings.

Builds running at the same time also stop repeating each other: a compilation
identical to one already running anywhere on the machine waits for that one
and restores its result instead of burning a core on it. How the pool weighs
compilations and links — and what happens when a guess is wrong — is described
in [how it works](/how-it-works#machine-wide-scheduling).

The pool is memory-aware. `scheduler.cpus` permits (default: logical CPUs)
divide `scheduler.memory` (default: 85% of physical memory, leaving headroom
for everything that is not a compiler; `"none"` keeps plain CPU permits). In a
container, "physical memory" means the cgroup's limit rather than the host's
RAM — a build in a 4 GiB container on a large machine is budgeted by the
4 GiB, because the rest was never its to spend.

`priority = "low"` (`MBX_SCHEDULER_PRIORITY`) is for builds nobody is sitting
at — CI on a shared box, an editor's background check. While a normal-priority
build is waiting for permits, low-priority builds leave a quarter of the pool
free for it.

Two limits are worth knowing: a single compilation larger than the machine is
still too large when it runs alone, and crates that have never been measured
start at one permit until the pool has seen them once.

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

This is how to qualify a setting whose tier you want to check against your
own workload, such as
[native link caching](/limits#native-linking-is-cached-only-where-the-linker-can-be-described),
before relying on it.

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
describes one checkout's edit history, not its source. The build reports those
compilations as `incremental` and says how many were held back.

The trigger is the crate's own sources, not its cache key, so a rebuilt
dependency — which changes the keys of everything above it — keeps compiling
and publishing normally, and so does a miss on unchanged sources (a wiped
`target/`, a first build in a new checkout). The record is private to each
checkout, living in `mbx-incremental/` inside that build's target directory
beside the incremental state itself; a managed target reclaims both, and each
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
