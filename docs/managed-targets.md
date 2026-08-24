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

mbx records the checkout associated with each target view. Once that
checkout no longer exists, `mbx gc` can remove its target directory. Cached
compilations shared with a live checkout remain protected.

Managed target directories are collected when their checkout disappears; they
are not counted against `gc.max_size`. The size budget covers cached objects
and action results.

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
