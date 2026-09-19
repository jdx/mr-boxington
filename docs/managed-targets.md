---
description: Understand target placement, disk budgets, automatic collection, and cleanup commands.
---
# Managed target directories

Cargo normally writes build outputs to `<workspace>/target`. Deleting a
worktree deletes useful outputs, while abandoning a checkout leaves gigabytes
behind indefinitely.

Managed targets are enabled by default. For a checkout without an existing
`target/`, the first build is enough:

```sh
mbx build
```

mbx places the target directory under its cache root and leaves a symlink at
`target`, so familiar paths continue to work:

```text
target -> <cache root>/targets/v1/<checkout digest>
```

Cargo continues to report artifacts through the workspace's `target` path, so
debugger launch configurations do not capture the private managed path that
collection may later replace. In a Git checkout, mbx also adds the exact link
path to `.git/info/exclude` when necessary. A directory-only `target/` pattern
does not match a symlink; the local exclude keeps `git status` clean without
changing the project's `.gitignore`.

## When mbx leaves a target alone

mbx does not override an explicit target directory supplied by:

- `--target-dir`
- `CARGO_TARGET_DIR`
- Cargo's `build.target-dir` configuration

## Change target placement

Set `target.root` in your global configuration to place managed targets on
another local disk:

```toml
[target]
root = "/path/to/local/build-targets"
```

After any builds using the old target have finished, the next build can update
an mbx-owned `target` link to the new managed location:

- When the old directory can be renamed to the new location, mbx moves it and
  preserves its outputs.
- When a rename fails, including across filesystems, mbx creates the destination
  and removes the old directory. It does not copy the old outputs. Matching
  compilations can be restored from the shared cache; other work compiles again.

The old collection record is retired after relocation. The target budget
scales with the destination disk unless you set it explicitly.

## Adopt existing target directories {#existing-target-directories}

Use `mbx adopt` to bring existing `target/` directories under mbx management
without deleting their contents. mbx moves each directory under the managed
root and leaves a link at its original path. From then on, Cargo continues to
use `target/`, while mbx applies the same collection policy as it does to any
other managed target.

Adopt the current checkout, name specific checkouts, or search below one or
more directories:

```sh
mbx adopt                            # the current checkout
mbx adopt ~/src/project              # one checkout
mbx adopt --recursive ~/src          # checkouts anywhere below a directory
mbx adopt --recursive --dry-run ~/src
```

Use `--dry-run` to see which directories are eligible without moving them.
Each result includes the logical size of the directory, and runs over multiple
checkouts end with a total:

```text
adopted /home/me/src/project/target (2.4 GiB logical)
adopted /home/me/src/other/target (912.0 MiB logical)
adopted 2 target directories (3.3 GiB logical)
```

Recursive searches look for directories containing both `Cargo.toml` and a
real `target/`. They do not descend into hidden directories, `target/`
directories, or symbolic links.

### Adoption during a build

An interactive mbx build that finds an existing real `target/` offers the same
move:

```text
Use a managed target directory?
mbx can move /path/to/project/target under its managed root and leave a link in its place. The outputs are kept, and the directory is pruned after this checkout is deleted.
```

“Move target/” is selected by default. Declining leaves every output untouched
and continues the Cargo command normally. Non-interactive builds never prompt,
move, or remove a directory.

If the managed root is on another filesystem, mbx cannot rename the directory
into it. In that case, the build-time prompt retains the previous option to
remove the old outputs, with “Keep it” selected by default. The `mbx adopt`
command never deletes or copies a target directory.

### Eligibility and recovery

mbx leaves a checkout unchanged and explains why when:

- `--target-dir`, `CARGO_TARGET_DIR`, or Cargo's `build.target-dir` names the
  target directory;
- it is a workspace member, whose outputs live in the workspace root's
  `target/`;
- managed targets are turned off for it;
- its `target/` is on a different filesystem from the managed root, where a
  rename would require copying;
- `target/` is already a symbolic link.

Set `target.root` to a location on the same filesystem when you want to adopt
a directory that would otherwise be skipped. Before moving one, mbx takes its
Cargo build locks; if a build is still writing there, mbx refuses the move and
asks you to try again later. It then renames the directory into the managed
root before creating the link. If the link or collection record cannot be
created, mbx moves the directory back. When a user accepts the build-time
removal option, mbx deletes the old outputs only after their managed
replacement is ready.

