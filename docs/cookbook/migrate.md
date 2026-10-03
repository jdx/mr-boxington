---
description: Replace rust-cache or sccache in a workflow and verify the resulting mbx cache behavior.
---
# Migrate from rust-cache or sccache

Move one development or CI workflow at a time. Verify that mbx is active, then
compare a representative warm build. For production release jobs, follow
[Production releases](/github-action#production-releases).

## From rust-cache or `actions/cache` over `target/`

Replace the existing cache action with `jdx/mr-boxington-action`. Both actions
can transport Cargo's target state, so using both for the same paths duplicates
the restore and save work. See [CI caches](/compared#tarball-ci-caches).

Before:

```yaml
steps:
  - uses: actions/checkout@v7
  - uses: Swatinem/rust-cache@v2
  - run: cargo test --workspace
```

After:

```yaml
steps:
  - uses: actions/checkout@v7
  - uses: jdx/mr-boxington-action@v1
  - run: mbx test --workspace
```

The action's default `target` payload carries the `target/` directory of the
Cargo workspace named by the action's `working-directory` input, plus Cargo's
registry and git download caches under `$CARGO_HOME` (`~/.cargo` by default).
That lets the workflow retain dependency artifacts and Cargo downloads.
`working-directory` defaults to the job's working directory, normally the
checkout root. If the workspace is in a subdirectory, set `working-directory`
to it. Otherwise the action finds no reusable target state and saves nothing,
not even the downloads.

The action saves a new entry only after a successful run for a push to the
default branch; other runs restore without saving. These inputs let more runs
save:

- `save-on-pull-request` for same-repository pull requests
- `save-on-protected-branch` for pushes to protected non-default branches
- `save-on-workflow-dispatch` for `workflow_dispatch` runs (enable it only in
  trusted workflows)

Fork pull requests and pushes to unprotected non-default branches never save.
See [GitHub Actions cache](/github-action#github-actions-cache) for details.

The `objects` payload (`github-cache-mode: objects`) leaves out Cargo's
download caches. Cache those downloads separately if you need them.

## From sccache

Both tools wrap rustc through `RUSTC_WRAPPER`, so they cannot be combined for
the same build. When `RUSTC_WRAPPER` is already set, mbx defers to the existing
wrapper and does not cache the build. To migrate, remove sccache from every
place that configures it:

- `RUSTC_WRAPPER` or `RUSTC_WORKSPACE_WRAPPER` in CI environments and shell
  profiles
- `build.rustc-wrapper` or `build.rustc-workspace-wrapper` in Cargo
  configuration such as `~/.cargo/config.toml`
- Workflow steps and settings that install or configure sccache, such as
  `mozilla-actions/sccache-action` and `SCCACHE_GHA_ENABLED`

Then run a build. If `RUSTC_WRAPPER` is still set in the environment, mbx
prints a `RUSTC_WRAPPER is already set` warning. A leftover
`RUSTC_WORKSPACE_WRAPPER` other than Clippy's `clippy-driver` gets a similar
warning: mbx defers to it for workspace crates and does not cache those
compilations. mbx does not warn about a wrapper left in Cargo configuration,
and `mbx doctor` does not check for rustc wrappers.

<span id="the-first-build-measures-nothing"></span>

## Verify the migration

Run `mbx doctor`, then your usual build. If the store is still empty, mbx has
to compile to fill it. If a restored Cargo target directory is already fresh,
Cargo runs no compilations. Neither result proves that mbx reuses work across
target directories. Follow
[Measure cache reuse](/cache-results#measure-cache-reuse) for a controlled
comparison, and use `mbx explain --last` to understand any remaining misses. To
see why compilations bypassed, run the build through `mbx explain`, such as
`mbx explain build`.
