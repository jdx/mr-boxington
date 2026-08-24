# Get started

## Install

### mise

```sh
mise use -g github:jdx/mr-boxington
```

### Release archive

Linux x86-64:

```sh
mkdir -p ~/.local/bin
curl -fsSL https://github.com/jdx/mr-boxington/releases/latest/download/mbx-x86_64-unknown-linux-musl.tar.gz \
  | tar -xzf - -C ~/.local/bin
```

Release archives are also available for Linux ARM64, Apple Silicon, Intel macOS,
and Windows x86-64. See [GitHub Releases](https://github.com/jdx/mr-boxington/releases)
for downloads and `SHA256SUMS`.

### Cargo

```sh
cargo install mbx
```

## Run a build

Put `mbx` before cargo's subcommand:

```sh
mbx build build
mbx build test --workspace --all-features
mbx build clippy --workspace --all-targets
```

Everything after `mbx build` is passed to cargo unchanged. Cargo still owns
dependency resolution, feature unification, build planning, and linking.

## Read the result

After a build that used the cache, mbx prints a summary to stderr:

```text
cache: 139 hits, 8 misses, 147 prefetched; 312.4 MiB downloaded, 0 B uploaded, 280.1 MiB stored locally
cache could not look up 4 compilations: no usable dep-info from an earlier build and no prediction to derive an action key from
cache bypassed 7 compilations: 5 unsupported-crate-type, 2 unsupported-search-path
```

These are three different outcomes: a miss means mbx looked up an action and
found nothing, “could not look up” means it had no key yet, and a bypass means
mbx deliberately declined to cache the action. See [Cache results](/cache-results).

## Inspect the store

```sh
mbx cache dir
mbx cache stats
mbx gc
mbx gc --max-size 20GiB
```

Automatic collection is enabled by default with a 20 GiB budget.

## Next steps

- Tune local behavior in [Configuration](/configuration).
- Warm pull requests with [GitHub Actions cache](/github-actions).
- Learn what enters a key in [How it works](/how-it-works).
