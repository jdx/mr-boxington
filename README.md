<p align="center">
  <img src="docs/public/logo.svg" alt="Mr Boxington, a friendly cache box wearing a monocle and bow tie" width="220">
</p>

<h1 align="center">mr boxington</h1>

<p align="center">
  <strong>Build it once.</strong><br>
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
mbx build build                  # cargo build, with caching
mbx build test --all-features    # cargo test --all-features, with caching
mbx build clippy --workspace     # cargo clippy --workspace, with caching
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
mkdir -p ~/.local/bin
curl -fsSL https://github.com/jdx/mr-boxington/releases/latest/download/mbx-x86_64-unknown-linux-musl.tar.gz \
  | tar -xzf - -C ~/.local/bin
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
mbx build build
```

mbx places the target directory under its cache root and leaves `target` as a
symlink, so familiar paths still work. Once the checkout is gone, `mbx gc` can
remove its target directory too.

An existing real `target/` is never replaced. Remove it to opt that checkout
into managed targets, or set `MBX_TARGET_VIEWS=0` to disable placement.

[Learn about managed targets →](https://mr-boxington.jdx.dev/managed-targets)

## Worktree and CI warming

An equivalent rustc action keys the same across checkout paths. One worktree's
dependency build can therefore warm another while every checkout keeps its own
target directory.

For GitHub-hosted CI, save `~/.cache/mbx` from `main` with `actions/cache`, then
restore it read-only in pull requests. This works for forks without exposing a
private cache host. Trusted environments can instead use a compatible remote
such as [`jdx/mbx-cache`](https://github.com/jdx/mbx-cache).

[Configure GitHub Actions →](https://mr-boxington.jdx.dev/github-actions)

## How it works

`mbx build` starts an in-process cache agent, points Cargo at a rustc shim with
`RUSTC_WRAPPER`, and exits the agent with the build—there is no daemon. The shim
derives an action key, restores cached outputs when possible, or runs the real
compiler and publishes a successful result.

Anything mbx cannot model exactly bypasses the cache. Linking is not cached,
incremental compilation is off by default, and plain `cargo build` does not use
mbx. Correctness comes before hit rate.

[Read the architecture and limits →](https://mr-boxington.jdx.dev/how-it-works)

## Documentation

- [Get started](https://mr-boxington.jdx.dev/getting-started)
- [Configuration](https://mr-boxington.jdx.dev/configuration)
- [GitHub Actions](https://mr-boxington.jdx.dev/github-actions)
- [Remote cache](https://mr-boxington.jdx.dev/remote-cache)
- [Cache results](https://mr-boxington.jdx.dev/cache-results)
- [CLI reference](https://mr-boxington.jdx.dev/cli)
- [Current limits](https://mr-boxington.jdx.dev/limits)

## License

[MIT](LICENSE)
