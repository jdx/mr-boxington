<p align="center">
  <img src="docs/public/logo.svg" alt="Mr Boxington, a cardboard cache box with a monocle and a handlebar mustache" width="180">
</p>

<h1 align="center">mr boxington</h1>

<p align="center">
  <strong>A shared cache. A tidier <code>target/</code>.</strong><br>
  Reuse Cargo builds across worktrees, keep disk use in check, and run builds together.
</p>

<p align="center">
  <a href="https://mr-boxington.jdx.dev/getting-started">Get started</a> ·
  <a href="https://mr-boxington.jdx.dev/guide">Documentation</a> ·
  <a href="https://mr-boxington.jdx.dev/benchmarks">Benchmarks</a> ·
  <a href="https://github.com/jdx/mr-boxington/releases">Releases</a>
</p>

mbx is a build cache for Rust projects. Cargo still resolves dependencies,
plans builds, and runs your tools. mbx restores matching compiler outputs from
one shared store and compiles the rest. Each command starts its own cache
agent and stops it when the build ends; there is no daemon to manage.

## Get started

With [mise](https://mise.jdx.dev):

```sh
mise use --global --tool-option mr_boxington=true rust mr-boxington
```

Or with Cargo:

```sh
cargo install mbx --locked
mbx setup
```

With mise 2026.9.2 or newer, the `mr_boxington` Rust option wraps Cargo
without an `mbx setup` hook. Open a shell with mise activation or shims on
`PATH`, [check Cargo's path](https://mr-boxington.jdx.dev/setup#verify-plain-cargo),
and use Cargo normally:

```sh
cargo build
cargo test --workspace --all-features
cargo clippy --workspace --all-targets -- -D warnings
```

Interactive builds use [cargo-pretty](https://github.com/romancitodev/cargo-pretty)'s
display by romancitodev, extended with mbx cache information. Follow live and
completed crates, browse warnings and test failures, and see cache hits, misses,
bypasses, and estimated compiler time saved. The build bar doubles as a cache
breakdown: green for hits, yellow for misses, and grey for bypasses and
compilations not looked up.

Cargo remains in charge of `run`, tests, doctests, and configured runners. CI,
redirected output, and explicit output formats keep Cargo's normal output.
Set `MBX_DISPLAY=plain`, or run `mbx settings set display plain`, to turn the
display off.

<picture>
  <source media="(prefers-reduced-motion: reduce)" srcset="docs/public/screenshots/cargo-pretty.png">
  <img src="docs/public/screenshots/cargo-pretty.gif" alt="A real mixed-cache rebuild after a shared-source edit, with per-crate outcomes">
</picture>

[View the still image](docs/public/screenshots/cargo-pretty.png).

To try mbx without automatic wrapping, install it and run `mbx build` directly.
For coding agents and other non-interactive tools, use `mise exec -- cargo build`
or put mise's shims on their `PATH`. The
[setup guide](https://mr-boxington.jdx.dev/setup) covers desktop applications
and standalone setup with the Cargo shim. For mise older than 2026.9.2, see
[Older mise versions](https://mr-boxington.jdx.dev/installation#older-mise-versions).

Verified release archives are available for Linux, macOS, and Windows.
[All installation options →](https://mr-boxington.jdx.dev/installation)

## What you get

- **Reuse across worktrees.** Equivalent compilations share cache keys even
  when checkout paths differ. Building one worktree warms the next.
- **Automatic cleanup.** The store has a disk budget. Managed target
  directories are collected when their checkout disappears, they go unused, or
  they exceed their budget. Preview collection with `mbx gc --dry-run`.
- **Parallel builds with a shared budget.** Independent Cargo commands share
  CPU and memory permits and deduplicate identical compilations in flight.
  Give each build its own target directory to avoid Cargo's directory lock;
  `check` and `clippy` already get a
  [check lane](https://mr-boxington.jdx.dev/managed-targets#check-lanes) of
  their own.
- **Faster local edits.** mbx keeps private incremental state for crates you
  are changing while sharing eligible work across the rest of the build.
- **CI reuse.** Use the GitHub Actions cache, a compatible cache server, or an
  S3-compatible bucket. mbx writes to a cache server or bucket only from
  pushes to protected branches, so pull request builds only read from it.
- **An explanation for each result.** The build summary counts each kind of
  [cache result](https://mr-boxington.jdx.dev/cache-results) separately: hits,
  misses, bypasses, and compilations not looked up. `mbx explain --last`
  diagnoses this workspace's last recorded build.

A cold store needs a build to fill it. Unsupported invocations run normally
without caching, and restored debug information can retain the original
checkout's paths. See [how it works](https://mr-boxington.jdx.dev/how-it-works)
and the [caching limits](https://mr-boxington.jdx.dev/limits).

## Use it in GitHub Actions

```yaml
permissions:
  contents: read

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
      - uses: jdx/mr-boxington-action@v1
      - run: mbx test --workspace
```

Install your chosen Rust toolchain before the cache action. The action's
default `github` backend restores a pruned Cargo target directory and registry
from the GitHub Actions cache. It saves a new entry after a successful run for
a push to the default branch. Pull requests only restore, unless you set
`save-on-pull-request` to let same-repository pull requests save entries of
their own; pull requests from forks never save. See the
[GitHub Action guide](https://mr-boxington.jdx.dev/github-action) for complete
workflows, parallel builds, remote caches, and release policy.

## Inspect and maintain the cache

```sh
mbx doctor                   # check tools, setup, and cache access
mbx tui                      # watch builds using this cache
mbx stats                    # report lifetime savings and workspace sharing
mbx explain --last           # explain the last recorded build
mbx cache stats              # summarize the store, managed targets, and incremental state
mbx gc --dry-run             # preview collection
mbx clean                    # remove this workspace's managed target
mbx adopt --recursive ~/src  # adopt existing target directories without deleting outputs
```

On a filesystem that supports reflinks, restored outputs share data blocks
with the store until modified. Elsewhere, mbx hard links the read-only store
object into place by default. It copies bytes on Windows, when it cannot link,
or with `restore_hardlink = false`. See
[output restoration](https://mr-boxington.jdx.dev/how-it-works#output-restoration).

Run `mbx adopt` to turn existing `target/` directories into managed
targets without deleting their contents. A build outside CI does the same
for its own checkout.
[Understand managed targets →](https://mr-boxington.jdx.dev/managed-targets)

## Find your next step

| Task | Guide |
| --- | --- |
| Set up editors, watchers, and worktrees | [Local development](https://mr-boxington.jdx.dev/cookbook/local-development) |
| Change budgets or build policy | [Configuration](https://mr-boxington.jdx.dev/configuration) |
| Choose mold, Wild, or toolchain LLD | [Managed linkers](https://mr-boxington.jdx.dev/linkers) |
| Share work across CI runners | [Remote cache](https://mr-boxington.jdx.dev/remote-cache) |
| Cache make or CMake builds | [Standalone C and C++](https://mr-boxington.jdx.dev/standalone-builds) |
| Investigate an unexpected result | [Troubleshooting](https://mr-boxington.jdx.dev/troubleshooting) |
| Look up a command | [CLI reference](https://mr-boxington.jdx.dev/cli/) |

## Contribute

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup, documentation
checks, tests, and pull request conventions. Ask questions in
[Discussions](https://github.com/jdx/mr-boxington/discussions); report suspected
vulnerabilities through the private process in [SECURITY.md](SECURITY.md).

mbx builds on Cargo and on ideas from sccache and kache; kache directly
inspired its design. See
[Acknowledgements](https://mr-boxington.jdx.dev/acknowledgements).

## License

[MIT](LICENSE)
