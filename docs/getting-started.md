# Get started

## Install

### mise

```sh
mise use -g github:jdx/mr-boxington
```

### Release archive

Linux x86-64:

```sh
(
set -e
mkdir -p ~/.local/bin
archive=mbx-x86_64-unknown-linux-musl.tar.gz
release=https://github.com/jdx/mr-boxington/releases/latest/download
curl -fsSLO "$release/$archive"
curl -fsSLO "$release/SHA256SUMS"
grep "  $archive$" SHA256SUMS | sha256sum --check --strict -
tar -xzf "$archive" -C ~/.local/bin
)
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
mbx build
mbx test --workspace --all-features
mbx clippy --workspace --all-targets
```

The command and its arguments are passed to Cargo unchanged. Cargo still owns
dependency resolution, feature unification, build planning, and linking. mbx
also forwards Cargo aliases and installed subcommands.

That is the whole setup. There is nothing to install into Cargo's
configuration and nothing to tune before the first build.

## The first build

The first build on a machine says what it arranged:

```text
mbx: first build on this machine -- here is the arrangement:
mbx:   compiled work is cached once in /home/you/.cache/mbx and shared with every checkout and worktree; the store sweeps itself back to 50.0 GiB
mbx:   this filesystem can reflink, so outputs land in target/ without copying -- many checkouts, one copy on disk
mbx:   target/ directories are managed and collected when their checkout disappears, they sit unused for 30 days, or they together outgrow 100.0 GiB
mbx:   nothing else to run; `mbx gc --dry-run` previews a cleanup and every cap is configurable
```

The budgets are a share of the disk holding the cache, so the numbers above
depend on the machine, and the reflink line appears only where a probe just
proved the filesystem supports it. See [managed target
directories](/managed-targets) for what collection removes and how to change
or disable each limit.

## Diagnose the installation

```sh
mbx doctor
```

Doctor checks the Cargo and rustc executables, cache write access, filesystem
reflink support, effective remote policy, and remote protocol connectivity.
Warnings describe optional features or fallbacks; failures make the command
exit unsuccessfully.

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
mbx gc --dry-run
mbx gc --max-size 20GiB
```

Automatic collection is on by default and scales its budgets to the disk; see
[managed target directories](/managed-targets).

Every inspection command also supports stable, versioned JSON for scripts and
CI integrations:

```sh
mbx doctor --json
mbx cache dir --json
mbx cache stats --json
mbx gc --json
```

## Next steps

- Tune local behavior in [Configuration](/configuration).
- Warm pull requests with [GitHub Actions cache](/github-actions).
- Learn what enters a key in [How it works](/how-it-works).
