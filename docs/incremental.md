---
description: Understand how mbx combines shared compiler caching with private incremental state for local edits.
---
# Incremental builds

mbx automatically keeps private incremental state for crates you edit. Unchanged
work remains eligible for the shared cache. Leave the defaults in place for a
normal edit/build loop.

| Mode | What is reused | Scope |
| --- | --- | --- |
| Shared action cache | Complete matching compiler outputs | Across equivalent builds and checkouts |
| Learned incremental reuse | Intermediate work from earlier source edits | Private to one checkout |
| Eager incremental reuse | Private intermediate work seeded on the first compilation | One checkout, locally or in CI |
| Cargo incremental mode | Cargo-managed incremental state | Private to Cargo's target directory |

## Learned incremental reuse

An edited crate has new source content, so its old shared result cannot match.
On the first source edit to a workspace crate, mbx compiles it with private
incremental state. A crate outside the workspace switches after three
consecutive misses with changed sources. The build reports this work as
`incremental`; the result never enters the shared cache.

The trigger is a source change. An unchanged crate in a fresh target or a new
checkout still compiles and publishes normally when it misses the cache.
Dependents of a crate using private incremental state also keep private state:
their inputs contain an artifact other checkouts cannot share. That part of the
build can publish again once the edited crate settles. Later edits to a known
active crate go directly to its private state.

### Where state lives

State and its records live under `incremental/` in the mbx cache directory,
separately for each checkout. `cargo clean` does not remove this state. `mbx
clean` and `mbx cache remove` remove it for the selected workspace. Garbage
collection removes it for deleted, expired, or least-recently-used checkouts;
state used by a running build is never removed.

### Bound the storage

`learned_incremental_max_size` limits each crate to `8GiB` by default. Once a
crate exceeds that limit, mbx warns and discards its state before the next
compilation. Raise it with:

```sh
mbx settings set learned_incremental_max_size 12GiB
```

The environment equivalent is `MBX_LEARNED_INCREMENTAL_MAX_SIZE`; `"none"`
disables the limit. rustc normally removes superseded sessions, so the budget
is a backstop. A large debug build can need several GiB. If the warning appears
on every edit, the limit may be too small to retain useful state: raising it
can prevent repeated full recompilations.

Garbage collection also bounds private state across all checkouts with
`gc.incremental_max_size`, which defaults to 5% of the cache disk between 10
GiB and 100 GiB. `gc.incremental_max_age` defaults to 30 days. The
least-recently-used checkouts are collected first, while mbx keeps the most
recent one even when it alone exceeds the aggregate budget:

```sh
mbx settings set gc.incremental_max_size 30GiB   # or none
mbx settings set gc.incremental_max_age 14d      # or none
```

`gc.max_total_size`, when set, covers the action store, managed targets, and
learned incremental state together.

## Cargo incremental mode

By default, mbx sets `CARGO_INCREMENTAL=0` and uses learned incremental reuse
for changing crates. `MBX_INCREMENTAL=1` stops it from forcing Cargo's setting
off locally:

```sh
MBX_INCREMENTAL=1 mbx build
```

This lets Cargo manage incremental workspace compilation. Those artifacts
remain checkout-specific and bypass the shared cache; their dependents may
also miss. Use it only when you want that tradeoff.

## Eager incremental reuse

`eager_incremental` is an opt-in mode for builds where you expect more source
edits and have disk space to retain intermediate compiler work. It is useful on
developer machines and persistent CI runners. Enable it for a build with:

```sh
MBX_EAGER_INCREMENTAL=1 mbx build --locked
```

To enable it globally on your machine:

```sh
mbx settings set eager_incremental true
```

Or commit the setting in `.mbx.toml`:

```toml
eager_incremental = true
```

The setting defaults to `false` and works locally and in CI. Unlike learned
incremental reuse, it seeds private state for workspace crates from their first
compilation, before any source edits. A later build can reuse that state even
after Cargo's target directory is removed. This works with
`CARGO_INCREMENTAL=0`: mbx manages the private state itself. Other dependencies
continue to use the shared action cache; artifacts that link private workspace
outputs also stay private.

The first build can be slower, and state can occupy several GiB. Workspace
crates use this private mode whenever state can be prepared, including when an
unchanged compilation could otherwise restore complete shared outputs. Extra
disk space makes retention practical, but does not guarantee a faster build.
Measure the initial cost and later edits on your workload before enabling it.
Fresh CI runners with no retained state pay the initial cost without the
next-build benefit.

State uses the same [storage limits and cleanup](#bound-the-storage) as learned
incremental reuse. Keep the same checkout and target paths, toolchain, and build
flags for useful reuse. Private state is not uploaded to remote caches or
included in cache exports. Verification disables incremental compilation for
selected units, but units consuming private artifacts still stay out of shared
publication. If private state cannot be prepared, a unit with no private inputs
can fall back to ordinary shared caching;
unsupported compiler invocations and existing unknown compiler wrappers keep
their usual bypass behavior. Set `MBX_EAGER_INCREMENTAL=0` to override a
checked-in setting and return to the default policy.

## Overrides and CI

- `MBX_LEARNED_INCREMENTAL=0` disables learned incremental reuse.
- `MBX_INCREMENTAL=1` hands incremental control to Cargo unless eager reuse is enabled.
- `MBX_EAGER_INCREMENTAL=1` takes precedence over both policies and seeds private
  workspace state from the first compilation, locally or in CI.
- `MBX_VERIFY=1` disables learned and eager reuse for the build being verified.
- CI disables Cargo incremental mode and learned incremental reuse by default.
  Eager reuse is available when explicitly enabled.

Use [Cache results](/cache-results) to distinguish private incremental work
from ordinary misses. The [local-edit benchmark](/benchmarks#local-edit) reports
both the initial state-building cost and a later edit.
