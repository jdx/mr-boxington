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

## Existing target directories

When an interactive mbx command finds an existing real `target/`, it offers to
remove the old outputs and replace the directory with a managed link:

```text
Use a managed target directory?
mbx can remove /path/to/project/target and replace it with a managed target that is pruned after this checkout is deleted.
```

“Keep it” is selected by default. Declining leaves every output untouched and
the Cargo command continues normally. Non-interactive runs never prompt or
remove the directory.

After acceptance, mbx temporarily moves the old directory aside. It removes
those outputs only after the managed link and its collection record both
succeed, then reports how much space the old outputs occupied. If placement
fails, mbx restores the original directory.

mbx does not offer removal for an explicitly configured target directory or a
symlink it does not own.

## Collection

mbx records the checkout associated with each target view. Collection runs
after a build, at most once an hour, and needs no configuration. It runs in
the background once the build has returned, so a walk of every managed
directory never holds up the build that happened to come due; the next build
reports what it removed. A target directory is removed when any of these is
true:

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
