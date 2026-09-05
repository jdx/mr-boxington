# FAQ

Short answers, with the page that owns each one.

## Is deleting the cache safe?

Always. The store is a cache of work mbx can redo: the worst case is a colder
build, never a wrong one. See [stability](/stability#the-store-is-disposable).

## Why wasn't my first build faster?

A first build has an empty store. There is nothing to hit, so it can only
cost time, and the summary's "could not look up" line dominates. Time the
second build instead. See
[cache results](/cache-results#troubleshooting-a-low-hit-rate), and the
[warm scenario](/benchmarks#warm) for what that second build gets.

## Why does a fully warm build still take time?

Links mbx cannot describe always run, so a large binary re-links even when
every compilation hit. Host binaries and tests are restored on Linux, macOS,
and Windows, and self-contained WebAssembly targets everywhere. The rest is
[limits](/limits#native-linking-is-cached-only-where-the-linker-can-be-described).

## The cache stopped hitting after a Rust update. Is it broken?

No. The compiler is part of every key, so a toolchain roll invalidates every
rustc action at once, and the build says so:
`a manifest predicting N compilations was loaded, but none matched this build`.
The benchmarks cover this in the [toolchain scenario](/benchmarks#toolchain).
Actions that do not depend on rustc, such as a build script's C objects,
survive.

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

Every feature has its own switch:

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

`mbx doctor --json`, a run with `MBX_LOG=debug`, and `MBX_BYPASS_LOG`. See
[reporting a problem](/getting-started#reporting-a-problem).

## Why is it called Mr. Boxington?

The project is named after this cardboard box, christened “Mr. Boxington” by
jdx's daughter while he was working on mbx. The name stuck.

::: info For non-English speakers
The joke is in “Boxington”: it combines _box_ with _-ington_, an ending
familiar from English place names and surnames. The result makes an ordinary
cardboard box sound like a distinguished gentleman.
:::

<img src="/mr-boxington.jpeg" alt="A child kneeling inside a tall cardboard box decorated with a face and a strawberry" width="480">

_The original Mr. Boxington._
