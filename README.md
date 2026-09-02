<p align="center">
  <img src="docs/public/logo.svg" alt="Mr Boxington, a friendly cache box wearing a monocle and bow tie" width="220">
</p>

<h1 align="center">mr boxington</h1>

<p align="center">
  <strong>fix <code>target/</code></strong><br>
  Keep using cargo. Every build on the machine shares one self-pruning cache, and you can run multiple Cargo builds in parallel.
</p>

<p align="center">
  <a href="https://mr-boxington.jdx.dev">Documentation</a>
  ·
  <a href="https://github.com/jdx/mr-boxington/releases">Releases</a>
</p>

`mbx` puts a content-addressed rustc cache behind ordinary Cargo commands.
Cargo still resolves dependencies, plans builds, and links outputs. mbx
restores the compilations it has seen before. Run `mbx setup` once and keep
using the Cargo commands you already run.

```sh
cargo build                # cached by mbx
cargo test --all-features  # cached by mbx
cargo clippy --workspace   # cached by mbx
mbx tui                    # watch every build's cache activity live
mbx gc --dry-run           # preview what cleanup would reclaim
```

## Why mbx?

- Cache keys contain no checkout-specific paths, so building one worktree
  warms its siblings, and no two checkouts wait on the same Cargo target lock.
- Disk use is bounded without a chore. The cache prunes itself to a share of
  the disk, and a managed `target/` directory is deleted when its checkout is
  gone, unused for 30 days, or over budget.
- Several Cargo builds can run at once. They share one machine-wide CPU and
  memory budget, and a cold compilation already running in one build is
  compiled once and restored into the others.
- CI can be warmed safely. GitHub Actions cache can warm fork pull requests
  from a cache built on `main`, and pull requests never publish remote objects.
- The summary counts hits, misses, and the compilations mbx could not look up
  or bypassed on purpose, so a high hit rate cannot hide work that never
  entered the cache.

## Install

With [mise](https://mise.jdx.dev):

```sh
mise use --global --postinstall "mbx setup --yes" mr-boxington
```

> [!NOTE]
> Automatic Cargo wrapping requires mise 2026.8.16 or newer.

Open a new shell and check that `command -v cargo` resolves to mise's command
wrapper. `mbx setup` writes a `[wrappers.cargo]` entry and runs `mise reshim`.
Tools that skip mise activation, such as SSH commands and coding agents, need
`~/.local/share/mbx/bin` on `PATH` instead. Prefixing a command with `mbx`
always works.

With Cargo:

```sh
cargo install mbx --locked
mbx setup
```

Release archives for Linux, macOS, and Windows are on the
[releases page](https://github.com/jdx/mr-boxington/releases), each with a
`SHA256SUMS` file.

[See all installation options →](https://mr-boxington.jdx.dev/getting-started)

## Automatic pruning

Collection runs after a build, at most once an hour, and needs no
configuration. The first build prints the budgets it chose for this machine.
mbx keeps a running total of what collection has reclaimed and prints one line
about it after a build:

```text
mbx[savings]: 41.7 GiB of target/ had outlived its checkouts. it has been dealt with.
```

`savings = "plain"` drops the joke. Inspect or collect the store by hand with:

```sh
mbx cache stats
mbx gc --dry-run
mbx gc
mbx clean        # this workspace's managed target/ only
```

For a checkout without an existing `target/`, the first build places the
target directory under the cache root and leaves `target` as a symlink, so
familiar paths still work and a deleted checkout's outputs get collected. An
existing `target/` is only replaced if you say yes at the prompt.

[Learn about managed targets →](https://mr-boxington.jdx.dev/managed-targets)

## CI

For GitHub-hosted CI, [`jdx/mr-boxington-action`](https://github.com/jdx/mr-boxington-action)
installs mbx and uses GitHub Actions cache. Trusted environments can point the
same action at a compatible remote such as the self-hostable
[cache server](https://mr-boxington.jdx.dev/cache-server).

Parallel steps that run Clippy and tests together can each get their own
`CARGO_TARGET_DIR` and share one CPU and memory budget through mbx. mise's
lint job finished up to 45% sooner this way.

[Copy the parallel workflow →](https://mr-boxington.jdx.dev/github-action#parallel-cargo-steps)

## How it works

An `mbx` Cargo command starts an in-process cache agent, points Cargo at rustc
and rustdoc shims, and stops the agent when the build ends. There is no daemon.
The shims derive action keys, restore cached outputs when they can, and
otherwise run the real tool and publish a successful result.

Anything mbx cannot model exactly bypasses the cache. Native links are cached
on Linux, macOS, and Windows when mbx can put the linker and its system inputs
into the key, and the workspace crate you are editing is recompiled
incrementally with state that never enters the shared cache.

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

## Acknowledgements

mbx relies on [Cargo](https://github.com/rust-lang/cargo) and follows earlier
compiler-cache work in [sccache](https://github.com/mozilla/sccache) and
[kache](https://github.com/kunobi-ninja/kache), which directly inspired its
design. [Read the acknowledgements](https://mr-boxington.jdx.dev/acknowledgements).

## License

[MIT](LICENSE)
