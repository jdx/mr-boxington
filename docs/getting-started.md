# Get started

## Install

### mise

```sh
mise use --global --postinstall "mbx setup --yes" mr-boxington
```

With `--global`, mise activates mbx in its global configuration. Drop
`--global` to activate it only in the current project's configuration.

### Release archive

:::tabs
== Linux x86-64

```sh
mkdir -p ~/.local/bin
archive=mbx-x86_64-unknown-linux-gnu.tar.gz
release=https://github.com/jdx/mr-boxington/releases/latest/download
curl -fsSLO "$release/$archive"
curl -fsSLO "$release/SHA256SUMS"
grep "  $archive$" SHA256SUMS | sha256sum --check --strict -
tar -xzf "$archive" -C ~/.local/bin
```

== Linux ARM64

```sh
mkdir -p ~/.local/bin
archive=mbx-aarch64-unknown-linux-gnu.tar.gz
release=https://github.com/jdx/mr-boxington/releases/latest/download
curl -fsSLO "$release/$archive"
curl -fsSLO "$release/SHA256SUMS"
grep "  $archive$" SHA256SUMS | sha256sum --check --strict -
tar -xzf "$archive" -C ~/.local/bin
```

== macOS

```sh
mkdir -p ~/.local/bin
archive=mbx-aarch64-apple-darwin.tar.gz
release=https://github.com/jdx/mr-boxington/releases/latest/download
curl -fsSLO "$release/$archive"
curl -fsSLO "$release/SHA256SUMS"
grep "  $archive$" SHA256SUMS | shasum -a 256 --check --strict -
tar -xzf "$archive" -C ~/.local/bin
```

== Windows x86-64

```powershell
$archive = "mbx-x86_64-pc-windows-msvc.zip"
$release = "https://github.com/jdx/mr-boxington/releases/latest/download"
Invoke-WebRequest "$release/$archive" -OutFile $archive
Invoke-WebRequest "$release/SHA256SUMS" -OutFile SHA256SUMS
$expected = (Select-String -Path SHA256SUMS -Pattern $archive).Line.Split(" ")[0]
if ((Get-FileHash $archive -Algorithm SHA256).Hash -ne $expected.ToUpper()) {
  throw "checksum mismatch"
}
Expand-Archive $archive -DestinationPath "$env:LOCALAPPDATA\Programs\mbx"
```

Add `%LOCALAPPDATA%\Programs\mbx` to `PATH`.

== Windows ARM64

```powershell
$archive = "mbx-aarch64-pc-windows-msvc.zip"
$release = "https://github.com/jdx/mr-boxington/releases/latest/download"
Invoke-WebRequest "$release/$archive" -OutFile $archive
Invoke-WebRequest "$release/SHA256SUMS" -OutFile SHA256SUMS
$expected = (Select-String -Path SHA256SUMS -Pattern $archive).Line.Split(" ")[0]
if ((Get-FileHash $archive -Algorithm SHA256).Hash -ne $expected.ToUpper()) {
  throw "checksum mismatch"
}
Expand-Archive $archive -DestinationPath "$env:LOCALAPPDATA\Programs\mbx"
```

Add `%LOCALAPPDATA%\Programs\mbx` to `PATH`.

:::

