---
description: Install mbx with mise, Cargo, or verified release archives on Linux, macOS, and Windows.
---
# Installation

mbx uses your existing Rust toolchain. Install Cargo and rustc before running a
build; `mbx doctor` checks which tools are active.

## mise

```sh
mise use --global --tool-option mr_boxington=true rust mr-boxington
```

Requires mise 2026.9.2 or newer. This installs Rust and mbx and enables Cargo
wrapping through mise; no `mbx setup` postinstall hook is needed.
`--tool-option` applies to the following tool, so keep it before `rust`.
Drop `--global` to enable it only in the current project. Keep an existing
Rust version pin by [editing its tool entry](/setup#share-setup-with-a-project).

Use `mise exec -- cargo build`, or open a shell with mise activation or shims
on `PATH` and follow [Verify plain Cargo](/setup#verify-plain-cargo).
The Rust option does not install a standalone mbx shim or configure
rust-analyzer; see [editor setup](/setup#rust-analyzer) if you need that too.

### Older mise versions

With mise 2026.8.16 through 2026.9.1:

```sh
mise use --global --postinstall "mbx setup --yes" mr-boxington
```

This runs standalone setup and writes an explicit `[wrappers.cargo]` entry.
Versions older than 2026.8.16 install the standalone shim and print an upgrade
warning instead of editing mise configuration.

## Cargo

```sh
cargo install mbx --locked
mbx --version
```

You can now run `mbx build` in a Rust workspace. To make plain `cargo` commands
use mbx too, run [`mbx setup`](/setup).

## Release archives

:::tabs
== Linux x86-64

```sh
mkdir -p ~/.local/bin
archive=mbx-x86_64-unknown-linux-gnu.tar.gz
release=https://github.com/jdx/mr-boxington/releases/latest/download
curl -fsSLO "$release/$archive" &&
  curl -fsSLO "$release/SHA256SUMS" &&
  grep "  $archive$" SHA256SUMS | sha256sum --check --strict - &&
  tar -xzf "$archive" -C ~/.local/bin
```

== Linux ARM64

```sh
mkdir -p ~/.local/bin
archive=mbx-aarch64-unknown-linux-gnu.tar.gz
release=https://github.com/jdx/mr-boxington/releases/latest/download
curl -fsSLO "$release/$archive" &&
  curl -fsSLO "$release/SHA256SUMS" &&
  grep "  $archive$" SHA256SUMS | sha256sum --check --strict - &&
  tar -xzf "$archive" -C ~/.local/bin
```

== macOS Apple Silicon

```sh
mkdir -p ~/.local/bin
archive=mbx-aarch64-apple-darwin.tar.gz
release=https://github.com/jdx/mr-boxington/releases/latest/download
curl -fsSLO "$release/$archive" &&
  curl -fsSLO "$release/SHA256SUMS" &&
  grep "  $archive$" SHA256SUMS | shasum -a 256 --check --strict - &&
  tar -xzf "$archive" -C ~/.local/bin
```

== Windows x86-64

```powershell
$ErrorActionPreference = "Stop"
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
$ErrorActionPreference = "Stop"
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

After extracting an archive, put its directory on `PATH`, then run
`mbx --version` and `mbx doctor`. Run `mbx setup` to enable
[automatic Cargo wrapping](/setup).

## Supported platforms

Release binaries cover:

- Linux x86-64 and ARM64 (GNU and static musl builds)
- macOS on Apple Silicon
- Windows x86-64 and ARM64

Other platforms with a Rust toolchain can build from source with
`cargo install mbx --locked`. mbx wraps whichever Cargo and rustc are active,
including rustup-managed toolchains. `mbx doctor` reports the pair it found.

Reflinked output restoration needs a filesystem with copy-on-write file
cloning: APFS on macOS, btrfs or XFS on Linux, ReFS (Dev Drive) on Windows.
mbx probes the cache and target locations and copies bytes where cloning is
unavailable. Caching still works on ext4 or NTFS but spends the disk twice.

### Windows

Windows is a supported release platform. The differences from Linux and macOS:

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

## Upgrade or uninstall

Upgrade using the same installation method: update the mise version, rerun
`cargo install mbx --locked`, or replace the binary with a verified release
archive. The stable Cargo shim follows upgrades; setup does not need to run
again.

Before removing the executable, disable `mr_boxington` in each Rust tool entry
where you enabled it. If you also ran standalone setup, run
`mbx setup --uninstall` in each scope you enabled. See
[Remove automatic wrapping](/setup#remove-automatic-wrapping).
Cached work is disposable; use [cache management](/managed-targets) to inspect
and reclaim it.

Continue with [your first build](/getting-started#run-a-build).
