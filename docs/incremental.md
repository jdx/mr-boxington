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
separately for each checkout. `cargo clean` does not remove this state.
Garbage collection removes it for deleted or expired checkouts.

### Bound the storage

`learned_incremental_max_size` limits each crate to `8GiB` by default. Once a
crate exceeds that limit, mbx warns and discards its state before the next
compilation. Set it in your global configuration:

```toml
learned_incremental_max_size = "12GiB"
```

The environment equivalent is `MBX_LEARNED_INCREMENTAL_MAX_SIZE`; `"none"`
disables the limit. rustc normally removes superseded sessions, so the budget
is a backstop. A large debug build can need several GiB. If the warning appears
on every edit, the limit may be too small to retain useful state: raising it
can prevent repeated full recompilations.

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

## Overrides and CI

- `MBX_LEARNED_INCREMENTAL=0` disables learned incremental reuse.
- `MBX_INCREMENTAL=1` supersedes it by handing incremental control to Cargo.
- `MBX_VERIFY=1` disables it for the build being verified.
- CI disables both incremental policies because a fresh runner has no earlier
  edit state to reuse.

Use [Cache results](/cache-results) to distinguish private incremental work
from ordinary misses. The [local-edit benchmark](/benchmarks#local-edit) reports
both the initial state-building cost and a later edit.
