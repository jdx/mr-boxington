<p align="center">
  <img src="docs/public/logo.svg" alt="Mr Boxington, a cache box wearing a monocle and bow tie" width="180">
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

`mbx` is a build cache for Rust projects. Cargo still resolves dependencies,
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

With mise 2026.9.2 or newer, the Rust option enables wrapping without an
`mbx setup` hook. Open a shell with mise activation or shims on `PATH`,
[check Cargo's path](https://mr-boxington.jdx.dev/setup#verify-plain-cargo),
and use Cargo normally:

```sh
cargo build
cargo test --workspace --all-features
cargo clippy --workspace --all-targets -- -D warnings
```

To try mbx without automatic wrapping, install it and run `mbx build` directly.
For coding agents and other non-interactive tools, use `mise exec -- cargo build`
or put mise's shims on their `PATH`. The
[setup guide](https://mr-boxington.jdx.dev/setup) covers desktop applications,
older mise versions, and standalone shims.

Verified release archives are available for Linux, macOS, and Windows.
[All installation options →](https://mr-boxington.jdx.dev/installation)

## What you get

- **Reuse across worktrees.** Equivalent compilations share cache keys even
  when checkout paths differ. Building one worktree warms the next.
- **Automatic cleanup.** The store has a disk budget. Managed targets are
  collected when their checkout disappears, they go unused, or they exceed
  their budget. Preview collection with `mbx gc --dry-run`.
- **Parallel builds with a shared budget.** Independent Cargo commands share
  CPU and memory permits and deduplicate identical compilations in flight.
  Give each command its own target directory to avoid Cargo's directory lock.
- **Faster local edits.** mbx keeps private incremental state for crates you
  are changing while sharing eligible work across the rest of the build.
- **CI reuse.** Use GitHub Actions cache, a compatible cache server, or an
  S3-compatible bucket. Pull request builds restore remote work without
  publishing new objects through mbx.
- **An explanation for each result.** Hits, misses, unavailable lookups, and
  bypasses are counted separately. `mbx explain --last` helps diagnose a build.

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

Install your chosen Rust toolchain before the cache action. The default backend
restores a pruned Cargo target and registry archive; pull requests are
restore-only. See the [GitHub Action guide](https://mr-boxington.jdx.dev/github-action)
for complete workflows, parallel builds, remote servers, and release policy.

## Inspect and maintain the cache

```sh
mbx doctor          # check tools, setup, and cache access
mbx tui             # watch builds across the machine
mbx stats           # report lifetime savings and workspace sharing
mbx explain --last  # explain the last recorded build
mbx cache stats     # inspect storage
mbx gc --dry-run    # preview collection
mbx clean           # remove this workspace's managed target
```

On a filesystem that supports reflinks, restored outputs share data blocks
with the store until modified. Elsewhere, mbx copies bytes. An existing real
`target/` is only replaced after you accept a prompt.
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

mbx builds on Cargo and was informed by sccache and kache, which directly
inspired its design. [Acknowledgements](https://mr-boxington.jdx.dev/acknowledgements).

## License

[MIT](LICENSE)