Adoption preserves the files already in `target/`, but it does not make every
plain Cargo artifact immediately reusable by mbx. Cargo keys builds run
through mbx differently, so the first mbx build may compile artifacts that
were produced without mbx. A target directory previously built through mbx
remains fresh after adoption.

## Collection

mbx records the checkout associated with each target view. Collection runs
after a build, at most once an hour, and needs no configuration. It normally
runs in the background once the build has returned, so a walk of every managed
directory never holds up the build that happened to come due, and the next
build reports what it removed. If the background collector cannot be started,
the build collects in the foreground instead. A directory that a build claims
or is compiling in while collection runs is left alone until the next sweep.
A target directory is removed when any of these is true:

- Its checkout is gone. This happens regardless of the limits below.
- It has gone unused for `target.max_age`, 30 days by default. The next build
  in that checkout can restore matching cached outputs. Evicted or unsupported
  work must compile again.
- The managed directories together exceed `target.max_size`. The least
  recently used go first. The most recently used directory is never collected
  for being over budget; if the budget cannot be met without it, mbx says so
  and keeps it.

Cached compilations shared with a live checkout remain protected throughout.

### Budgets scale with the disk

All three budgets scale with the disk that holds the data. By default, targets,
learned incremental state, and the action store share the cache disk. A custom
`target.root` uses its own volume for the target budget:

| Budget | Default | Bounds |
| --- | --- | --- |
| `gc.max_size` (action store) | 5% of the disk | 5 GiB to 500 GiB |
| `target.max_size` (managed targets) | 10% of the disk | 10 GiB to 100 GiB |
| `gc.incremental_max_size` (learned incremental) | 5% of the disk | 10 GiB to 100 GiB |

Scaled budgets are rounded down to a whole 5 GiB. When the disk cannot be
measured, mbx uses 20 GiB, 30 GiB, and 20 GiB respectively. An explicit budget
overrides these defaults, and `mbx gc --dry-run` previews the effect of a policy
without deleting anything.

### Changing or disabling the limits

```toml
[target]
max_size = "60GiB"
max_age = "none"   # keep live checkouts' outputs indefinitely

[gc]
# Optional: one budget covering targets, learned incremental, and the action store.
max_total_size = "50GiB"
incremental_max_size = "20GiB"
incremental_max_age = "30d"
```

`"none"` turns off `target.max_size`, `target.max_age`,
`gc.incremental_max_size`, `gc.incremental_max_age`, or `gc.max_total_size`.
Invalid sizes and durations are errors, so a typo cannot disable collection.
`gc.max_size` does not accept `"none"`; the action store is always bounded. To
stop creating managed targets, see
[Disable managed targets](#disable-managed-targets).

## Inspect and clean up

| Command | Effect |
| --- | --- |
| `mbx cache stats` | Inspect the action store, managed targets, and learned incremental state |
| `mbx gc --dry-run` | Preview collection under the configured budgets |
| `mbx gc` | Collect eligible targets, cached objects, and learned incremental state |
| `mbx clean` | Remove this workspace's managed target, link, and learned incremental state |
| `mbx adopt [--recursive] [PATH]...` | Adopt existing `target/` directories without deleting their contents |
| `mbx cache remove /path/to/workspace` | Remove the target and incremental state, then forget that workspace's cache claims |

`mbx clean` also accepts a workspace path. It keeps shared cached objects and
the workspace's cache claims, so a later build can restore matching outputs.
`mbx cache remove` forgets those claims as well; objects used by other
workspaces remain available and normal garbage collection reclaims unneeded
objects.

Cargo's `cargo clean` follows Cargo's own target-directory behavior and does not
remove mbx's private incremental state.

## Disable managed targets

Set `MBX_TARGET_VIEWS=0`, or configure:

```toml
[target]
views = false
```

Turning placement off does not delete a target directory mbx already manages.
The existing `target` link continues to work, and collection can still reclaim
the directory after its checkout disappears.

Use the [cleanup commands](#inspect-and-clean-up) to remove existing managed
outputs immediately.

::: warning Windows
Creating the link requires Developer Mode or a privileged process on Windows.
If Windows cannot create it, mbx lets Cargo use its ordinary target directory.
:::

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
