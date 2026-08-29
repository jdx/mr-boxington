<p align="center">
  <img src="docs/public/logo.svg" alt="Mr Boxington, a friendly cache box wearing a monocle and bow tie" width="220">
</p>

<h1 align="center">mr boxington</h1>

<p align="center">
  <strong>fix <code>target/</code></strong><br>
  Put mbx in front of any cargo command. Every build on the machine shares one self-pruning cache, and you can run multiple Cargo builds in parallel.
</p>

<p align="center">
  <a href="https://mr-boxington.jdx.dev">Documentation</a>
  ·
  <a href="https://github.com/jdx/mr-boxington/releases">Releases</a>
</p>

`mbx` wraps ordinary Cargo commands with a content-addressed rustc cache. Cargo
still resolves dependencies, plans builds, and links outputs; mbx restores
supported compilations it has seen before. There is nothing to configure and
nothing to install into Cargo: put `mbx` in front of the command you already
run.

```sh
mbx build                  # cargo build, with caching
mbx test --all-features    # cargo test --all-features, with caching
mbx clippy --workspace     # cargo clippy --workspace, with caching
mbx tui                    # watch every build's cache activity live
mbx gc --dry-run           # preview what cleanup would reclaim
```

The first build prints what it set up — where the cache lives, the budget it
is pruned to, and when a `target/` directory becomes collectable.

## Why mbx?

- **Warm every worktree.** Cache keys contain no checkout-specific absolute
  paths, so building one checkout warms its siblings automatically without
  sharing a Cargo target lock.
- **Bounded disk, without a chore.** The action store prunes itself to a
  budget, and managed `target/` directories are deleted when their checkout is
  gone, unused for 30 days, or over their share of the disk. Both budgets
  scale with the disk rather than assuming every machine is the same size.
- **Warm CI safely.** GitHub Actions cache can warm fork pull requests from a
  cache built on `main`, while a self-hosted remote can serve trusted runners
  and teammates. Pull requests never publish remote objects.
- **Run multiple Cargo builds at the same time.** They share one machine-wide
  CPU and memory budget. Identical cold compilations already running are
  compiled once and restored into every other build.
- **See the whole result.** mbx reports hits, misses, actions it could not look
  up, and actions it deliberately bypassed. A high hit rate cannot hide work
  that never entered the cache. `mbx tui` shows the same outcomes as they
  happen, for every build on the machine at once.

## Install

