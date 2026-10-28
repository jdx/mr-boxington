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

mbx does not add incremental state to compiles that use LTO, or to compiles
optimized at opt-level 2 or higher (including `s` and `z`). Above opt-level 0,
rustc runs local ThinLTO across a crate's codegen units, and incremental ThinLTO
can leave an rlib whose units do not link. Opt-level 1 stays eligible because
Cargo itself compiles opt-level 1 dev profiles incrementally. In a Cargo
profile, `lto = "off"` or `codegen-units = 1` turns local ThinLTO off, so those
compiles stay eligible at any opt-level. `lto = false`, Cargo's default, does
not turn it off.

The trigger is a source change, or a changed value of an environment variable
the crate reads with `env!` or `option_env!`, such as one its build script sets
with `cargo:rustc-env`. An unchanged crate in a fresh target or a new checkout
still compiles and publishes normally when it misses the cache. Later source
edits to a known active workspace crate go directly to its private state,
without a cache lookup. A changed environment value is looked up first, so a
value the cache already holds, such as a commit hash another checkout
published, restores instead of compiling.

Dependents of a crate using private incremental state also keep private state,
because their inputs contain an artifact other checkouts cannot share. They can
publish again after the edited crate compiles once more without a source change.

### Start new checkouts warm

With rustc 1.98 or later, mbx seeds private incremental state from another
checkout by default. When a crate first needs private state, mbx copies the
most recently used checkout's state for the same crate and flags. Files use
copy-on-write filesystem clones where supported and plain copies otherwise,
so a later compile cannot change the donor's session. rustc validates that state
against the current sources, so the first edit in a new checkout can compile
incrementally too.

Upgrading from a version that seeded state with hard links discards learned
incremental state once: those links let one checkout's compiler overwrite
another checkout's state. A checkout another build is using at that moment is
discarded at a later build once it is idle. Stop builds running under older mbx
versions before upgrading, since an older version can still create shared files.

For these compilations only, mbx adds `-Zremap-cwd-prefix=.` and makes paths
under the checkout relative. Cargo's arguments and shared cache keys stay the
same. Debug info records `.` as the working directory for crates being edited,
and backtrace frames show paths such as `./src/lib.rs`. For sources passed by
absolute path, `file!()` and panic locations also change to that relative path.
Build-script compilations are not affected. Debuggers started from
the checkout can find those sources. When starting a debugger elsewhere, map
that directory to the checkout:

```text
# gdb
set substitute-path . /absolute/path/to/checkout
# lldb
settings set target.source-map . /absolute/path/to/checkout
```

On stable and beta compilers, mbx sets `RUSTC_BOOTSTRAP=1` only on the compiler
processes using this private state. It appends `-Zallow-features=` so that
`#![feature]` declarations remain rejected. Nightly and dev compilers need
neither addition. If `RUSTC_BOOTSTRAP` is already set to any value, mbx leaves
that choice in place and does not add the remap flag. On stable and beta it
also skips injection when the invocation has any `-Z` flag. If a crate reads
`RUSTC_BOOTSTRAP` with `env!()` or `option_env!()`, mbx retries without injection
and remembers that choice for the unit. It checks that the compiler accepts the
flag before adding it.

To turn off automatic seeding, run:

```sh
mbx settings set seed_incremental false
# Or for one build:
MBX_SEED_INCREMENTAL=0 mbx build
```

A user-supplied `-Zremap-cwd-prefix` still enables seeding on rustc 1.98 or
later, even with `seed_incremental` off. mbx keeps its value and does not add a
second flag or change bootstrap. Earlier compilers keep checkout-specific
state and do not seed it.

### Where state lives

mbx keeps the state and its records under `incremental/` in the cache
directory, separately for each checkout. `cargo clean` does not remove this
state. `mbx clean` and `mbx cache remove` remove it for the selected workspace.
[Collection](/managed-targets#collection) collects individual compilation units
across all checkouts. A deleted checkout can still seed a new checkout until
its units expire or the budget needs their space. Each profile, target kind,
feature set, or dependency identity can have its own unit. A unit's last use
is the newest of its last-use stamp, its newest rustc session directory, and,
when it has no sessions, the unit directory's modification time. The stamp is
a `<unit>.use` file beside the unit, whose modification time is refreshed at
most once a minute when a build restores the unit from the cache, so a unit
served by the cache on every build does not look idle. While its state is over
the backstop described below, the stamp records the budget it was rejected
under, and cache hits stop refreshing it until a compilation accepts the state
again or the budget changes. Building another unit in the same checkout
does not refresh old units. Seeding takes state from the checkout whose unit
was used last.

A checkout a build is using keeps every unit directory. Collection removes
only the unit's finished rustc sessions there, each under an exclusive lock on
the session's own lock file, taken without waiting, the way rustc's own
session cleanup does. A compiler starting from a session, or a seed copying
it, holds that lock, and the session stays. Sessions still being written,
crate directories, churn records, stamps, and seeding staging directories are
left alone. A compiler that starts while its unit's newest session is being
removed finds no session and compiles that unit from scratch once. Under the
size budget, units of such a checkout used within the last hour are kept,
unless they have expired, their state exceeds half the budget, or the disk is
short of `gc.min_free_size`. In checkouts no build is using, held rustc
session locks protect a whole unit, including donor leases during seeding.
New sessions, lock files, and seeding staging directories receive a 60-second
grace period; stale staging directories can be collected like other units.
A preview (`mbx gc --dry-run`) takes no rustc lock, so it can count a session a
real run keeps.

### Storage and collection

`gc.incremental_max_size` bounds private state across all checkouts. It
defaults to 5% of the cache disk, between 10 GiB and 100 GiB.
`gc.incremental_max_age` defaults to 30 days. Collection removes expired units
first, then remaining units least recently used first until the budget is met.
Seeded copies have separate inodes and count separately toward this logical
budget, even when filesystem clones share their data blocks.
Shared hard-linked files count once across the whole sweep and in
`mbx cache stats`. A collection credits a file's bytes as freed only when it
removes the file's last link, whether it removes a whole unit or a session:
rustc links the files a new session reuses from the one before it, and a
compile can link a file after the sweep listed it. Counts reflect each
sweep's snapshot, so links created during a sweep are counted at the next one. Live checkouts retain their bookkeeping even when all their units are removed;
a deleted checkout loses its directory and bookkeeping after its last unit
is collected.
To change either limit:

```sh
mbx settings set gc.incremental_max_size 30GiB   # or none
mbx settings set gc.incremental_max_age 14d      # or none
```

When set, `gc.max_total_size` is one combined budget for the action store,
[managed target directories](/managed-targets), learned incremental state, and
generated source trees. The aggregate incremental cap defaults to 10% of that
total, at least 10 GiB but never above the total. Explicit incremental limits
still apply. See [Single cache budget](/configuration#single-cache-budget).

The former `learned_incremental_max_size` setting is removed; existing config
files containing it warn and continue. As an internal backstop, a learned unit
whose existing state exceeds half the resolved aggregate incremental budget,
counting each hard-linked file once as collection does, takes the shared compilation path for that compile without seeding from a
donor. Otherwise, eligible units can seed before compiling with private
state. Existing oversized state remains for GC;
with no aggregate budget, this backstop is disabled.

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

The state uses the same [storage limits and cleanup](#storage-and-collection) as
learned incremental reuse. For useful reuse, keep the same checkout and target
paths, toolchain, and build flags. Eager incremental reuse follows the same
optimization and LTO limits as learned incremental reuse. It also interacts
with other features:

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
