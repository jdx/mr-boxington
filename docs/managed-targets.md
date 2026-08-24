# Managed target directories

Cargo normally writes build outputs to `<workspace>/target`. Deleting a
worktree deletes useful outputs, while abandoning a checkout leaves gigabytes
behind indefinitely.

Managed targets are enabled by default. For a checkout without an existing
`target/`, the first build is enough:

```sh
mbx build build
```

mbx places the target directory under its cache root and leaves a symlink at
`target`, so familiar paths continue to work:

```text
target -> ~/.cache/mbx/targets/v1/<checkout digest>
```

## Collection

`mbx build` records the checkout associated with each target view. Once that
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

It also leaves an existing real `target/` directory alone. Remove that directory
if you want the next build to replace it with a managed link.

## Disable managed targets

Set `MBX_TARGET_VIEWS=0`, or configure:

```toml
[target]
views = false
```

Turning placement off does not delete a target directory mbx already manages.
The existing `target` link continues to work, and collection can still reclaim
the directory after its checkout disappears.

::: warning Windows
Creating the link requires Developer Mode or a privileged process on Windows.
If Windows cannot create it, mbx lets Cargo use its ordinary target directory.
:::
