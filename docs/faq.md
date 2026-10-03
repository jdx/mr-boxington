---
description: Find answers about cache reuse, storage, toolchains, sccache, feature switches, and the project name.
---
# FAQ

Common questions about reuse, storage, and compatibility. For a specific
build problem, start with [Troubleshooting](/troubleshooting).

## Is deleting the cache safe?

Yes. The shared compiler outputs can be rebuilt. For routine cleanup, preview
with `mbx gc --dry-run`, then run `mbx gc` to reclaim space under the
configured policy. Deleting the whole cache directory also removes private
incremental state and, unless `target.root` places them elsewhere, managed
target directories, so stop running builds first. See
[The store is disposable](/stability#the-store-is-disposable).

## Why wasn't my first build faster?

A cold store has no results to restore. The first build records compilation
inputs and outputs for later reuse.

A second command in the same target directory is often already fresh to
Cargo, so it measures Cargo's freshness check rather than mbx reuse. To
measure reuse, build into
[two fresh target directories](/cache-results#measure-cache-reuse). The
[warm build benchmark](/benchmarks#warm-build) shows what such a second build
gains. If reuse stays low, see
[Troubleshoot a low hit rate](/cache-results#troubleshooting-a-low-hit-rate).

## Why does a fully warm build still take time?

Cargo still plans the build, mbx validates inputs and materializes outputs,
and any unsupported actions run normally. mbx can restore eligible host
binaries and tests on Linux, macOS, and Windows, but links with unmodeled
inputs still run. See
[Caching limits](/limits#native-linking-is-cached-only-where-the-linker-can-be-described).

## The cache stopped hitting after a Rust update. Is it broken?

No. The compiler is part of every cache key, so a toolchain update
invalidates every rustc action at once. The one-line summary that local
builds print counts those compilations as
[`not looked up`](/cache-results#could-not-look-up).

The `ci` summary (the default in CI) and the `full` summary
(`MBX_SUMMARY=full`) also name the cause:
`a manifest predicting N compilations was loaded, but none matched this build`.
They print that line only when the build loaded
[recorded predictions](/how-it-works#prediction-and-dep-info), looked nothing
up, and could not look up at least half as many compilations as were predicted.

Actions that do not depend on rustc, such as a build script's C objects,
survive. If any of them is looked up, whether it hits or misses, the
summaries leave that line out.

## Can I use mbx together with sccache?

No. Both wrap rustc through `RUSTC_WRAPPER`, so they cannot be combined for
the same build. When `RUSTC_WRAPPER` is already set, mbx defers to it, warns,
and does not cache the build. mbx treats a preset `RUSTC_WORKSPACE_WRAPPER`
other than Clippy's `clippy-driver` the same way for workspace crates: it
defers to the wrapper and does not cache those compilations. See
[Migrate from rust-cache or sccache](/cookbook/migrate#from-sccache).

## Are restored artifacts byte-identical to freshly compiled ones? {#are-restored-artifacts-byte-identical-to-what-a-compile-would-produce}

Equivalent, not always identical: rustc and C compilers record absolute
source paths in metadata and debug information, so artifacts from two
checkouts can differ without behaving differently.
[`MBX_VERIFY=1`](/troubleshooting#verify-mode) compares bytes and names what
differed. See
[Caching limits](/limits#restored-artifacts-are-equivalent-not-always-identical).

## Where does everything live?

`cache_dir` (`MBX_CACHE_DIR`) sets the cache directory. `mbx cache dir`
prints the store inside it, `<cache_dir>/actions`. By default, managed
targets live beside the store in `<cache_dir>/targets`;
[`target.root`](/managed-targets#change-target-placement) can place them
elsewhere. Configuration comes from the paths listed in
[Configuration](/configuration).

## How do I turn one feature off?

Each feature below has its own switch. `mbx settings set` turns it off from
then on; the environment variable turns it off for one command.

| Turns off | Command | For one command |
| --- | --- | --- |
| [machine-wide compile scheduling](/scheduling#machine-wide-compile-scheduling), including deduplication of identical compilations running at once | `mbx settings set scheduler.enabled false` | `MBX_SCHEDULER=0` |
| [build-script and `mbx exec` C and C++ caching](/configuration#build-script-c-and-c) | `mbx settings set cc false` | `MBX_CC=0` |
| [managed target directories](/managed-targets#disable-managed-targets) | `mbx settings set target.views false` | `MBX_TARGET_VIEWS=0` |
| [automatic collection](/managed-targets#collection); `mbx gc` still reclaims space on demand | `mbx settings set gc.auto false` | `MBX_GC_AUTO=0` |
| [native link caching](/limits#native-linking-is-cached-only-where-the-linker-can-be-described) | environment only | `MBX_CACHE_LINKS=0` |
| [learned incremental reuse](/incremental#learned-incremental-reuse) | `mbx settings set learned_incremental false` | `MBX_LEARNED_INCREMENTAL=0` |
| [per-compilation event streams](/tui#recording) | `mbx settings set events false` | `MBX_EVENTS=0` |
| [the savings line](/configuration#the-savings-line) | `mbx settings set savings off` | `MBX_SAVINGS=off` |

## Something looks wrong. What should a report include?

Include the output of `mbx doctor --json`, a build run with `MBX_LOG=debug`,
and the bypass log that `MBX_BYPASS_LOG` writes.
[Report a problem](/troubleshooting#reporting-a-problem) shows the commands
and what each one reveals about your machine.

## Why is it called Mr Boxington?

The project is named after this cardboard box, christened “Mr Boxington” by
jdx's daughter while he was working on mbx. The name stuck.

::: info For non-English speakers
The joke is in “Boxington”: it combines _box_ with _-ington_, an ending
familiar from English place names and surnames. The result makes an ordinary
cardboard box sound like a distinguished gentleman.
:::

<img src="/mr-boxington.jpeg" alt="A child kneeling inside a tall cardboard box decorated with a face and a strawberry" width="480">

_The original Mr Boxington._