Every release publishes its archives and `SHA256SUMS` on
[GitHub Releases](https://github.com/jdx/mr-boxington/releases).
Linux also has `-musl` archives for a static binary that does not depend on a
host glibc.

### Cargo

```sh
cargo install mbx --locked
mbx setup
```

`mbx setup` prompts for global mise activation, project-local mise activation,
or no activation. During `mise use --postinstall`, `mbx setup --yes` activates
the configuration named by `MISE_CONFIG_FILE`. Outside a postinstall hook,
`--yes` selects global activation. Without mise, setup prints the exact
shell-specific `PATH` change; it never edits a shell startup file.

After installing from a release archive, run `$HOME/.local/bin/mbx setup` on
Unix. On Windows, run
`& "$env:LOCALAPPDATA\Programs\mbx\mbx.exe" setup` in PowerShell.

## Supported platforms

Release binaries cover:

- Linux x86-64 and ARM64 (GNU and static musl builds)
- macOS on Apple Silicon
- Windows x86-64 and ARM64

Other platforms with a Rust toolchain can build from source with
`cargo install mbx --locked`. mbx wraps whichever Cargo and rustc are active,
including rustup-managed toolchains — `mbx doctor` reports the pair it found.

Reflinked output restoration needs a filesystem with copy-on-write file
cloning: APFS on macOS, btrfs or XFS on Linux, ReFS (Dev Drive) on Windows.
mbx probes the actual cache and target locations rather than assuming from
the platform, and copies bytes where cloning is unavailable — caching still
works on ext4 or NTFS, it just spends the disk twice.

### Windows

Windows is a supported release platform with a narrower caching tier. The
differences, which the pages they belong to also note in place:

- Reflinks need ReFS, which usually means a Dev Drive; on NTFS mbx copies
  bytes instead.
- The managed `target` link needs Developer Mode or a privileged process;
  where Windows refuses to create it, Cargo keeps its ordinary target
  directory. See [managed target directories](/managed-targets).
- rustc compilations and native host links are cached; native-link keys bind
  the selected MSVC/LLVM linker, Windows SDK, and CRT. See
  [limits](/limits#native-linking-is-cached-only-where-the-linker-can-be-described).
- MSVC C and C++ compiles from build scripts and `mbx exec` are cached through
  a conservative `cl.exe` adapter. See
  [limits](/limits#c-and-c-caching-covers-the-host-compiles-mbx-drives).

## Run a build

After setup, use Cargo normally:

```sh
cargo build
cargo test --workspace --all-features
cargo clippy --workspace --all-targets
```

The command and its arguments are passed to Cargo unchanged. Cargo still owns
dependency resolution, feature unification, build planning, and linking. Cargo
aliases, installed subcommands, and toolchain selection are preserved:

```sh
cargo +1.91 check --workspace
```

`MBX_DISABLE=1 cargo …` bypasses mbx for one invocation. You can also skip
setup and run `mbx +1.91 check` directly as a zero-config alternative; only
commands prefixed with `mbx` use caching in that mode. mbx's own `tui`, `cache`,
`gc`, `doctor`, and diagnostic commands stay under `mbx`.

## Run multiple Cargo builds at the same time

Any task runner can start multiple mbx commands at the same time. For example,
mise runs these two lint configurations together:

```toml
# mise.toml
[tasks."lint:default"]
run = "mbx clippy --workspace -- -D warnings"
env.CARGO_TARGET_DIR = "target/clippy-default"

[tasks."lint:all"]
run = "mbx clippy --workspace --all-features --all-targets -- -D warnings"
env.CARGO_TARGET_DIR = "target/clippy-all"
```

```sh
mise run lint:default ::: lint:all
```

Separate target directories keep Cargo's directory lock from serializing the
commands. mbx shares one machine-wide compiler pool and deduplicates identical
work in flight without further configuration — see
[how it works](/how-it-works#machine-wide-scheduling) for the mechanism and
the same shape [inside GitHub Actions](/github-action#parallel-cargo-steps).

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

```text
  ok  cargo        cargo 1.98.0 (797e8a9bc 2026-08-05)
  ok  rustc        rustc 1.98.0 (88d9e12ae 2026-08-18)
  ok  cache        /home/you/.cache/mbx is writable
  ok  config       50.0 GiB budget, automatic gc enabled, managed targets enabled at /home/you/.cache/mbx/targets
  ok  reflink      supported by the cache filesystem
  ok  setup        Cargo shim is active and current at /home/you/.local/share/mbx/bin/cargo
  ok  remote       not configured; using the local cache

0 failures, 0 warnings
```

Doctor checks that the installed Cargo shim matches the running mbx and is the
first `cargo` on `PATH`, along with the Cargo and rustc executables, cache write
access, filesystem reflink support, effective remote policy, and remote protocol
connectivity. Warnings describe setup problems, optional features, or fallbacks;
failures make the command exit unsuccessfully.

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

## Reporting a problem

Three things describe almost any mbx problem, and a bug report that carries
them rarely needs a follow-up question:

```sh
mbx doctor --json           # environment, store, and setup state
MBX_LOG=debug cargo build     # mbx's own diagnostics for the run
MBX_BYPASS_LOG=bypass.log cargo build   # why each compilation was not cached
```

All three describe your machine: `mbx doctor --json` reports absolute cache
paths and the URL and namespace of any configured remote, and the logs name
the crates you build. Credentials themselves are never printed, but remove
whatever else you would rather not publish before posting.

`MBX_LOG` takes an [env_logger](https://docs.rs/env_logger) filter, so
`debug`, `trace`, or a per-module filter such as `mbx=trace` all work; it
defaults to `info`. It covers the `mbx` process that drives the build. The
rustc shim runs without a logger, so per-compilation detail comes from
`MBX_BYPASS_LOG` instead, or from `mbx explain` for a grouped summary. See
[Cache results](/cache-results).

Report a problem in
[Q&A discussions](https://github.com/jdx/mr-boxington/discussions/categories/q-a),
and propose a change in
[Ideas](https://github.com/jdx/mr-boxington/discussions/categories/ideas).
A suspected vulnerability goes through
[private advisory reporting](https://github.com/jdx/mr-boxington/security/advisories/new)
rather than a public thread; see
[SECURITY.md](https://github.com/jdx/mr-boxington/blob/main/SECURITY.md).

## Next steps

- Tune local behavior in [Configuration](/configuration).
- Warm pull requests with [GitHub Actions cache](/github-action).
- Run independent tasks together through
  [mise or GitHub Actions](#run-multiple-cargo-builds-at-the-same-time).
- Learn what enters a key in [How it works](/how-it-works).
