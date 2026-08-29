# Migrate from rust-cache or sccache

Both migrations end in the same place: `mbx` in front of the Cargo command and
nothing else standing between Cargo and rustc. What has to be removed differs.

## From rust-cache or `actions/cache` over `target/`

A tarball cache and mbx solve the same problem, so replace the step rather
than stacking them — an archived `target/` restored over a managed one is
exactly the stale, ever-growing entry mbx exists to replace (see
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

One thing the swap gives up: rust-cache also cached Cargo's download caches
under `~/.cargo`, and the plain action does not, so each run re-fetches the
registry. To keep those cached too, use the
[manual GitHub cache setup](/github-action#manual-github-cache-setup), which
shares one entry between Cargo's download caches and the mbx store.

Leave release jobs out of the migration entirely: a production release should
not restore any compiler cache, mbx's or the one being removed. See the
[release warning](/github-action#s3-compatible-bucket).

## From sccache

Both tools wrap rustc through `RUSTC_WRAPPER`, so they cannot be combined for
the same build. mbx does not fight for the seat: with `RUSTC_WRAPPER` already
set it defers to the existing wrapper and warns that the build is not cached.
Migration is therefore removal — take sccache out of:

- `RUSTC_WRAPPER` in CI environments and shell profiles,
- `build.rustc-wrapper` in `~/.cargo/config.toml`,
- workflow steps that install or configure it (`mozilla-actions/sccache-action`,
  `SCCACHE_GHA_ENABLED`, and similar).

Then check the result:

```sh
mbx doctor
```

The `setup` check reports `no plain-cargo wrapper installed; mbx wraps cargo
directly` once nothing else is configured, and warns
`Cargo uses another wrapper: sccache` while the old configuration is still in
place.

## The first build measures nothing

However you arrive, the first mbx build has an empty store: it restores
nothing, stores everything, and the summary's
[could not look up](/cache-results#could-not-look-up) line dominates. The
second build is the honest measurement, and
[`mbx explain`](/cache-results#bypass) says what any remaining gap is made of.