With [mise](https://mise.jdx.dev):

```sh
mise use -g mr-boxington
```

With Cargo:

```sh
cargo install mbx --locked
```

Or install the latest Linux x86-64 release archive:

```sh
mkdir -p ~/.local/bin
archive=mbx-x86_64-unknown-linux-gnu.tar.gz
release=https://github.com/jdx/mr-boxington/releases/latest/download
curl -fsSLO "$release/$archive"
curl -fsSLO "$release/SHA256SUMS"
grep "  $archive$" SHA256SUMS | sha256sum --check --strict -
tar -xzf "$archive" -C ~/.local/bin
```

Use the corresponding `-musl` archive for a static Linux binary. Release
archives cover both libc variants on x86-64 and ARM64, plus Apple Silicon and
Windows x86-64 and ARM64.
Every release includes `SHA256SUMS`.

[See all installation options →](https://mr-boxington.jdx.dev/getting-started)

## Automatic pruning

Collection runs after a build, at most once an hour, and needs no
configuration. Both size budgets default to a share of the disk holding the
cache — 5% for the action store and 10% for managed `target/` directories,
each from its floor (5 GiB and 10 GiB) up to 100 GiB and rounded down to a
whole 5 GiB — and a managed directory is also collected once its checkout is
gone or it has sat unused for 30 days.

mbx keeps a running total of what that has been worth and reports one line of
it after a build:

```text
mbx[savings]: 41.7 GiB of target/ had outlived its checkouts. it has been dealt with.
```

The line is drawn from a pool, so it does not repeat itself. `savings =
"plain"` states the same facts without the joke, and `savings = "off"` keeps
the totals without printing anything (`MBX_SAVINGS` from the environment). Inspect or collect the store explicitly with:

```sh
mbx cache stats
mbx cache projects
mbx cache largest --limit 10
mbx cache verify
mbx cache remove /path/to/workspace
mbx gc
mbx gc --max-size 3GB
mbx gc --dry-run
```

For a checkout without an existing `target/`, managed target directories are
enabled automatically:

```sh
mbx build
```

mbx places the target directory under its cache root and leaves `target` as a
symlink, so familiar paths still work — and the outputs of a checkout that is
deleted get collected rather than stranded.

For an existing real `target/`, an interactive `mbx` asks before removing the
old outputs and replacing the directory with a managed link. The safe default
is to keep it. Non-interactive runs never remove it. Set `MBX_TARGET_VIEWS=0`
to disable managed placement and the prompt.

[Learn about managed targets →](https://mr-boxington.jdx.dev/managed-targets)

## Worktrees and parallel CI

An equivalent rustc action keys the same across checkout paths. One worktree's
dependency build can therefore warm another while every checkout keeps its own
target directory.

For GitHub-hosted CI, [`jdx/mr-boxington-action`](https://github.com/jdx/mr-boxington-action)
can install mbx and use GitHub Actions cache, saving only from the default
branch and restoring in pull requests. Trusted environments can switch the
same action to a compatible remote such as the self-hostable
[cache server](https://mr-boxington.jdx.dev/cache-server).

GitHub Actions' parallel steps can start independent Clippy or test
configurations together. Give each one a separate `CARGO_TARGET_DIR` and run
it through mbx: the commands share one CPU and memory budget instead of each
trying to fill the runner on its own. We saw mise's lint job finish up to 45%
sooner this way.

[Copy the parallel workflow →](https://mr-boxington.jdx.dev/github-action#parallel-cargo-steps)

## How it works

An `mbx` Cargo command starts an in-process cache agent, points Cargo at a rustc shim with
`RUSTC_WRAPPER`, and exits the agent with the build — there is no daemon. The shim
derives an action key, restores cached outputs when possible, or runs the real
compiler and publishes a successful result.

Anything mbx cannot model exactly bypasses the cache. A native link is cached
only when the linker itself can enter the key: host binaries and tests on Linux
and macOS, where mbx resolves the linker, startup objects, and libc, and
self-contained WebAssembly targets everywhere, whose linker and libc ship in
the Rust toolchain. Incremental compilations bypass the cache. Correctness
comes before hit rate.

[Read the architecture and limits →](https://mr-boxington.jdx.dev/how-it-works)

## Documentation

- [Get started](https://mr-boxington.jdx.dev/getting-started)
- [Configuration](https://mr-boxington.jdx.dev/configuration)
- [GitHub Action](https://mr-boxington.jdx.dev/github-action)
- [Benchmarks](https://mr-boxington.jdx.dev/benchmarks)
- [Remote cache](https://mr-boxington.jdx.dev/remote-cache)
- [Protocol compatibility](https://mr-boxington.jdx.dev/protocol-compatibility)
- [Cache results](https://mr-boxington.jdx.dev/cache-results)
- [Watching builds](https://mr-boxington.jdx.dev/tui)
- [CLI reference](https://mr-boxington.jdx.dev/cli)
- [Current limits](https://mr-boxington.jdx.dev/limits)

> [!NOTE]
> mbx relies on [Cargo](https://github.com/rust-lang/cargo) and follows earlier
> compiler-cache work in [sccache](https://github.com/mozilla/sccache) and
> [kache](https://github.com/kunobi-ninja/kache). kache directly inspired mbx
> and has the longer production track record.
> [Read the acknowledgements](https://mr-boxington.jdx.dev/acknowledgements).

## License

[MIT](LICENSE)
