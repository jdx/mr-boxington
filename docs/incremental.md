---
description: Understand how mbx combines shared compiler caching with private incremental state for local edits.
---
# Incremental builds

mbx automatically keeps private incremental state for crates you edit. Unchanged
work remains eligible for the shared cache. Leave the defaults in place for a
normal edit/build loop.

| Mode | What is reused | Scope |
| --- | --- | --- |
| Shared cache | Complete matching compiler outputs | Across equivalent builds and checkouts |
| Learned incremental reuse | Intermediate work from earlier source edits | Private to one checkout |
| Eager incremental reuse | Private intermediate work seeded on the first compilation | One checkout, locally or in CI |
| Cargo incremental mode | Cargo-managed incremental state | Private to Cargo's target directory |

## Learned incremental reuse

An edited crate has new source content, so its old shared result cannot match.
On the first source edit to a workspace crate, mbx compiles it with private
incremental state. A crate outside the workspace switches after three
consecutive misses with changed sources. The result never enters the shared
cache; [the build summary](#incremental-counts-in-the-build-summary) counts the
compilation as `incremental`.

The trigger is a source change. An unchanged crate in a fresh target or a new
checkout still compiles and publishes normally when it misses the cache. Later
edits to a known active workspace crate go directly to its private state,
without a cache lookup.

Dependents of a crate using private incremental state also keep private state,
because their inputs contain an artifact other checkouts cannot share. They can
publish again after the edited crate compiles once more without a source change.

### Where state lives

mbx keeps the state and its records under `incremental/` in the cache
directory, separately for each checkout. `cargo clean` does not remove this
state. `mbx clean` and `mbx cache remove` remove it for the selected workspace.
[Collection](/managed-targets#collection) removes it for deleted, expired, or
least-recently-used checkouts, but never removes state a running build is using.

### Bound the storage

`learned_incremental_max_size` limits each crate to 8 GiB by default. Once a
crate exceeds that limit, mbx warns and discards its state before the next
compilation. Raise it with:

```sh
mbx settings set learned_incremental_max_size 12GiB
```

The environment equivalent is `MBX_LEARNED_INCREMENTAL_MAX_SIZE`; `"none"`
disables the limit. rustc normally removes superseded sessions, so the limit is
a backstop, but a large crate in a debug build can need several GiB. If the
warning appears on every edit, raise the limit; otherwise mbx keeps discarding
the state and the crate recompiles in full on every edit.

Collection also bounds private state across all checkouts with
`gc.incremental_max_size`, which defaults to 5% of the cache disk, between
10 GiB and 100 GiB. `gc.incremental_max_age` defaults to 30 days. Collection
removes the least-recently-used checkouts first, but keeps the most recent one
even when it alone exceeds the aggregate budget. To change either limit:

```sh
mbx settings set gc.incremental_max_size 30GiB   # or none
mbx settings set gc.incremental_max_age 14d      # or none
```

When set, `gc.max_total_size` is one combined budget for the action store,
[managed target directories](/managed-targets), learned incremental state, and
generated source trees. It replaces the default aggregate incremental cap, and
the per-crate limit defaults to the combined budget. Explicit incremental
limits still apply. See [Single cache budget](/configuration#single-cache-budget).

## Eager incremental reuse

Eager incremental reuse keeps private incremental state for workspace crates
from their first compilation, before any source edit. The `eager_incremental`
setting defaults to `false` and works locally and in CI. Turn it on for builds
where you expect more source edits and have disk space to retain intermediate
compiler work, such as on developer machines and persistent CI runners.

Enable it for one build:

```sh
MBX_EAGER_INCREMENTAL=1 mbx build --locked
```

Enable it globally on your machine:

```sh
mbx settings set eager_incremental true
```

Or commit the setting in `.mbx.toml`:

```toml
eager_incremental = true
```

Set `MBX_EAGER_INCREMENTAL=0` to override a checked-in setting and return to
the default policy.

mbx manages the private state itself, so eager incremental reuse works with
`CARGO_INCREMENTAL=0`, and a later build can reuse the state even after Cargo's
target directory is removed. Dependencies outside the workspace continue to use
the shared cache. Artifacts linking private workspace outputs also stay private.

Measure the initial cost and later edits on your workload before you enable it:

- The first build can be slower, and the state can occupy several GiB.
- Workspace crates use private state whenever mbx can prepare it, even when an
  unchanged compilation could otherwise restore complete shared outputs. Extra
  disk space makes retention practical but does not guarantee a faster build.
- Fresh CI runners with no retained state pay the initial cost without the
  next-build benefit.

The state uses the same [storage limits and cleanup](#bound-the-storage) as
learned incremental reuse. For useful reuse, keep the same checkout and target
paths, toolchain, and build flags. Eager incremental reuse also interacts with
other features:

- mbx keeps private state out of remote caches and cache exports.
- Verification disables incremental compilation for the units it selects, but
  units that consume private artifacts still stay out of the shared cache.
- If mbx cannot prepare private state, a unit with no private inputs can fall
  back to ordinary shared caching.
- Unsupported compiler invocations and existing unknown compiler wrappers keep
  their usual bypass behavior.

## Cargo incremental mode

By default, mbx sets `CARGO_INCREMENTAL=0` and uses learned incremental reuse
for changing crates. Set `MBX_INCREMENTAL=1` to let Cargo manage incremental
compilation of workspace members instead:

```sh
MBX_INCREMENTAL=1 mbx build
```

mbx then stops forcing `CARGO_INCREMENTAL=0` but does not set it, so Cargo's
profile settings decide. By default, Cargo compiles local packages
incrementally in dev profiles and not in release. A `CARGO_INCREMENTAL=0`
already in your environment still wins, and mbx warns about it.

Cargo's incremental artifacts remain checkout-specific and bypass the shared
cache, and their dependents may also miss. Use this mode only when you want
that tradeoff. It has no effect in CI or when eager incremental reuse is
enabled; see [Overrides and CI](#overrides-and-ci).

## Overrides and CI

- `MBX_LEARNED_INCREMENTAL=0` disables learned incremental reuse.
- `MBX_INCREMENTAL=1` hands incremental control to Cargo unless eager
  incremental reuse is enabled.
- `MBX_EAGER_INCREMENTAL=1` takes precedence over both policies and seeds
  private workspace state from the first compilation, locally or in CI.
- `MBX_VERIFY=1` disables learned and eager incremental reuse for the build
  being verified.
- When `CI` is `1`, `true`, or `yes` (case-insensitive), mbx always turns off
  Cargo incremental mode and learned incremental reuse. `MBX_INCREMENTAL=1`,
  `MBX_LEARNED_INCREMENTAL=1`, and the `incremental` and `learned_incremental`
  settings do not re-enable them. Eager incremental reuse is the only
  incremental mode available in CI, and only when explicitly enabled.

## Incremental counts in the build summary

The build summary counts compilations that kept private incremental state as
`incremental`: mbx ran the compiler for them but did not store the results.
This count overlaps `misses` and `not looked up`, and it is separate from the
`incremental` bypass reason that
[Cargo incremental mode](#cargo-incremental-mode) produces.
[Incremental](/cache-results#incremental) in Cache results explains how to read
these counts, and the [JSON build report](/cache-results#json-build-report)
records the count as `incremental_compilations`.

The [local-edit benchmark](/benchmarks#local-edit) reports both the initial
state-building cost and a later edit.
