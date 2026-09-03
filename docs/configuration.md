# Configuration

mbx reads configuration from three places; the first value found wins:

1. Environment variables (`MBX_*`).
2. `.mbx.toml` at the resolved Cargo workspace root, for the
   [build-policy switches](#workspace-policy) only.
3. `mbx/config.toml` in the platform configuration directory:
   - Linux: `~/.config/mbx/config.toml`, honoring `$XDG_CONFIG_HOME`
   - macOS: `~/Library/Application Support/mbx/config.toml`
   - Windows: `%APPDATA%\mbx\config.toml`

Anything still unset takes its default. Unknown TOML keys are rejected, so a
misspelled setting is an error.

## Disk-scaled defaults

The two size budgets default to a share of the disk holding the cache: 5% for
the action store (`gc.max_size`) and 10% for managed target directories
(`target.max_size`), each bounded at both ends. Managed targets are also
collected after 30 days unused. The table in
[managed target directories](/managed-targets#budgets-scale-with-the-disk)
lists the bounds and what collection removes.

Setting any of them outright overrides the scaling; `"none"` disables
`target.max_size`, `target.max_age`, and `gc.max_total_size`.

## Example

Every value below is optional; these are shown set explicitly.

```toml
# <config directory>/mbx/config.toml
cache_dir = "/var/cache/mbx"
incremental = false
learned_incremental_max_size = "8GiB"  # or "none"
share_out_dir = true
build_script_execution = true
cc = true
summary = "short"        # or "off", "full"
savings = "quips"        # or "plain", "off"

[linker]
default = "system"

[linker.profiles.dev]
aarch64-apple-darwin = "jdxld@0.10.0"
x86_64-unknown-linux-gnu = "mold@2.42.0"
default = "rust-lld"

[linker.profiles.release]
default = "system"

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
reserve_cpus = 2         # default: 0
memory = "24GiB"         # default: 85% of physical memory
priority = "normal"      # or "low"
```

## Managed linkers

mbx can select a linker by Cargo profile and target triple, install an exact
version from its official GitHub releases, and route native Rust links through
it. The built-in selectors are:

- `system`, which leaves Cargo's linker selection alone;
- `rust-lld` or `lld`, which uses the LLD shipped with the active Rust
  toolchain;
- `jdxld@<version>`, `mold@<version>`, or `wild@<version>`, which downloads the
  matching release asset and verifies GitHub's SHA-256 digest; and
- `path:<executable>`, which selects a linker mbx does not install.

Managed GitHub linkers require an exact version. Downloads are installed once
beneath `<cache_dir>/tools`; concurrent builds share the same installation
lock. `GITHUB_TOKEN` may authenticate GitHub API and download requests.

Within a profile table, an exact target triple wins over `default`. The
top-level `linker.default` applies when the active profile has no entry. Cargo's
ordinary profile is `dev`, `--release` selects `release`, `cargo bench` selects
`bench`, and `--profile <name>` selects that custom profile.

```toml
[linker]
default = "system"

[linker.profiles.dev]
aarch64-apple-darwin = "jdxld@0.10.0"
x86_64-unknown-linux-gnu = "mold@2.42.0"
aarch64-unknown-linux-gnu = "wild@0.10.0"

[linker.profiles.release]
default = "rust-lld"
```

`MBX_LINKER` overrides every file for one invocation:

```sh
MBX_LINKER=mold@2.42.0 cargo build
MBX_LINKER=system cargo build --release
```

mold and Wild currently provide managed releases for Linux. jdxld provides a
managed Apple Silicon macOS release. Unsupported host platforms fail before
Cargo starts rather than silently changing the linker. The selected executable
remains part of mbx's native-link cache identity.

## Workspace policy

A repository may check in a `.mbx.toml` containing the build-policy switches
and scheduler policy below:

```toml
incremental = false
share_out_dir = false
build_script_execution = true
cc = true

[linker.profiles.dev]
default = "rust-lld"

[scheduler]
reserve_cpus = 2
memory = "12GiB"
priority = "normal"
```

Environment variables still win. Machine paths, remote-cache configuration,
credentials, diagnostics, target placement, and garbage collection are not
accepted from a repository-owned file. mbx reports an error for an unsupported
or misspelled workspace setting.

`share_out_dir = true` is the global default. A workspace may set it to false
when generated source paths must remain literal in debug information. This
applies to C and C++ objects as well as Rust artifacts, because a build
script's generated headers reach both.

`build_script_execution = true` (`MBX_BUILD_SCRIPT_EXECUTION`) caches eligible
`build.rs` executions. Set it to false to keep compilation caching while every
build script runs normally.

## Build-script C and C++

`cc = true` (`MBX_CC`, on by default) caches host C and C++ compiled by Cargo
build scripts, such as the native code built by `*-sys` crates. No project
changes are required: for the duration of the mbx command, build scripts use
mbx's compiler wrappers.

mbx preserves a host compiler selected with `CC`, `CXX`, `HOST_CC`, or
`HOST_CXX`, and does not cache those compiles. For a cross-compile, mbx does
not guess the target toolchain. It caches only when the build names a compiler
with `CC_<target>`, `CXX_<target>`, `TARGET_CC`, or `TARGET_CXX`; mbx wraps
that compiler without replacing the build's choice.

If mbx cannot safely model a compiler call, it runs the real compiler without
caching that call. Use `mbx explain` to see why a build bypassed the cache, or
read the
[full C and C++ limits](/limits#c-and-c-caching-covers-the-host-compiles-mbx-drives).

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
and restores its result. How the pool weighs compilations and links, and what
happens when a guess is wrong, is described in
[how it works](/how-it-works#machine-wide-scheduling).

The pool is memory-aware. `scheduler.cpus` permits (default: logical CPUs),
less `scheduler.reserve_cpus` (default: 0), divide `scheduler.memory` (default:
85% of physical memory, leaving headroom for everything that is not a compiler;
`"none"` keeps plain CPU permits). The pool always keeps at least one permit.
Cargo's `-j`/`--jobs` or `CARGO_BUILD_JOBS` can cap how many weighted permits
one build holds without shrinking the machine-wide pool available to other
builds. In a container, physical memory means the cgroup's limit, so a build in
a 4 GiB container on a large machine is budgeted by the 4 GiB.

`priority = "low"` (`MBX_SCHEDULER_PRIORITY`) is for builds nobody is sitting
at, such as CI on a shared box or an editor's background check. While a
normal-priority build is waiting for permits, low-priority builds leave a
quarter of the pool free for it.

Two limits apply: a single compilation larger than the machine is still too
large when it runs alone, and crates that have never been measured start at one
permit until the pool has seen them once.

## Verify mode

`MBX_VERIFY=1` compiles and consults the cache side by side and compares the
results. It is expensive; use it to qualify correctness, not for everyday
builds.

The build reports what it found:

```text
mbx[cache]: qualification: 24 verified, 0 diverged
```

Run it in the checkout that filled the cache. Artifacts restored from another
checkout embed that checkout's paths and would be reported as divergences. Any
divergence in the same checkout is a modeling bug; please report it.
`MBX_BYPASS_LOG` and `mbx explain` show what was left out.

This is how to qualify a setting whose tier you want to check against your
own workload, such as
[native link caching](/limits#native-linking-is-cached-only-where-the-linker-can-be-described),
before relying on it.

## The savings line

`savings` controls the one-line report of accumulated savings after a build
(`MBX_SAVINGS` from the environment). `quips`, the default, draws the line
from a pool of dry one-liners. `plain` states the same facts in the register
of the other `mbx[...]` lines. `off` keeps the totals without printing
anything.

## Build summaries

`summary` controls the cache report printed to stderr after a build
(`MBX_SUMMARY` from the environment). `short`, the default, prints one line
and leaves routine `compiler-query` and `standard-input` probes out of its
bypass count. `full` prints the detailed timing, compiler, bypass, transfer,
and materialization breakdown. `off` prints no cache summary, while still
writing `MBX_STATS_REPORT` when configured. Cargo's `-q` and `--quiet` also
suppress the summary for that invocation.

## Incremental builds

::: tip Smart incremental is already on
mbx speeds up repeated edits without relying on `CARGO_INCREMENTAL`. It keeps
incremental state for crates you are actively changing while leaving everything
else reusable across branches and worktrees.

You probably do not want to turn it on. When policy allows, it makes local
workspace crates use Cargo's incremental mode. Those checkout-specific builds
bypass the shared cache and can make crates above them miss it too.
:::

`MBX_INCREMENTAL=1` stops mbx from forcing `CARGO_INCREMENTAL=0` locally. This
can help an edit/rebuild loop, but an incremental workspace artifact changes
the inputs of crates above it and reduces reuse across worktrees. CI always
disables it because a fresh runner has no incremental state to reuse.

## Learned incremental reuse

The crate you are editing misses the cache on every build, because its content
is new every time. On the first source edit to a crate inside the workspace,
mbx compiles that crate with its own incremental state and keeps the result out
of the shared cache, since an incremental artifact describes one checkout's
edit history. A crate outside the workspace switches after three consecutive
misses with changed sources. The build reports those compilations as
`incremental` and says how many were held back.

The trigger is the crate's own sources, not its cache key. A rebuilt dependency
changes the keys of everything above it, and those crates keep compiling and
publishing normally. So does a miss on unchanged sources, such as a wiped
`target/` or a first build in a new checkout. Once a crate is known to be hot,
later edits go straight to its private incremental state instead of rebuilding
and looking up a shared action the edit cannot hit. The record and state are
private to each checkout and live under `incremental/` in mbx's cache directory,
so `cargo clean` does not discard the next edit's speedup. Normal garbage
collection removes state for deleted or expired checkouts.

Each crate's state is discarded, with a warning naming the crate, once it
passes `learned_incremental_max_size` (`MBX_LEARNED_INCREMENTAL_MAX_SIZE`,
default 8 GiB; `"none"` lifts the limit). rustc keeps about one session's worth
of state per crate and removes the sessions it has superseded, so the state is
proportional to the crate rather than to how long it has been edited, and the
budget is a backstop rather than a size ordinary crates reach. A large binary
built with debug info keeps a few GiB. Set the budget below that and every edit
compiles from scratch while still being reported as incremental, so raise it
rather than lower it when the warning appears.

CI never does this, because a fresh runner has no earlier state to build on.
`MBX_LEARNED_INCREMENTAL=0` turns it off. `MBX_INCREMENTAL=1` supersedes it by
handing incremental compilation back to cargo, and `MBX_VERIFY=1` disables it
for the build being verified.

## Sizes and durations

Sizes accept SI and IEC units. `20GB` and `20GiB` are different values. Durations
accept values such as `30s`, `15m`, and `1h`.

## Settings

Every setting, generated from the same usage-rs declaration mbx uses at
runtime.

<!-- The line range skips the generated header and file-precedence preamble;
     the top of this page describes precedence more completely. -->
<!--@include: ./cli/configuration.md{9,}-->
