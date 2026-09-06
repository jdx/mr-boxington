---
description: Replace rust-cache or sccache in a workflow and verify the resulting mbx cache behavior.
---
# Migrate from rust-cache or sccache

Move one development or CI workflow at a time, verify activation, then compare
a representative warm build. Keep production release jobs subject to the
[release policy](/github-action#production-releases).

## From rust-cache or `actions/cache` over `target/`

Replace the existing cache action with `jdx/mr-boxington-action`. Both actions
can transport Cargo's target state, so using both for the same paths adds
duplicate restore/save work. See [archive caching](/compared#tarball-ci-caches).

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

The action's default entry carries Cargo's target directory and its download
caches under `~/.cargo`, so the workflow can retain dependency artifacts and Cargo downloads.
The `github-cache-mode: objects` payload omits the download caches; use a separate Cargo-download cache if you need those files too.

Production release jobs may use a trusted local cache, but should not restore
compiler outputs from a remote or an Actions archive. See
[Production releases](/github-action#production-releases).

## From sccache

Both tools wrap rustc through `RUSTC_WRAPPER`, so they cannot be combined for
the same build. With `RUSTC_WRAPPER` already set, mbx defers to the existing
wrapper and warns that the build is not cached. Migration is removal: take
sccache out of

- `RUSTC_WRAPPER` in CI environments and shell profiles,
- `build.rustc-wrapper` in `~/.cargo/config.toml`,
- workflow steps that install or configure it (`mozilla-actions/sccache-action`,
  `SCCACHE_GHA_ENABLED`, and similar).

Then check the result:

```sh
mbx doctor
```

<span id="the-first-build-measures-nothing"></span>

## Verify the migration

Run `mbx doctor`, then your usual build. A cold object store needs work to fill
it; a restored Cargo target may already be fresh and require no compilations.
Neither result proves cross-target cache reuse. Follow
[Measure cache reuse](/cache-results#measure-cache-reuse) for a controlled
comparison, and use `mbx explain --last` to understand any remaining bypasses.
