---
description: Find configuration files, understand precedence, and look up every mbx setting.
---
# Configuration

Defaults work without a configuration file. Add only the values you want to
change. mbx reads configuration from three places; the first value found wins:

1. Environment variables (`MBX_*`).
2. `.mbx.toml` at the resolved Cargo workspace root, for the
   [supported workspace settings](#workspace-policy) only. `mbx exec` reads it
   from its [project root](/standalone-builds#sharing-results-across-checkouts)
   instead: `--project-root` when given, otherwise the enclosing checkout root,
   or the working directory outside a checkout.
3. `mbx/config.toml` in the platform configuration directory:
   - Linux: `~/.config/mbx/config.toml`, honoring `$XDG_CONFIG_HOME`
   - macOS: `~/Library/Application Support/mbx/config.toml`
   - Windows: `%APPDATA%\mbx\config.toml`

Anything still unset takes its default. mbx rejects unknown TOML keys, so a
misspelled setting is an error.

Shell examples on this page set a variable for one command with POSIX syntax,
`NAME=value command`. In PowerShell, set `$env:NAME` before the command and
remove it afterward.

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
one.

`unset` removes the key, so the setting falls back to its default. It also
removes a key mbx does not recognize. Use it to clear a misspelled key that
stops mbx from loading; the error for such a key names the command.

`mbx settings ls` prints every setting with its current value, and
`mbx settings ls gc` prints one group. `ls` does not print the value of
`remote.token`; `mbx settings get remote.token` does. `get` and `ls` read the
environment, the global file, and defaults; they do not read `.mbx.toml`. Edit
table settings such as `linker.profiles` in the file directly. Settings read
only from the environment, such as `MBX_VERIFY`, cannot be written with
`mbx settings set`.

## Sizes and durations

Sizes accept SI and IEC units. `20GB` and `20GiB` are different values. Durations
accept values such as `30s`, `15m`, `1h`, and `30d`.

## Common adjustments

| Change | Command or guide |
| --- | --- |
| Leave capacity for your editor | `mbx settings set scheduler.reserve_cpus 2`; [parallel builds](/scheduling) |
| Set one budget for cached build data | `mbx settings set gc.max_total_size 50GiB`; [single cache budget](#single-cache-budget) |
| Keep the action store under a fixed size | `mbx settings set gc.max_size 20GiB` |
| Keep live targets longer | `mbx settings set target.max_age 60d`; [managed target directories](/managed-targets#collection) |
| Collect agent worktrees' targets first | `mbx settings set target.evict_first .claude/worktrees`; [managed targets](/managed-targets#keep-or-evict-specific-checkouts) |
| Use factual savings messages | `mbx settings set savings plain` |
| Print more cache detail | `mbx settings set summary full`; [cache results](/cache-results) |
| Share results with CI | [Remote cache](/remote-cache) |
| Pin a linker per profile or target | [Managed linkers](/linkers) |
| Tune the shared compiler pool | [Parallel builds](/scheduling) |
| Keep private incremental state from the first compilation | `mbx settings set eager_incremental true`; [incremental builds](/incremental#eager-incremental-reuse) |
| Bound learned incremental state per crate | `mbx settings set learned_incremental_max_size 12GiB`; [incremental builds](/incremental#bound-the-storage) |
| Compare restored outputs with fresh compilations | `MBX_VERIFY=1`; [troubleshooting](/troubleshooting#verify-mode) |

<span id="managed-linkers"></span>
<span id="machine-wide-compile-scheduling"></span>
<span id="incremental-builds"></span>
<span id="learned-incremental-reuse"></span>
<span id="verify-mode"></span>

## Local build storage

Keep the cache directory and build outputs on local storage. Remote caches are
configured separately under `[remote]`.

By default, the cache directory (`cache_dir`, `MBX_CACHE_DIR`) is `mbx` inside
the platform cache directory:

- Linux: `~/.cache/mbx`, honoring `$XDG_CACHE_HOME`
- macOS: `~/Library/Caches/mbx`
- Windows: `%LOCALAPPDATA%\mbx`

`mbx cache dir` prints the store's path, `<cache_dir>/actions`, not the cache
directory itself.

On Linux and macOS, mbx refuses to start a build when the cache directory or
Cargo output storage is on NFS; other platforms do not run this check. Set
`cache_dir` (`MBX_CACHE_DIR`) and, when configured separately, `target.root`
(`MBX_TARGET_ROOT`) to local storage. Cargo target directories and separate
intermediate build directories that you choose must also be local: check
`CARGO_TARGET_DIR` / `build.target-dir` and `CARGO_BUILD_BUILD_DIR` /
`build.build-dir`.

The check follows symlinks and checks the destination filesystem even when the
directory does not exist yet. An NFS source checkout is supported when its build
outputs are local, including a `target` link into a local managed target
directory. Remote cache URLs are unaffected; use a [remote cache](/remote-cache)
to share results across machines.

Help, cache inspection, and cleanup commands still run with the old
configuration, so you can inspect or remove previous NFS storage. Changing the
configuration does not copy the cache to the new disk; expect a cold cache.
See [Change target placement](/managed-targets#change-target-placement) before
moving managed targets to another disk.

## Containers sharing a cache

Compiler shims are executable wrappers, not cached build artifacts, but by
default both live under `cache_dir`. When containers share that directory but
each has its own mbx installation, set `shims_dir` (`MBX_SHIMS_DIR`) in each
container to a private, dedicated local directory:

```sh
MBX_CACHE_DIR=/shared/mbx MBX_SHIMS_DIR=/var/lib/worker/mbx-shims mbx build
```

The shim directory must survive later builds, because CMake and other build
systems can record absolute compiler or launcher paths. mbx resolves
`shims_dir` as follows:

- Unset: `<cache_dir>/shims`
- Absolute path: used as given
- Relative path: resolved under `cache_dir`; rejected if `..` climbs out of it
- Empty, or a relative path that normalizes to empty (such as `.`, `./`, or
  `a/..`): rejected

The setting also covers `mbx exec` and CMake launchers; cached artifacts stay in
the shared `cache_dir`. Set it in the global file or the environment; it is not
a workspace policy.

Use a directory reserved for mbx shims, with no real compilers in it. mbx marks
shim directories with `.mbx-shims` and skips them when searching for real
compilers.

Changing `shims_dir` does not rewrite build configurations generated earlier. If
one still records an old shim path, reconfigure that build with the new setting
and its original configure options. For a CMake build, that is
`mbx exec cmake --fresh -S . -B build` plus those options.

::: warning Keep shims while builds run
Do not remove a shim directory while a build uses it. Updating mbx during an
active build retains the existing executable-replacement limitations.
:::

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
incremental state, and [generated source trees](/limits#out-dir-sharing).
These components share the budget without the usual disk-scaled size caps.
The per-crate learned incremental limit defaults to this same budget.
Explicit component limits still apply; remove those settings if you want mbx
to manage the allocation. Age limits and the automatic disk-free-space
safeguard remain enabled.

Collection reserves only the space the action store occupies, up to its limit,
so an empty store does not force useful targets out. Learned incremental state
and generated source trees use the remaining allowance; managed targets use what
remains after them. If protected state leaves less room, mbx collects the
action store to fit the remaining budget. This order favors shared cached
results and incremental state over older target directories; allocation is not
based on measured rebuild cost.

The budget is a **logical-byte collection target**, not a physical disk quota.
Shared blocks can make physical usage smaller, while metadata, session history,
and temporary files add overhead outside the budget. Builds can exceed the
budget between sweeps. Active builds, the most recently used state, explicitly
kept targets, and untracked state can also prevent collection from reaching it;
mbx warns when the combined remainder exceeds the budget. If a component cannot
be measured, mbx reports that the combined budget could not be verified and
conservatively gives the action store no remaining allowance. Use
`mbx gc --dry-run` to inspect collection, or `mbx gc --json` for each
component's logical sizes.

One budget applies even when managed targets live on a different disk from the
cache directory; free-space safeguards still operate per disk. Setting
`gc.max_total_size = "none"` restores the
[disk-scaled defaults](/managed-targets#budgets-scale-with-the-disk) for any
component without an explicit limit.

<span id="disk-scaled-defaults"></span>

## Example global configuration {#example}

This example shows several available controls, not a recommended configuration.
Copy only the settings you need into your global configuration file. Write a
dotted setting under its TOML section header: `gc.max_size` is `max_size` under
`[gc]`. Remote settings and machine-specific paths do not belong in a
checked-in `.mbx.toml`, which rejects them; see
[Workspace policy](#workspace-policy).

<details>
<summary>Show the example</summary>

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
max_total_size = "50GiB" # optional combined budget
min_free_size = "20GiB"  # default: 10% of each disk
interval = "1h"

[target]
views = true
max_size = "30GiB"       # default: 10% of the target disk
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

A repository may check in a `.mbx.toml` containing only these settings:

- `incremental`, `eager_incremental`, `share_out_dir`, `share_workspace_root`,
  `build_script_execution`, and `cc`
- `linker.default` and `linker.profiles`, with any
  [selector](/linkers#selectors) except `path:`
- `scheduler.enabled`, `scheduler.cpus`, `scheduler.reserve_cpus`,
  `scheduler.memory`, `scheduler.priority`, `scheduler.pressure`, and
  `scheduler.tests`

For example:

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
credentials, diagnostics, target placement, and collection settings are not
accepted from a repository-owned file. mbx reports an error for an unsupported
or misspelled workspace setting.

See [`share_out_dir`](#share-out-dir) and
[`build_script_execution`](#build-script-execution) in the settings reference,
and [`OUT_DIR` sharing](/limits#out-dir-sharing) for eligibility and
compatibility details.

`share_workspace_root = false` is the global default. Setting it to true maps
the workspace root to a placeholder wherever rustc records a source path. A
crate rebuilt in a second checkout then comes out byte-identical, so the crates
that depend on it still share cached results. The setting suits a machine that
builds many checkouts of one repository. The cost is that debug information,
`file!()`, and panic locations name the placeholder instead of the real source
path. See
[A rebuilt workspace crate records its checkout](/limits#a-rebuilt-workspace-crate-records-its-checkout).

## Build-script C and C++

`cc = true` (`MBX_CC`, on by default) caches host C and C++ compiled by Cargo
build scripts, such as the native code built by `*-sys` crates. No project
changes are required: for the duration of the mbx command, build scripts use
mbx's compiler wrappers.

If mbx cannot safely model a compiler call, it runs the real compiler without
caching that call. Run `mbx explain` to see why a build bypassed the cache, or
read the
[full C and C++ limits](/limits#c-and-c-caching-covers-the-host-compiles-mbx-drives).

To cache C and C++ builds that run outside Cargo, put the build command after
`mbx exec`. With `cc = false`, `mbx exec` warns and runs the command uncached.
See [Cache C and C++ builds outside Cargo](/standalone-builds).

## The savings line

`savings` controls the one-line report of accumulated savings after a build
(`MBX_SAVINGS` from the environment). `quips`, the default, draws the line from
a pool of dry one-liners. `plain` reports the same figures without a quip. `off`
keeps the totals without printing anything.

## Build summaries

`summary` (`MBX_SUMMARY` from the environment) controls the build summary mbx
prints to stderr after a build:

| Value | Prints |
| --- | --- |
| `auto` (default) | `ci` in CI, `short` otherwise |
| `short` | One line; routine compiler probes (`compiler-query`, `standard-input`, and their `cc-` counterparts) are left out of its bypass count |
| `ci` | The `short` line plus session timing, estimated compiler time avoided, explanations for compilations not looked up, and bypass reasons |
| `full` | Detailed timing, compiler, bypass, transfer, and output-restoration figures |
| `off` | No build summary; `MBX_STATS_REPORT` is still written when configured |

`auto` treats a build as CI when `CI` or `GITHUB_ACTIONS` is `1`, `true`, or
`yes` (case-insensitive). Set `short`, `ci`, `full`, or `off` to use that
style everywhere. Cargo's `-q` and `--quiet` also suppress the summary for
that invocation.

The `ci` style's `object cache:` counts and transfers exclude artifacts Cargo
reused directly and archives that a CI cache step restored or saved. Compiler
time avoided is summed across compilations, not elapsed job time saved. In CI,
mbx also skips the first-build notice about local cache management.

## Settings

The complete setting reference is generated from the setting declarations and
doc comments in `crates/mbx/src/config.rs`. Environment-only settings are
labeled in the entries below.

<!-- The line range skips the generated header and file-precedence preamble;
     the top of this page describes precedence more completely. -->
<!--@include: ./cli/configuration.md{9,}-->
