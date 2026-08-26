# Get started

## Install

### mise

```sh
mise use -g mr-boxington
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

Release archives are also available for Linux ARM64, Apple Silicon, and Windows
x86-64. See [GitHub Releases](https://github.com/jdx/mr-boxington/releases)
for downloads and `SHA256SUMS`.

### Cargo

```sh
cargo install mbx
```

## Supported platforms

Release binaries cover Linux x86-64 and ARM64 (static musl builds), macOS on
Apple Silicon, and Windows x86-64; other platforms with a Rust toolchain can
build from source with `cargo install mbx`. mbx wraps whichever Cargo and
rustc are active, including rustup-managed toolchains — `mbx doctor` reports
the pair it found.

Reflinked output restoration needs a filesystem with copy-on-write file
cloning: APFS on macOS, btrfs or XFS on Linux, ReFS (Dev Drive) on Windows.
mbx probes the actual cache and target locations rather than assuming from
the platform, and copies bytes where cloning is unavailable — caching still
works on ext4 or NTFS, it just spends the disk twice. On Windows, managed
target directories also need Developer Mode or a privileged process to create
the `target` link; see [managed target directories](/managed-targets).

## Run a build

Put `mbx` before cargo's subcommand:

```sh
mbx build
mbx test --workspace --all-features
mbx clippy --workspace --all-targets
```

The command and its arguments are passed to Cargo unchanged. Cargo still owns
dependency resolution, feature unification, build planning, and linking. mbx
also forwards Cargo aliases and installed subcommands. Nothing goes into
Cargo's configuration, and there is nothing to tune before the first build.

## The first build

The first build on a machine prints what it set up:

```text
mbx[setup]: first build on this machine
mbx[setup]:   cache is /home/you/.cache/mbx, shared by every checkout and worktree, pruned to 50.0 GiB
mbx[setup]:   this filesystem supports reflinks, so target/ shares disk with the cache instead of copying
mbx[setup]:   target/ is managed: deleted when its checkout is gone, unused for 30 days, or over 100.0 GiB total
mbx[setup]:   `mbx gc --dry-run` previews cleanup; every limit is configurable
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
mbx[cache]: 139 hits, 8 misses, 147 prefetched; 312.4 MiB downloaded, 0 B uploaded, 280.1 MiB stored locally
mbx[cache]: could not look up 4 compilations: no usable dep-info from an earlier build and no prediction to derive an action key from
mbx[cache]: bypassed 7 compilations: 5 unsupported-crate-type, 2 unsupported-search-path
```

These are three different outcomes: a miss means mbx looked up an action and
found nothing, “could not look up” means it had no key yet, and a bypass means
mbx deliberately declined to cache the action. See [Cache results](/cache-results).

## Inspect the store

```sh
mbx cache dir       # where the store lives
mbx cache stats     # size and contents of the store
mbx cache projects  # cache use attributed to recorded workspaces
mbx cache largest   # the largest objects and action results
mbx cache verify    # check local objects against their digests
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
