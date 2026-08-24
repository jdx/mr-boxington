<p align="center">
  <img src="docs/public/logo.svg" alt="Mr Boxington, a friendly cache box wearing a monocle and bow tie" width="220">
</p>

<h1 align="center">mr boxington</h1>

<p align="center">
  <strong><code>target/</code>, fixed: shared and self-pruning.</strong><br>
  A Cargo wrapper that shares compiled work across worktrees and CI—and prunes build storage automatically.
</p>

<p align="center">
  <a href="https://mr-boxington.jdx.dev">Documentation</a>
  ·
  <a href="https://github.com/jdx/mr-boxington/releases">Releases</a>
  ·
  <a href="PLAN.md">Road to v1</a>
</p>

> [!WARNING]
> mr boxington is pre-1.0. The cache format and behavior may change without
> notice, and releases are not a stability promise.

`mbx` wraps ordinary Cargo commands with a content-addressed rustc cache. Cargo
still resolves dependencies, plans builds, and links outputs; mbx restores
supported compilations it has seen before.

```sh
mbx build                  # cargo build, with caching
mbx test --all-features    # cargo test --all-features, with caching
mbx clippy --workspace     # cargo clippy --workspace, with caching
mbx setup                  # cache future plain cargo commands locally
```

## Why mbx?

- **Warm every worktree.** Cache keys contain no checkout-specific absolute
  paths, so building one checkout warms its siblings automatically without
  sharing a Cargo target lock.
- **Prune automatically.** mbx sweeps its action store back to a size budget
  and, by default, removes managed target directories after their checkout
  disappears.
- **Warm CI safely.** GitHub Actions cache can warm fork pull requests from a
  cache built on `main`, while a self-hosted remote can serve trusted runners
  and teammates. Pull requests never publish remote objects.
- **See the whole result.** mbx reports hits, misses, actions it could not look
  up, and actions it deliberately bypassed. A high hit rate cannot hide work
  that never entered the cache.

## Install

With [mise](https://mise.jdx.dev):

```sh
mise use -g github:jdx/mr-boxington
```

With Cargo:

```sh
cargo install mbx
```

Or install the latest Linux x86-64 release archive:

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

Release archives also cover Linux ARM64, Apple Silicon, Intel macOS, and
Windows x86-64. Every release includes `SHA256SUMS`.

[See all installation options →](https://mr-boxington.jdx.dev/getting-started)

## Automatic pruning

The action store is swept automatically to a 20 GiB budget by default. Inspect
or collect it explicitly with:

```sh
mbx cache stats
mbx gc
mbx gc --max-size 3GB
```

For a checkout without an existing `target/`, managed target directories are
enabled automatically:

```sh
mbx build
```

mbx places the target directory under its cache root and leaves `target` as a
symlink, so familiar paths still work. Once the checkout is gone, `mbx gc` can
remove its target directory too.

For an existing real `target/`, an interactive `mbx` asks before removing the
old outputs and replacing the directory with a managed link. The safe default
is to keep it. Non-interactive runs never remove it. Set `MBX_TARGET_VIEWS=0`
to disable managed placement and the prompt.

[Learn about managed targets →](https://mr-boxington.jdx.dev/managed-targets)

## Plain Cargo commands

Run `mbx setup` once to install a persistent rustc wrapper and configure it in
Cargo's global configuration. Afterwards, ordinary `cargo build`, `cargo test`,
and other Cargo commands use the local action store without a daemon. Setup
leaves the configuration untouched when `build.rustc-wrapper` already names
another tool, such as sccache.

The persistent wrapper deliberately stays local-only: use `mbx <subcommand>`
when a build needs remote prefetch, session statistics, managed targets, or
automatic collection. Rerun `mbx setup` after upgrading mbx to refresh the
installed wrapper binary.

## Worktree and CI warming

An equivalent rustc action keys the same across checkout paths. One worktree's
dependency build can therefore warm another while every checkout keeps its own
target directory.

For GitHub-hosted CI, [`jdx/mr-boxington-action`](https://github.com/jdx/mr-boxington-action)
can install mbx and use GitHub Actions cache, saving only from the default
branch and restoring in pull requests. Trusted environments can switch the
same action to a compatible remote such as
[`jdx/mbx-cache`](https://github.com/jdx/mbx-cache).

[Configure GitHub Actions →](https://mr-boxington.jdx.dev/github-actions)

## How it works

An `mbx` Cargo command starts an in-process cache agent, points Cargo at a rustc shim with
`RUSTC_WRAPPER`, and exits the agent with the build—there is no daemon. The shim
derives an action key, restores cached outputs when possible, or runs the real
compiler and publishes a successful result.

Anything mbx cannot model exactly bypasses the cache. Native linking is not
cached; the intentionally narrow exception is `wasm32-unknown-unknown`, whose
default linker ships inside the Rust toolchain. Incremental compilations bypass
the cache. Correctness comes before hit rate.

[Read the architecture and limits →](https://mr-boxington.jdx.dev/how-it-works)

## Documentation

- [Get started](https://mr-boxington.jdx.dev/getting-started)
- [Configuration](https://mr-boxington.jdx.dev/configuration)
- [GitHub Actions](https://mr-boxington.jdx.dev/github-actions)
- [Remote cache](https://mr-boxington.jdx.dev/remote-cache)
- [Protocol compatibility](https://mr-boxington.jdx.dev/protocol-compatibility)
- [Cache results](https://mr-boxington.jdx.dev/cache-results)
- [CLI reference](https://mr-boxington.jdx.dev/cli)
- [Current limits](https://mr-boxington.jdx.dev/limits)

## License

[MIT](LICENSE)
