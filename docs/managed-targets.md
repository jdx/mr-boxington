---
description: Understand target placement, disk budgets, automatic collection, and cleanup commands.
---
# Managed target directories

Cargo normally writes build outputs to `<workspace>/target`. Deleting a
worktree deletes useful outputs, while abandoning a checkout leaves gigabytes
behind indefinitely.

Managed target directories are on by default. The first build creates one:

```sh
mbx build
```

mbx places the target directory in its cache directory and leaves a symlink at
`target`, so familiar paths keep working:

```text
target -> <cache_dir>/targets/v1/<checkout digest>
```

If a checkout already has a real `target/`, its first mbx build outside CI
moves it there and keeps its outputs. See
[Adoption during a build](#adoption-during-a-build).

Cargo keeps reporting artifacts through the workspace's `target` path, so
debugger launch configurations do not capture the managed target's own path,
which collection may later replace. In a Git checkout, mbx also adds the exact
link path to `.git/info/exclude` when necessary. A directory-only `target/`
pattern does not match a symlink; the local exclude keeps `git status` clean
without changing the project's `.gitignore`.

::: warning Windows
Creating the link requires Developer Mode or a privileged process on Windows.
If Windows cannot create it, mbx lets Cargo use its ordinary target directory.
:::

## When mbx leaves a target alone

mbx does not override a target directory you set with `--target-dir`,
`CARGO_TARGET_DIR`, or Cargo's `build.target-dir` configuration. For the cases
where mbx leaves an existing `target/` in place, including every CI build, see
[Adoption during a build](#adoption-during-a-build) and
[Eligibility and recovery](#eligibility-and-recovery).

## Checks run beside builds {#check-lanes}

Cargo locks a target directory while it compiles, so a `cargo clippy` started
next to a `cargo build` waits for the build to finish. mbx gives `check` and
`clippy` a **check lane**: a directory of their own inside the managed target.
A `clippy` and a build then run at the same time, with nothing to configure:

```sh
mbx build &
mbx clippy --workspace --all-targets -- -D warnings
```

```text
target/            (linked to the managed target)
├── debug/         build, test, run
└── check/debug/   check, clippy
```

Aliases count: `mbx c` and an alias such as `chk = "check"` get the check lane
too. Builds keep writing to `target/debug`, so binaries stay where they always
were. Both commands share one compile budget and one cache, so the second
starts with whatever the first has already stored. The check lane is part of
the managed target: mbx collects, moves, and removes it with the rest.

The first `check` or `clippy` in a checkout after upgrading compiles into the
check lane rather than reusing `target/debug`. The cache restores most of that
work. Proc macros and build scripts, which Cargo compiles for both, exist once
in each directory.

mbx does not give `check` and `clippy` a check lane when:

- `--target-dir`, `CARGO_TARGET_DIR`, or `build.target-dir` sets the target
  directory
- Cargo's build directory is set at all, with `build.build-dir`,
  `CARGO_BUILD_BUILD_DIR`, or `--config`, because that is where Cargo keeps
  the lock and a check lane cannot move it
- mbx is not placing the target, for example with `target.views = false` or
  while `target/` is still a real directory

Running in CI does not turn the check lane off. A CI checkout with no
`target/` gets a managed target and a check lane like any other, while one
that restored a real `target/` from its cache gets neither. See
[Adoption during a build](#adoption-during-a-build) for CI settings.

Set `target.lanes = false` (`MBX_TARGET_LANES=0`) to have `check` and `clippy`
write to the same profile directory as builds, such as `target/debug`. Two
builds, or a build and a test run, still share `target/debug` and wait for
each other; give them separate targets as described in
[Parallel builds](/scheduling).

## Inspect and clean up

| Command | Effect |
| --- | --- |
| `mbx cache stats` | Inspect the action store, managed targets, and learned incremental state |
| `mbx cache projects` | Show action-store and managed-target use for each recorded workspace |
| `mbx cache largest` | List the largest objects and action results in the store |
| `mbx gc --dry-run` | Preview collection under the configured budgets |
| `mbx gc` | Collect eligible targets, cached objects, and learned incremental state |
| `mbx clean` | Remove this workspace's managed target, link, and learned incremental state |
| `mbx adopt [--recursive] [PATH]...` | Adopt existing `target/` directories without deleting their contents |
| `mbx cache remove /path/to/workspace` | Remove the managed target and learned incremental state, then forget that workspace's cache claims |
| `mbx cache remove --interactive` | Choose recorded workspaces to remove from a list (needs a terminal) |

`mbx clean` and `mbx cache remove` keep the target directory, with a warning,
while a command run through mbx is using it. `mbx clean` also accepts a
workspace path. It keeps shared cache objects and the workspace's cache
claims, so a later build can restore matching outputs. `mbx cache remove`
forgets those claims as well. Objects other workspaces use remain available,
and collection later reclaims the ones nothing needs.

`cargo clean` follows Cargo's own target-directory behavior and does not
remove mbx's private incremental state.

## Change target placement

Set `target.root` to place managed targets on another local disk:

```sh
mbx settings set target.root /path/to/local/build-targets
```

mbx clones or hard links restored outputs only when the managed target root is
on the same filesystem as the cache directory. On another filesystem, it copies
every restored output; `mbx doctor` reports which method the root gets. See
[Output restoration](/how-it-works#output-restoration).

After any builds using the old target have finished, the next build in a
checkout points its mbx-owned `target` link into the new managed target root:

- When the old directory can be renamed to the new location, mbx moves it and
  preserves its outputs.
- When a rename fails, including across filesystems, mbx creates the destination
  and removes the old directory. It does not copy the old outputs. Matching
  compilations can be restored from the shared cache; other work compiles again.

mbx then retires the old collection record. The target budget scales with the
destination disk unless you set it explicitly.

Each checkout moves only when it builds again. Automatic collection, `mbx gc`,
`mbx cache stats`, `mbx clean`, and `mbx cache remove` look only under the
current `target.root`. A target left under the old root, including one whose
checkout is already deleted, is never counted against a budget or collected.
Before you change the root, run `mbx gc` to collect the targets of deleted
checkouts, and run `mbx clean` for checkouts you no longer build. If you already
changed it, run both commands with `MBX_TARGET_ROOT` set to the old root.

## Adopt existing target directories {#existing-target-directories}

Use `mbx adopt` to bring existing `target/` directories under mbx management
without deleting their contents. mbx moves each directory under the managed
target root and leaves a link at its original path. From then on, Cargo keeps
using `target/`, and mbx collects the directory under the same rules as any
other managed target.

Adopt the current checkout, name specific checkouts, or search below one or
more directories:

```sh
mbx adopt                             # the current checkout
mbx adopt ~/src/project               # one checkout
mbx adopt --recursive ~/src           # checkouts anywhere below a directory
mbx adopt --recursive --dry-run ~/src # preview without moving anything
```

Use `--dry-run` to see which directories are eligible without moving them.
mbx prints the [logical size](#collection-byte-counts) of each adopted
directory and the reason it left each skipped checkout alone. A run over
several checkouts ends with a total:

```text
adopted /home/me/src/project/target (2.4 GiB logical)
adopted /home/me/src/other/target (912.0 MiB logical)
adopted 2 target directories (3.3 GiB logical)
```

Recursive searches look for directories containing both `Cargo.toml` and a
real `target/`. They do not descend into hidden directories, `target/`
directories, or symbolic links.

### Adoption during a build

A build that finds an existing real `target/` moves it under the managed target
root the same way and keeps its outputs:

```text
mbx[cache]: moved the existing target/ directory under the managed root (2.4 GiB logical)
```

This happens with or without a terminal, so builds run by coding agents and
scripts adopt too. If Cargo is still writing to the directory, the build
continues in it and a later build moves it. Set `target.views = false` to keep
every `target/` where Cargo puts it.

CI builds leave an existing `target/` in place, because a CI cache step that
saves `target/` would save only the link. A CI checkout with no `target/`
still gets a managed target, so if your CI caches `target/` itself, set
`MBX_TARGET_VIEWS=0` there. The [GitHub Action](/github-action#github-actions-cache)
sets it for you with its default `github` backend and `target` payload.

If the managed target root is on another filesystem, mbx cannot rename the
directory into it. An interactive build then offers to remove the old outputs
instead, with “Keep it” selected by default. Non-interactive builds never
remove a directory, and the `mbx adopt` command never deletes or copies one.

### Eligibility and recovery

`mbx adopt` leaves a checkout unchanged and prints the reason when:

- `--target-dir`, `CARGO_TARGET_DIR`, or Cargo's `build.target-dir` names the
  target directory
- it is a workspace member, whose outputs live in the workspace root's
  `target/`
- managed targets are turned off for it
- its `target/` is on a different filesystem from the managed target root,
  where a rename would require copying
- `target/` is already a symbolic link

A build skips the same cases without printing a reason, except that an
interactive build offers to remove a `target/` on another filesystem; see
[Adoption during a build](#adoption-during-a-build).

To adopt a directory skipped for being on another filesystem, set
`target.root` to a location on that filesystem. If that filesystem does not
also hold the cache directory, mbx copies every restored output into the
managed target instead of cloning or hard linking it.

Before moving a directory, mbx takes its Cargo build locks. If a build is still
writing there, `mbx adopt` leaves that directory in place and reports
`Cargo is using <path>, so it was not adopted`. It goes on to the remaining
checkouts, then exits with a failure status; run it again once the build
finishes. Otherwise mbx renames the directory into the managed target root
before creating the link. If the link or collection record cannot be created,
mbx moves the directory back. When you accept the build-time removal option,
mbx deletes the old outputs only after their managed replacement is ready.

Adoption preserves the files already in `target/`, but it does not make every
plain Cargo artifact immediately reusable by mbx. Cargo keys builds run
through mbx differently, so the first mbx build may compile artifacts that
were produced without mbx. A target directory previously built through mbx
remains fresh after adoption.

## Start new checkouts from existing units

With Cargo 1.100 or later, the first build of a profile in a new checkout
copies the registry and Git dependency build units that another checkout's
managed target already built. Cargo treats the copies as fresh and skips those
units, so the build compiles or restores only what differs:

```text
mbx[target]: copied 302 registry build units from /home/me/src/project
```

mbx tries other managed targets, most recently used first, and copies from the
first one with units for this checkout's dependencies. Where the filesystem
records access times, it copies only the units that profile's latest build
read. The same profile below each target triple the other checkout built is
copied too, so a target chosen in Cargo configuration is covered. Copies are
reflinks where the filesystem supports them.

`check` and `clippy` in their [check lane](#check-lanes), and editor checks
under `target/rust-analyzer`, are seeded the same way from that directory in
the other checkout. The first `cargo check` in a new worktree then checks only
the workspace's own crates. mbx skips this step when:

- the profile already has a `build/` directory in this checkout
- the Cargo running this build is older than 1.100
- a build holds the Cargo lock of either profile
- no other checkout was built by Cargo 1.100 or later

Path dependencies and workspace members are never copied. Cargo trusts their
source modification times, so a copied unit could pass as fresh with another
checkout's contents.

[Collection](#unused-build-units) removes copied units this checkout does not
use. Where the filesystem does not record access times, as under `noatime`,
mbx cannot tell which units the other checkout last read. It then copies every
unit of each dependency, and the unused copies stay until the target directory
itself is collected.

Turn copying off for one command with `MBX_TARGET_SEED=0`, or from then on with:

```sh
mbx settings set target.seed false
```

## Collection

mbx records which checkout each managed target belongs to. Collection runs
after a build, at most once an hour, and needs no configuration. It normally
runs in the background after the build has returned, so walking every managed
target never holds up the build that came due. The next build reports what it
removed. If mbx cannot start the background collector, the build collects in
the foreground instead.

Set `gc.interval` to change how often collection runs. To turn automatic
collection off, including the collection described in
[When the disk runs low](#when-the-disk-runs-low), set `gc.auto = false`
(`MBX_GC_AUTO=0`). `mbx gc` still collects when you run it.

Every command you run through mbx, such as `mbx test`, `mbx run`, or
`mbx nextest run`, holds a shared lease on its target directory from start to
exit. Collection does not remove a directory while any command holds its lease.
`mbx gc` and the next build's report list such directories as kept, for
example `kept 1 target directories in use by running commands`. The lease ends
when the mbx process exits or is killed, and the usual rules then apply. The
lease does not protect:

- a binary you start directly from `target/` in a separate command, including
  test binaries built earlier with `--no-run`
- Cargo run through the `target` link without mbx, protected only while Cargo
  holds its build lock
- a child process that keeps running after only its mbx process was killed

Ctrl-C stops the whole process group, so it ends the command and its lease
together.

A target directory is removed when any of these is true:

- Its checkout is gone. This happens regardless of the limits below.
- It has gone unused for `target.max_age`, 30 days by default. The next build
  in that checkout can restore matching cached outputs. Evicted or unsupported
  work must compile again.
- The managed targets together exceed `target.max_size`. The least recently
  used go first. The most recently used directory is never collected for being
  over budget; if the budget cannot be met without it, mbx keeps it and warns.
- The disk is low on space. See [When the disk runs low](#when-the-disk-runs-low).

[Keep or evict specific checkouts](#keep-or-evict-specific-checkouts) changes
the order for checkouts you list.

`mbx gc` prints the `target.max_size` warning. A background sweep does not add
that warning to the next build's report. Instead it writes it to
`gc/v1/sweep.log` in the directory `mbx cache dir` prints, and each sweep starts
that log afresh.

Removing a target directory never removes cached compilations from the store.
When the store itself must shrink, compilations no live checkout uses go
first. A live checkout's compilations go only when that is not enough to bring
the store within its budget or relieve a short cache disk, and losing one
costs a recompile.

### Unused build units

A checkout in regular use keeps its target directory, but Cargo leaves the
previous build units behind whenever a lockfile update, feature change, or
toolchain update moves its builds on to new ones. Collection also removes
those units once no build has used them for `target.max_age` plus a day:

- With Cargo 1.100 or later, each unit is its own directory,
  `<profile>/build/<package>/<hash>/`, and each is removed on its own.
- Earlier Cargo versions spread units across `deps/`, `.fingerprint/`, and
  `build/`. mbx removes that layout as a whole once no build has used any of
  it, which happens after a checkout moves to Cargo 1.100.

Cargo reads the fingerprint of every unit a build uses, even when there is
nothing to compile, and mbx judges use by that read's access time. The extra
day covers Linux's default `relatime`, which refreshes an access time at most
daily. mbx skips this step when the filesystem does not record access times,
as under `noatime`, and leaves a target directory alone while a build holds
its Cargo lock.

The next build that needs a removed unit restores it from the cache or
compiles it again. Unused units go before the size budget is weighed, so they
are removed ahead of whole target directories. `target.max_age = "none"` keeps
them.

### Budgets scale with the disk

All three budgets scale with the disk that holds the data. By default, targets,
learned incremental state, and the action store share the cache disk. A custom
`target.root` uses its own volume for the target budget:

| Budget | Default | Bounds |
| --- | --- | --- |
| `gc.max_size` (action store) | 5% of the disk | 5 GiB to 500 GiB |
| `target.max_size` (managed targets) | 10% of the disk | 10 GiB to 100 GiB |
| `gc.incremental_max_size` (learned incremental state) | 5% of the disk | 10 GiB to 100 GiB |

Scaled budgets are rounded down to a whole 5 GiB. When the disk cannot be
measured, mbx uses 20 GiB, 30 GiB, and 20 GiB respectively. An explicit budget
overrides these defaults, and `mbx gc --dry-run` previews the effect of a policy
without deleting anything.

### Keep or evict specific checkouts

`target.keep` lists checkouts whose targets are never collected for age or
size, and whose unused build units are left alone. A kept target is still
removed when its checkout is gone. `target.evict_first` lists checkouts whose
targets go before any other when the managed targets are over budget:

```sh
mbx settings set target.keep '~/src/app'
mbx settings set target.evict_first .claude/worktrees
```

An absolute entry, or one starting with `~`, covers the checkouts at or under
that directory. A relative entry matches wherever its components appear
together in a checkout's path, so `.claude/worktrees` covers the worktrees
coding agents create under `.claude/worktrees` in any repository.

When both lists match a checkout, the entry that names more of its path wins.
With the example above, `~/src/app` is kept and its agent worktrees under
`~/src/app/.claude/worktrees` are evicted first. A tie keeps.

Evict-first targets are collected least recently used first, ahead of the rest.
The most recently used target directory is still spared, whichever list it is
on. Kept targets count toward `target.max_size`, so the other checkouts' targets
make room for them.

`mbx settings set` and the environment variables take comma-separated lists:

```sh
mbx settings set target.evict_first .claude/worktrees,scratch
export MBX_TARGET_EVICT_FIRST=.claude/worktrees,scratch
```

### When the disk runs low

The [budgets](#budgets-scale-with-the-disk) are shares of the disk's size, so
they do not shrink when something else fills the disk. `gc.min_free_size` sets
how much free space mbx tries to keep: by default 10% of the disk, from 5 GiB
to 50 GiB. The cache disk and a custom `target.root` volume are each measured
against their own size.

While a disk has less free space than that, collection runs as often as every
five minutes instead of once per `gc.interval`. It does not wait for a build to
finish: a compilation that misses the cache checks the disk when it is done
and starts collection in the background. Collection frees the shortfall from
private state and shared cache data regardless of the budgets:

1. Learned incremental state and generated source trees, least recently used
   first.
2. Managed target directories, least recently used first, with
   `target.evict_first` checkouts ahead of the rest and `target.keep` checkouts
   left alone.
3. Shared cache objects on the cache disk, after the private state and managed
   targets stop freeing bytes.

The most recently used target directory, and anything a running mbx command
is using, is kept as usual. The shared action store can go below `gc.max_size`
when the cache disk is short, because keeping the build machine usable takes
precedence over retaining every shared result. If collection cannot free
enough, mbx logs a warning and leaves the rest to you. The next build reports
what was removed and why:

```text
mbx[gc]: 3.1 GiB free on the disk holding /home/me/.cache/mbx, under the 25.0 GiB minimum; collecting private state and managed targets past their budgets, then shared cache objects if the cache disk is short
mbx[gc]: removed 4 target directories (18.2 GiB logical, 0 abandoned and 4 live); 6.0 GiB logical remain
mbx[gc]: evicted 12 shared cache objects and 3 action results below gc.max_size because the disk was under gc.min_free_size (2.0 GiB logical freed)
```

Restored outputs that share blocks with the store through reflinks or hard
links free less disk than their logical size. Collection therefore repeats each
step while the disk is still short, measuring the disk again before each round
rather than trusting the logical total. The number of rounds is bounded, so
unrelated files filling the disk cannot make a sweep run forever. Set
`gc.min_free_size` to a size such as `"20GiB"`, or to `"none"` to collect by
the budgets alone.

`mbx gc --dry-run` previews a single pass. It lowers the budgets for learned
incremental state, generated source trees, and managed targets by the
shortfall, but it cannot measure what each removal would free. A real run
measures the disk again as it goes, so it can differ from the preview:

- It may remove fewer target directories when earlier steps already freed
  enough.
- It may remove more learned incremental state, generated source trees, and
  target directories when removals free less disk than their logical size.
- It may evict shared cache objects below `gc.max_size`, which the preview
  leaves out.

### Change or disable the limits {#changing-or-disabling-the-limits}

For one setting covering all managed build data, use `gc.max_total_size` alone:

```toml
[gc]
max_total_size = "50GiB"
```

This replaces implicit component size caps with a shared logical-byte budget.
Explicit component caps still apply. See
[Single cache budget](/configuration#single-cache-budget) for allocation order,
protected state, and physical-space limitations. The following example instead
sets advanced overrides:

```toml
[target]
max_size = "60GiB"
max_age = "none"   # no age limit; the size budgets and low-disk collection still apply

[gc]
# optional combined budget for targets, learned incremental state, and the action store
max_total_size = "50GiB"
incremental_max_size = "20GiB"
incremental_max_age = "30d"
min_free_size = "20GiB"
```

`"none"` turns off `target.max_size`, `target.max_age`,
`gc.incremental_max_size`, `gc.incremental_max_age`, `gc.max_total_size`, or
`gc.min_free_size`. Invalid sizes and durations are errors, so a typo cannot
disable collection. `gc.max_size` does not accept `"none"`; the action store
is always bounded. To stop creating managed targets, see
[Disable managed targets](#disable-managed-targets).

### Collection is approximate

Object eviction prefers abandoned checkout data and then older access times.
Filesystems using `relatime` coarsen that order; `noatime` removes it. A poor
choice costs a recompile, not correctness.

The action-store budget covers action objects and results. Prediction data,
checkout records, and temporary downloads add overhead.

## Disable managed targets

Set `MBX_TARGET_VIEWS=0` for one command, or stop creating managed targets from
then on:

```sh
mbx settings set target.views false
```

Turning managed targets off does not delete a target directory mbx already
manages. The existing `target` link continues to work, and builds that mbx
runs through it still count as use. The directory stays under the usual
[collection rules](#collection): mbx removes it when its checkout is gone, when
it goes unused for `target.max_age`, when the managed targets together exceed
`target.max_size`, or when the disk runs low.

To remove existing managed targets now, use the commands in
[Inspect and clean up](#inspect-and-clean-up).

## Collection byte counts

Collection reports **logical bytes**: the sum of removed file lengths. The
`removed_bytes` and `remaining_bytes` fields in `mbx gc --json` use this measure;
`byte_accounting: "logical"` identifies it explicitly. The lifetime `savings`
object in `mbx stats --json` also carries that marker. Lifetime savings totals
and cleanup messages use the same measure. Existing `freed_*_bytes` fields in
the saved lifetime tally retain their names for compatibility.

Physical disk space released can differ. Reflinks and hard links may leave data
referenced by another file; sparse files may occupy fewer blocks than their
length. Filesystem snapshots and delayed allocation also affect reclamation.
These counters do not estimate physical space released.
