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

## Common adjustments

| Change | Setting or guide |
| --- | --- |
| Leave capacity for your editor | `scheduler.reserve_cpus = 2`; [parallel builds](/scheduling) |
| Keep the action store under a fixed size | `gc.max_size = "20GiB"` |
| Keep live targets longer | `target.max_age = "60d"`; [managed targets](/managed-targets) |
| Use factual savings messages | `savings = "plain"` |
| Print more cache detail | `summary = "full"`; [cache results](/cache-results) |
| Share results with CI | [Remote cache](/remote-cache) |

Use TOML section headers for dotted settings, as shown in the example below.
Shell examples that set `NAME=value command` use POSIX syntax; PowerShell users
can set `$env:NAME` before the command and remove it afterward.

## Local build storage

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
Existing managed targets follow the normal [target relocation rules](/managed-targets).

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

This example shows several available controls, not a recommended configuration.
Copy the sections you need into your global configuration file. Remote settings
and machine-specific paths do not belong in a checked-in `.mbx.toml`.

<details>
<summary>Example global configuration</summary>

```toml
# <config directory>/mbx/config.toml
cache_dir = "/var/cache/mbx"
incremental = false
learned_incremental_max_size = "8GiB"  # or "none"
share_out_dir = true
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

</details>

## Managed linkers

Select a linker for each Cargo profile and target, or override it for one build
with `MBX_LINKER`. See [Managed linkers](/linkers) for selectors, prerequisites,
and complete examples.

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

Simultaneous mbx builds share CPU and memory permits. Set `scheduler.cpus`,
`scheduler.reserve_cpus`, `scheduler.memory`, and `scheduler.priority` to tune
that pool. See [Parallel builds](/scheduling) for examples and the difference
between a shared budget and Cargo's per-build `-j` limit.

## Verify mode

`MBX_VERIFY=1` compiles and consults the cache side by side and compares the
results. It is expensive; use it to qualify correctness, not for everyday
builds.

For routine checks, set `MBX_VERIFY_SAMPLE_RATE=5` (or `verify_sample_rate = 5`)
to verify approximately 5% of compilation identities. The range is 0–100;
0 disables sampling. Selection is stable across wrapper processes and build
order, so rerunning the same invocation selects the same sample. This samples
units, not elapsed compiler time. `MBX_VERIFY=1` takes precedence and verifies
all eligible units. Selected units rehash inputs and disable learned
incremental compilation, just like full verification.

The build reports what it found:

```text
mbx[cache]: qualification: 24 verified, 0 diverged
```

The verified count includes divergent compilations. Each compilation contributes
at most one divergence, reporting its first mismatch. Warnings identify the
adapter, unit and action; stdout/stderr differences include the first differing
byte offset, line number and bounded, escaped excerpts of both results. Cached
diagnostics are rewritten into this checkout's paths before comparison.

Cargo must actually invoke the compiler to verify anything. Run in the checkout
that filled the cache with a fresh target directory; an unchanged build in an
existing target can be a Cargo no-op. Keep the original target and shared store.
`MBX_BYPASS_LOG` and `mbx explain` show what was left out.

For audits across worktrees, populate and verify using the same virtual source
root. For example, run this from each checkout's workspace root, first with
`MBX_VERIFY=0` to populate, then with `MBX_VERIFY=1` in the other checkout:

```sh
RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }--remap-path-prefix=$PWD=/workspace" \
  CARGO_TARGET_DIR="$PWD/target-audit" MBX_VERIFY=1 mbx build --all-targets --locked
```

Use a fresh `target-audit` directory each time and the same toolchain, profile,
and other compiler flags. The source side of `--remap-path-prefix` is keyed
portably; its virtual destination must agree between checkouts. If using
`CARGO_ENCODED_RUSTFLAGS`, add the remap there instead: Cargo gives it precedence
over `RUSTFLAGS`.

Remapping reduces embedded-source-path differences; it does not guarantee
byte-identical outputs, especially for native links or paths outside the mapped
root. See [artifact equivalence](/limits#restored-artifacts-are-equivalent-not-always-identical).
Investigate remaining divergences rather than treating every cross-worktree
mismatch as harmless. Please report unexplained differences, including the
identified unit and action.

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
(`MBX_SUMMARY` from the environment). `auto`, the default, selects `ci` when
`CI` or `GITHUB_ACTIONS` is `1`, `true`, or `yes` (case-insensitive), and `short`
otherwise. `short` prints one line and leaves routine `compiler-query` and
`standard-input` probes out of its bypass count. `ci` adds session timing,
estimated compiler time avoided, explanations for compilations that could not
be looked up, and bypass reasons. Its object-cache counts and transfers exclude
artifacts Cargo reused directly and archives restored or saved by a CI action.
Compiler time avoided is summed across compilations, not elapsed job time saved.
CI also skips the first-build notice about local cache management.

Set a fixed style to override automatic selection. `full` prints the detailed timing, compiler, bypass, transfer,
and materialization breakdown. `off` prints no cache summary, while still
writing `MBX_STATS_REPORT` when configured. Cargo's `-q` and `--quiet` also
suppress the summary for that invocation.

## Incremental builds

Leave `MBX_INCREMENTAL` unset for mbx's default combination of shared caching
and private incremental state. `MBX_INCREMENTAL=1` hands control to Cargo and
reduces reuse across checkouts. See [Incremental builds](/incremental).

## Learned incremental reuse

mbx recognizes source edits and retains private state for the affected crates.
`learned_incremental_max_size` bounds that state per crate; its default is
`8GiB`. See [Learned incremental reuse](/incremental#learned-incremental-reuse)
for triggers, cleanup, and overrides.

## Sizes and durations

Sizes accept SI and IEC units. `20GB` and `20GiB` are different values. Durations
accept values such as `30s`, `15m`, and `1h`.

## Settings

The complete setting reference is generated from mbx's runtime declarations.
Environment-only settings are labeled in the entries below.

<!-- The line range skips the generated header and file-precedence preamble;
     the top of this page describes precedence more completely. -->
<!--@include: ./cli/configuration.md{9,}-->
