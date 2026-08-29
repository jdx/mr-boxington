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

## Collection

mbx records the checkout associated with each target view. Collection runs
after a build, at most once an hour, and needs no configuration. A target
directory is removed when any of these is true:

- **Its checkout is gone.** Nothing can ask for those outputs again, so this
  happens regardless of the limits below.
- **It has gone unused for `target.max_age`** — 30 days by default. The next
  build in that checkout starts warm from the shared store rather than from
  scratch, so this costs a re-link rather than a rebuild.
- **The managed directories together exceed `target.max_size`**, in which case
  the least recently used go first. The most recently used directory is never
  collected for being over budget — it is the checkout you are working in, and
  deleting it would only make the next build recreate it. If the budget cannot
  be met without it, mbx says so and keeps it.

Cached compilations shared with a live checkout remain protected throughout.

### Budgets scale with the disk

Both budgets default to a share of the disk holding the cache, so a laptop and
a build server do not need the same configuration:

| Budget | Default | Bounds |
| --- | --- | --- |
| `gc.max_size` (action store) | 5% of the disk | 5 GiB to 100 GiB |
| `target.max_size` (managed targets) | 10% of the disk | 10 GiB to 100 GiB |

Scaled budgets are rounded down to a whole 5 GiB. When the disk cannot be
measured, mbx uses 20 GiB and 30 GiB respectively. Any value you set outright
wins, and `mbx gc --dry-run` previews the effect of a policy without deleting
anything.

### Changing or disabling the limits

```toml
[target]
max_size = "60GiB"
max_age = "none"   # keep live checkouts' outputs indefinitely

[gc]
# Optional: one budget covering managed targets and the action store together.
max_total_size = "50GiB"
```

`"none"` turns off `target.max_size`, `target.max_age`, or
`gc.max_total_size`. A value that is neither a size nor `"none"` is an error
rather than a silently ignored setting, so a typo cannot disable collection.
`gc.max_size` has no `"none"`: an unbounded action store is the problem
collection exists to prevent. `MBX_TARGET_VIEWS=0` opts out of managed target
directories altogether, and a directory that is still reached through an
existing `target` symlink keeps counting as in use, so turning placement off
does not schedule existing outputs for deletion.

Each budget is measured against the disk that actually holds it, so putting
`target.root` on a large scratch volume sizes the target budget from that
volume rather than from the cache disk.

## When mbx leaves a target alone

mbx does not override an explicit target directory supplied by:

- `--target-dir`
- `CARGO_TARGET_DIR`
- Cargo's `build.target-dir` configuration

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

## Disable managed targets

Set `MBX_TARGET_VIEWS=0`, or configure:

```toml
[target]
views = false
```

Turning placement off does not delete a target directory mbx already manages.
The existing `target` link continues to work, and collection can still reclaim
the directory after its checkout disappears.

To remove one workspace's managed target immediately and forget its cache
claims, run `mbx cache remove /path/to/workspace`. Shared objects stay available
to other workspaces and are reclaimed by normal garbage collection.

::: warning Windows
Creating the link requires Developer Mode or a privileged process on Windows.
If Windows cannot create it, mbx lets Cargo use its ordinary target directory.
:::
