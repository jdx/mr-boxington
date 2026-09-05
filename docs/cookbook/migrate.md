# Migrate from rust-cache or sccache

## From rust-cache or `actions/cache` over `target/`

A tarball cache and mbx solve the same problem, so replace the step instead of
stacking them. An archived `target/` restored over a managed one is the stale,
ever-growing entry mbx replaces (see
[tarball CI caches](/compared#tarball-ci-caches)).

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
caches under `~/.cargo`, so nothing rust-cache restored is lost in the swap.
The `github-cache-mode: objects` payload omits the download caches; pair it
with the [manual GitHub cache setup](/github-action#manual-github-cache-setup)
to keep them.

Leave release jobs out of the migration: a production release should not
restore any compiler cache, mbx's or the one being removed. See the
[release warning](/github-action#s3-compatible-bucket).

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

## The first build measures nothing

However you arrive, the first mbx build has an empty store: it restores
nothing, stores everything, and the summary's
[could not look up](/cache-results#could-not-look-up) count dominates. Compare
the second build, and
[`mbx explain`](/cache-results#bypass) says what any remaining gap is made of.
