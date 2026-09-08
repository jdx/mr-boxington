---
description: Install mbx, run your first cached Cargo build, and learn what to expect from the result.
---
# Get started

mbx reuses compiler work across Rust workspaces, checkouts, and CI runs. Cargo
still resolves dependencies and decides what needs building. mbx restores
matching compilations and runs the compiler for everything else.

You need an existing Rust toolchain and a Cargo project. No cache server or
configuration file is required.

## Install

With [mise](https://mise.jdx.dev):

```sh
mise use --global --tool-option mr_boxington=true rust mr-boxington
```

This requires mise 2026.9.2 or newer. Drop `--global` for project-scoped
wrapping. See [Installation](/installation#mise) for older mise versions.

Or install from crates.io:

```sh
cargo install mbx --locked
```

Linux, macOS, and Windows binaries are also available. See
[Installation](/installation) for verified downloads and platform requirements.

## Run a build

From your Rust workspace:

```sh
mbx doctor
mbx build
```

Pass Cargo arguments as usual:

```sh
mbx test --workspace --all-features
mbx clippy --workspace --all-targets -- -D warnings
mbx +stable check --workspace
```

Cargo aliases, installed subcommands, and toolchain selection are preserved.
Use the same toolchain, features, and profile across builds to reuse the same
cached work.

## Keep using plain Cargo

The mise command above enables wrapping without running `mbx setup`. Use
`mise exec -- cargo build`, `mise run` tasks, or plain `cargo` with mise
activation or shims on `PATH`.

For standalone installations, run:

```sh
mbx setup
mbx setup --status
```

See [Cargo and editor setup](/setup) to verify Cargo's path, share project
configuration, configure rust-analyzer, and use mbx from desktop applications.

## The first build

A new local store has no compilations to restore. The first build fills it;
later builds and equivalent worktrees can reuse that work. If Cargo already
has up-to-date outputs in `target/`, it skips those compilations entirely.
That is normal and will not appear as mbx cache hits.

For a checkout without an existing `target/`, mbx creates a managed target and
leaves a `target` symlink in the workspace. An existing directory is replaced
only after you accept the interactive prompt. See
[Managed target directories](/managed-targets) for placement and cleanup.

The first build also prints the cache location and disk budgets chosen for your
machine. Automatic collection runs after builds, at most once an hour.

## Read the result

An illustrative summary looks like this:

```text
mbx[cache]: 139 hits, 8 misses, 4 not looked up, 147 prefetched, 7 bypassed; 312.4 MiB downloaded, 0 B uploaded, 280.1 MiB stored locally
```

| Result | What happened |
| --- | --- |
| Hit | mbx restored a matching compilation |
| Miss | mbx looked up a key, then compiled and stored a new result |
| Not looked up | mbx needed a first compilation to learn the inputs |
| Bypassed | The invocation ran without shared caching |

[Cache results](/cache-results) explains all counters and how to measure reuse.
Use `mbx explain --last` to inspect the last recorded build or `mbx tui` to
[watch builds live](/tui).

## Inspect the store

```sh
mbx cache stats     # size and contents
mbx gc --dry-run    # preview cleanup
```

A cache can always be rebuilt. Use the [management guide](/managed-targets)
to understand what each cleanup command removes.

## Next steps

| I want to… | Read |
| --- | --- |
| Set up an editor or watch loop | [Local development](/cookbook/local-development) |
| Cache a GitHub Actions job | [GitHub Action](/github-action) |
| Run independent builds together | [Parallel builds](/scheduling) |
| Tune disk, output, or build policy | [Configuration](/configuration) |
| Understand an unexpected result | [Troubleshooting](/troubleshooting) |

<details>
<summary>Looking for a section that moved?</summary>

<span id="verify-plain-cargo"></span>
[Verify plain Cargo](/setup#verify-plain-cargo) now lives in the setup guide.

<span id="supported-platforms"></span>
[Supported platforms](/installation#supported-platforms) now lives in Installation.

<span id="mise"></span>
<span id="cargo"></span>
<span id="release-archive"></span>
<span id="windows"></span>
[Installation methods and Windows notes](/installation) have moved there too.

<span id="choose-a-linker"></span>
[Choose a linker](/linkers) has its own guide.

<span id="run-multiple-cargo-builds-at-the-same-time"></span>
[Parallel builds](/scheduling) covers separate targets and shared budgets.

<span id="diagnose-the-installation"></span>
<span id="reporting-a-problem"></span>
[Installation checks](/troubleshooting#diagnose-the-installation) and
[Reporting a problem](/troubleshooting#reporting-a-problem) now live in Troubleshooting.

</details>
