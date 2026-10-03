---
description: Find configuration files, understand precedence, and look up every mbx setting.
---
# Configuration

Defaults work without a configuration file. Add only the values you want to
change. mbx reads configuration from three places; the first value found wins:

1. Environment variables (`MBX_*`).
2. `.mbx.toml` at the resolved Cargo workspace root, for the
   [supported workspace settings](#workspace-policy) only.
3. `mbx/config.toml` in the platform configuration directory:
   - Linux: `~/.config/mbx/config.toml`, honoring `$XDG_CONFIG_HOME`
   - macOS: `~/Library/Application Support/mbx/config.toml`
   - Windows: `%APPDATA%\mbx\config.toml`

Anything still unset takes its default. Unknown TOML keys are rejected, so a
misspelled setting is an error.

## Change settings from the command line

`mbx settings set` writes one setting to the global configuration file and
creates the file if it does not exist yet:

```sh
mbx settings set gc.max_size 20GiB
mbx settings set target.evict_first .claude/worktrees,tmp
mbx settings get gc.max_size
mbx settings unset gc.max_size
```

The value must match the setting's type and allowed values and load as that
setting, or nothing is written. A problem already elsewhere in the file does not
block the edit, so `set` can repair one setting at a time; mbx warns about what
is still wrong. List settings take comma-separated items. Comments and
formatting elsewhere in the file are kept, and a file that is a symlink stays
one. `unset` removes the key, so the setting falls back to its default. It also
removes a key mbx does not recognize, which is how to clear a misspelled key that
stops mbx from loading; the error for such a key names the command.

`mbx settings ls` prints every setting with its current value, and
`mbx settings ls gc` prints one group. It does not print the value of
`remote.token`; `mbx settings get remote.token` does. `get` and `ls` read the environment, the
global file, and defaults; they do not read `.mbx.toml`. Table settings such as
`linker.profiles` are edited in the file directly. Settings that are read only
from the environment, such as `MBX_VERIFY`, cannot be written with
`settings set`.

## Sizes and durations

Sizes accept SI and IEC units. `20GB` and `20GiB` are different values. Durations
accept values such as `30s`, `15m`, and `1h`.

## Common adjustments

| Change | Command or guide |
| --- | --- |
| Leave capacity for your editor | `mbx settings set scheduler.reserve_cpus 2`; [parallel builds](/scheduling) |
| Set one budget for cached build data | `gc.max_total_size = "50GiB"`; [single cache budget](#single-cache-budget) |
| Keep the action store under a fixed size | `mbx settings set gc.max_size 20GiB` |
| Keep live targets longer | `mbx settings set target.max_age 60d`; [managed targets](/managed-targets) |
| Collect agent worktrees' targets first | `mbx settings set target.evict_first .claude/worktrees`; [managed targets](/managed-targets#keep-or-evict-specific-checkouts) |
| Use factual savings messages | `mbx settings set savings plain` |
| Print more cache detail | `mbx settings set summary full`; [cache results](/cache-results) |
| Share results with CI | [Remote cache](/remote-cache) |
| Pin a linker per profile or target | [Managed linkers](/linkers) |
| Tune the shared compiler pool | [Parallel builds](/scheduling) |
| Keep private incremental state from the first compile | `mbx settings set eager_incremental true`; [incremental builds](/incremental#eager-incremental-reuse) |
| Bound learned incremental state per crate | `mbx settings set learned_incremental_max_size 12GiB`; [incremental builds](/incremental#bound-the-storage) |
| Compare restored outputs with fresh compiles | `MBX_VERIFY=1`; [troubleshooting](/troubleshooting#verify-mode) |

<span id="managed-linkers"></span>
<span id="machine-wide-compile-scheduling"></span>
<span id="incremental-builds"></span>
<span id="learned-incremental-reuse"></span>
<span id="verify-mode"></span>

In the file, use TOML section headers for dotted settings, as shown in the
example below.
Shell examples that set `NAME=value command` use POSIX syntax; PowerShell users
can set `$env:NAME` before the command and remove it afterward.

## Local build storage

Keep the working cache and build outputs on local storage. Remote caches are
configured separately under `[remote]`.

On Linux and macOS, mbx rejects NFS-backed working caches and Cargo output
storage before starting a build. Set `cache_dir` (`MBX_CACHE_DIR`) and, when
configured separately, `target.root` (`MBX_TARGET_ROOT`) to local storage.
User-selected Cargo target directories and separate intermediate build
directories must also be local: check `CARGO_TARGET_DIR` / `build.target-dir`
and `CARGO_BUILD_BUILD_DIR` / `build.build-dir`.

The check follows symlinks and checks the destination filesystem even when
the directory has not been created yet. An NFS source checkout is supported
when its build outputs are local, including a `target` link into a local
managed target directory. Remote cache URLs are unaffected; use a
[remote cache server](/remote-cache) to share results across machines.
Other platforms do not currently enforce this filesystem check.

Help, cache inspection, and cleanup commands remain available with the old
configuration so you can inspect or remove previous NFS storage. Changing the
configuration does not copy the cache to the new disk; expect a cold cache.
See [changing target placement](/managed-targets#change-target-placement) before
moving managed targets to another disk.

## Containers sharing a cache

Compiler shims are executable wrappers, not cached build artifacts. By default
both live under `cache_dir`. If containers share that directory but have private
mbx installations, set `shims_dir` (`MBX_SHIMS_DIR`) to a private, dedicated local directory
in each container:

```sh
MBX_CACHE_DIR=/shared/mbx MBX_SHIMS_DIR=/var/lib/worker/mbx-shims mbx build
```

The shim directory must survive subsequent builds: CMake and other build systems
can record absolute compiler or launcher paths. Absolute values are used directly;
relative values resolve beneath `cache_dir` and are rejected if `..` would traverse
above it. Explicit empty values and relative values that normalize to an empty
path (such as `.`, `./`, or `a/..`) are rejected. The default remains
`<cache_dir>/shims`. This setting also covers `mbx exec` and CMake launchers;
cached artifacts remain in the shared cache. It is a global or environment setting,
not a workspace policy.

Use a directory reserved for mbx shims, with no real compilers in it. mbx marks
shim directories with `.mbx-shims` and excludes those directories when searching
for real compilers.

Changing this setting does not rewrite existing generated build configurations.
If one still records an old shim path, reconfigure that build using the new setting
(for example, `mbx exec cmake --fresh -S . -B build` for a CMake build,
reapplying its original configure options). Do not
remove a shim directory while a build uses it. Updating mbx during an active build
retains the existing executable-replacement limitations.

## Single cache budget

To manage cached build data with one size setting, add this to your global
configuration:

```toml
[gc]
max_total_size = "50GiB"
```

Or run `mbx settings set gc.max_total_size 50GiB`. The environment equivalent is
`MBX_GC_MAX_TOTAL_SIZE`.

The budget covers action-store objects and results, managed targets, learned
incremental state, and generated source copies. These components share the
budget without the usual disk-scaled size caps. The per-crate learned
incremental limit defaults to this same budget. Explicit component limits
still apply; remove those settings if you want mbx to manage the allocation.
Age limits and the automatic disk-free-space safeguard remain enabled.

Collection reserves only the space the action store actually occupies, up to
its limit, so an empty store does not force useful targets out. Learned
incremental state and generated sources use the remaining allowance; targets
use what remains after them. If protected state leaves less room, the action
store is collected to fit the remaining budget. This favors shared cached
results and incremental state over older target directories; allocation is
not based on measured rebuild cost.

This is a **logical-byte collection target**, not a physical disk quota. Shared
blocks can make physical usage smaller, while metadata, session history, and
temporary files add overhead outside the budget. Active builds, the most
recently used state, explicitly kept targets, and untracked state can prevent
collection from reaching the target; mbx warns when the combined remainder
exceeds it. If a component cannot be measured, mbx reports that the combined
budget could not be verified and conservatively gives the action store no
remaining allowance. Builds can also exceed it between sweeps. Use `mbx gc --dry-run`
to inspect collection, or `mbx gc --json` for each component's logical sizes.

The budget spans the cache and managed targets even when they live on separate
disks; free-space safeguards still operate per disk. Setting
`gc.max_total_size = "none"` restores the
[disk-scaled defaults](/managed-targets#budgets-scale-with-the-disk) for any
component without an explicit limit.

<span id="disk-scaled-defaults"></span>

## Example

This example shows several available controls, not a recommended configuration.
Copy only the settings you need into your global configuration file. Remote settings
and machine-specific paths do not belong in a checked-in `.mbx.toml`.

<details>
<summary>Example global configuration</summary>

```toml
# <config directory>/mbx/config.toml
cache_dir = "/var/cache/mbx"
incremental = false
eager_incremental = false  # opt-in state from the first compilation
learned_incremental_max_size = "8GiB"  # or "none"
share_out_dir = true
share_workspace_root = false
build_script_execution = true
cc = true
summary = "auto"         # or "short", "ci", "full", "off"
savings = "quips"        # or "plain", "off"

[linker]
default = "system"

[linker.profiles.dev]
x86_64-unknown-linux-gnu = "mold@2.42.0"
aarch64-unknown-linux-gnu = "wild@0.10.0"
default = "rust-lld"

[linker.profiles.release]
default = "system"

[gc]
auto = true
max_size = "20GiB"       # default: 5% of the cache disk
incremental_max_size = "20GiB" # default: 5% of the cache disk
incremental_max_age = "30d"    # default
max_total_size = "50GiB" # optional action + target + incremental budget
min_free_size = "20GiB"  # default: 10% of each disk
interval = "1h"

[target]
views = true
max_size = "30GiB"       # default: 10% of the cache disk
max_age = "30d"          # default
keep = ["~/src/app"]     # never collected for age or size
evict_first = [".claude/worktrees"]  # collected first when over budget

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

</details>

## Workspace policy

A repository may check in a `.mbx.toml` containing the build-policy switches
and scheduler policy below:

```toml
incremental = false
eager_incremental = false
share_out_dir = false
share_workspace_root = false
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

The [`share_out_dir`](#share-out-dir) and
[`build_script_execution`](#build-script-execution) settings are described in
the reference below; see
[`OUT_DIR` sharing](/limits#out-dir-sharing) for eligibility and compatibility
details.

`share_workspace_root = false` is the global default. Setting it to true maps
the workspace root to a placeholder wherever rustc records a source path, so a
crate rebuilt in a second checkout comes out byte-identical and the crates above
it still share. It is worth turning on for a machine that builds many checkouts
of one repository, and costs literal source paths in debug information,
`file!()` and panic locations. See
[A rebuilt workspace crate records its checkout](/limits#a-rebuilt-workspace-crate-records-its-checkout).

## Build-script C and C++

`cc = true` (`MBX_CC`, on by default) caches host C and C++ compiled by Cargo
build scripts, such as the native code built by `*-sys` crates. No project
changes are required: for the duration of the mbx command, build scripts use
mbx's compiler wrappers.

If mbx cannot safely model a compiler call, it runs the real compiler without
caching that call; use `mbx explain` to see why a build bypassed the cache, or
read the
[full C and C++ limits](/limits#c-and-c-caching-covers-the-host-compiles-mbx-drives).

To cache C and C++ builds that run outside Cargo, put the build command after
`mbx exec`. See [cache C and C++ builds outside Cargo](/standalone-builds).

## The savings line

`savings` controls the one-line report of accumulated savings after a build
(`MBX_SAVINGS` from the environment). `quips`, the default, draws the line from
a pool of dry one-liners. `plain` reports the same figures without a quip. `off`
keeps the totals without printing anything.

## Build summaries

`summary` controls the cache report printed to stderr after a build
(`MBX_SUMMARY` from the environment). `auto`, the default, selects `ci` when
`CI` or `GITHUB_ACTIONS` is `1`, `true`, or `yes` (case-insensitive), and `short`
otherwise. `short` prints one line and leaves routine `compiler-query` and
`standard-input` probes out of its bypass count. `ci` adds session timing,
estimated compiler time avoided, explanations for compilations that could not
be looked up, and bypass reasons. Its object-cache counts and transfers exclude
artifacts Cargo reused directly and archives restored or saved by a CI action.
Compiler time avoided is summed across compilations, not elapsed job time saved.
CI also skips the first-build notice about local cache management.

Set a fixed style to override automatic selection. `full` prints detailed
timing, compiler, bypass, transfer, and output-restoration figures. `off` prints
no cache summary, while still writing `MBX_STATS_REPORT` when configured.
Cargo's `-q` and `--quiet` also suppress the summary for that invocation.

## Settings

The complete setting reference is generated from mbx's runtime declarations.
Environment-only settings are labeled in the entries below.

<!-- The line range skips the generated header and file-precedence preamble;
     the top of this page describes precedence more completely. -->
<!--@include: ./cli/configuration.md{9,}-->
