# FAQ

Short answers, with the page that owns each one.

## Is deleting the cache safe?

Always. The store is a cache of work mbx can redo: the worst case is a colder
build, never a wrong one. See [stability](/stability#the-store-is-disposable).

## Why wasn't my first build faster?

A first build has an empty store — there is nothing to hit, so it can only
cost time, and the summary's "could not look up" line dominates. The second
build is the honest measurement. See
[cache results](/cache-results#troubleshooting-a-low-hit-rate) and the cold
scenario in [benchmarks](/benchmarks#cold).

## Why does a fully warm build still take time?

Links mbx cannot describe always run — a large binary re-links even when every
compilation hit. Host binaries and tests are restored on Linux and macOS, and
self-contained WebAssembly targets everywhere; the rest is
[limits](/limits#native-linking-is-cached-only-where-the-linker-can-be-described).

## The cache stopped hitting after a Rust update. Is it broken?

No — the compiler is part of every key, so a toolchain roll invalidates every
rustc action at once, and the build says so:
`a manifest predicting N compilations was loaded, but none matched this build`.
The [toolchain benchmark scenario](/benchmarks#toolchain) exists to pin down
exactly this diagnosis. Actions that do not depend on rustc, such as a build
script's C objects, legitimately survive.

## Can I use mbx together with sccache?

No. Both wrap rustc through `RUSTC_WRAPPER`, so they cannot be combined for
the same build; with `RUSTC_WRAPPER` already set, mbx defers to it and does
not cache. See [migrate from rust-cache or sccache](/cookbook/migrate).

## Are restored artifacts byte-identical to what a compile would produce?

Equivalent, not always identical: rustc and C compilers record absolute
source paths in metadata and debug information, so artifacts from two
checkouts can differ without behaving differently.
[`MBX_VERIFY=1`](/configuration#verify-mode) compares bytes and names what
differed. See
[limits](/limits#restored-artifacts-are-equivalent-not-always-identical).

## Where does everything live?

`mbx cache dir` prints the store's location. Managed target directories live
under the same root ([managed targets](/managed-targets)), and configuration
comes from the paths listed at the top of [configuration](/configuration).

## How do I turn one feature off?

Every feature has its own switch, so none of them has to be a package deal:

| Switch | Turns off |
| --- | --- |
| `MBX_SCHEDULER=0` | [machine-wide compile scheduling](/configuration#machine-wide-compile-scheduling) |
| `MBX_CC=0` | [build-script and `mbx exec` C and C++ caching](/configuration#build-script-c-and-c) |
| `MBX_TARGET_VIEWS=0` | [managed target directories](/managed-targets#disable-managed-targets) |
| `MBX_CACHE_LINKS=0` | [native link caching](/limits#native-linking-is-cached-only-where-the-linker-can-be-described) |
| `MBX_LEARNED_INCREMENTAL=0` | [learned incremental reuse](/configuration#learned-incremental-reuse) |
| `MBX_EVENTS=0` | [per-compilation event streams](/tui#recording) |
| `MBX_SAVINGS=off` | [the savings line](/configuration#the-savings-line) |

## Something looks wrong. What should a report include?

Three things describe almost any mbx problem: `mbx doctor --json`,
`MBX_LOG=debug`, and `MBX_BYPASS_LOG`. See
[reporting a problem](/getting-started#reporting-a-problem).
