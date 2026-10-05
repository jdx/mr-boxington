---
description: Install mbx with mise, Cargo, Nix, or verified release archives on Linux, macOS, and Windows.
---
# Installation

mbx wraps the Cargo and rustc that are active. The [mise](#mise) command
installs Rust along with mbx; otherwise, install Cargo and rustc before running
a build. `mbx doctor` checks which tools are active.

## mise

```sh
mise use --global --tool-option mr_boxington=true rust mr-boxington
```

With mise 2026.9.2 or newer, this installs Rust and mbx and has mise run Cargo
commands through mbx, so no `mbx setup` postinstall hook is needed.
`--tool-option` applies to the tool that follows it, so keep it before `rust`.
Drop `--global` to set this up only for the current project. To keep an
existing Rust version pin, add `mr_boxington = true` to that `rust` entry
instead. See [Share setup with a project](/setup#share-setup-with-a-project).

Use `mise exec -- cargo build`, or open a shell with mise activation or shims
on `PATH` and follow [Verify plain Cargo](/setup#verify-plain-cargo).

The `mr_boxington` option does not install the Cargo shim or configure
rust-analyzer. To add both, see [rust-analyzer](/setup#rust-analyzer).

### Older mise versions

With mise 2026.8.16 through 2026.9.1:

```sh
mise use --global --postinstall "mbx setup --yes" mr-boxington
```

After installing mbx, mise runs [standalone setup](/setup#standalone-setup),
which writes an explicit `[wrappers.cargo]` entry. With mise older than
2026.8.16, `mbx setup` installs the Cargo shim and prints an upgrade warning
instead of editing the mise configuration.

## Cargo

```sh
cargo install mbx --locked
mbx --version
```

Building mbx from crates.io needs Rust 1.91 or newer as the active toolchain.
In a project that pins an older toolchain, run the install from another
directory, or select a newer toolchain with rustup:
`cargo +stable install mbx --locked`. mise and the release archives install a
prebuilt binary instead.

You can now run `mbx build` in a Rust workspace. To make plain `cargo` commands
use mbx too, run [`mbx setup`](/setup#standalone-setup).

## Nix

With Nix flakes enabled, run mbx directly from GitHub without installing it:

```sh
nix run github:jdx/mr-boxington -- --version
nix run github:jdx/mr-boxington -- build
```

The flake provides `mbx` as its default package and app on Linux and macOS,
for both x86-64 and ARM64. To include it in a project's dev shell, add it as
a flake input:

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
    mbx.url = "github:jdx/mr-boxington";
    mbx.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = { nixpkgs, mbx, ... }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
      mbxPackage = mbx.packages.${system}.mbx;
    in {
      devShells.${system}.default = pkgs.mkShell {
        packages = [ pkgs.cargo pkgs.rustc mbxPackage ];
        shellHook = ''
          export PATH="${mbxPackage}/libexec/mbx:$PATH"
        '';
      };
    };
}
```

Set `system` to your platform and run `nix develop`. The hook puts mbx's
Cargo wrapper first on `PATH`, so plain `cargo build`, `cargo test`, and
other build commands use mbx automatically. The wrapper lives in
`libexec/mbx/cargo`, separate from the package's `bin` directory, so adding
the package alone does not change how Cargo runs.

## Release archives

Each snippet downloads the latest archive for one platform, checks it against
the release's `SHA256SUMS`, and extracts it into a per-user directory.

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
Expand-Archive $archive -DestinationPath "$env:LOCALAPPDATA\Programs\mbx" -Force
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
Expand-Archive $archive -DestinationPath "$env:LOCALAPPDATA\Programs\mbx" -Force
```

Add `%LOCALAPPDATA%\Programs\mbx` to `PATH`.

:::

Every release publishes its archives and `SHA256SUMS` on
[GitHub Releases](https://github.com/jdx/mr-boxington/releases).
Linux also has `-musl` archives for a static binary that does not depend on a
host glibc.

After extracting an archive, make sure the destination directory is on `PATH`,
then run `mbx --version` and `mbx doctor`. To make plain `cargo` commands use
mbx, run [`mbx setup`](/setup#standalone-setup).

## Supported platforms

Release binaries cover:

- Linux x86-64 and ARM64 (GNU and static musl builds)
- macOS on Apple Silicon
- Windows x86-64 and ARM64

Other platforms with Rust 1.91 or newer can build from source with
`cargo install mbx --locked`. mbx wraps whichever Cargo and rustc are active,
including rustup-managed toolchains. `mbx doctor` reports the pair it found.

mbx restores a cached output into the target directory with the first of these
methods that works, and the method decides how much disk space the output
takes:

1. **Reflink.** The output is a copy-on-write clone of the store object. This
   needs the store and the target directory on the same filesystem, and that
   filesystem must support file cloning: APFS on macOS; Btrfs, XFS with
   reflink, or ZFS on Linux; or ReFS (Dev Drive) on Windows.
2. **Hard link.** Where cloning is unavailable, as on ext4, mbx on Linux,
   macOS, and other Unix systems hard links the store object into place. This
   needs the store and the target directory on the same filesystem, and the
   [`restore_hardlink`](/configuration#restore-hardlink) setting enabled (the
   default). The output shares the store object's disk space but is read-only,
   so running Cargo without mbx in that target directory can fail with
   `output file ... is not writeable`.
3. **Copy.** mbx copies the bytes when it can do neither. On Windows it never
   hard links, so it copies wherever it cannot clone. The store and the copied
   output take separate disk space.

Caching still works on ext4 and NTFS. `mbx doctor` reports which method applies
to the managed target root and to the current workspace's target directory. See
[Output restoration](/how-it-works#output-restoration).

### Windows

On Windows, mbx differs from Linux and macOS in these ways:

- Reflinks need the cache directory and the target directory on the same ReFS
  volume, which usually means a Dev Drive. Moving only the checkout is not
  enough: set [`cache_dir`](/configuration#cache-dir) to a path on the Dev
  Drive. On NTFS, mbx copies restored outputs, even where `mbx doctor` reports
  a hard link.
- A managed target directory needs a `target` symlink, which Windows creates
  only in Developer Mode or for a privileged process. If Windows refuses, mbx
  lets Cargo use its ordinary target directory. See
  [Managed target directories](/managed-targets).
- mbx caches rustc compilations and native host links. A native link's cache
  key binds the selected MSVC or LLVM linker, Windows SDK, and CRT. See
  [native link caching](/limits#native-linking-is-cached-only-where-the-linker-can-be-described).
- mbx caches MSVC C and C++ compilations from build scripts and `mbx exec`
  through a conservative `cl.exe` adapter. See
  [C and C++ caching](/limits#c-and-c-caching-covers-the-host-compiles-mbx-drives).

## Upgrade or uninstall

Upgrade the same way you installed:

- With mise, update the `mr-boxington` version.
- With Cargo, rerun `cargo install mbx --locked`.
- With a release archive, rerun your platform's snippet from
  [Release archives](#release-archives). It verifies the new archive and
  extracts it over the old binary.

The Cargo shim that `mbx setup` installs follows Cargo and release-archive
upgrades, which replace mbx at the same path. mise installs each version in its
own directory instead. On Unix, the shim runs the mbx that `mbx setup`
recorded, so run `mbx setup` again once mise removes that version. Until you
do, `mbx doctor` reports the shim as outdated, and the shim fails wherever mbx
is not on `PATH`.

To uninstall, turn off automatic wrapping before removing the executable.
Disable `mr_boxington` in each Rust tool entry where you enabled it. If you
also ran standalone setup, run `mbx setup --uninstall` in each scope you
enabled. See [Remove automatic wrapping](/setup#remove-automatic-wrapping).

Cached work is disposable. To inspect and reclaim it, see
[Inspect and clean up](/managed-targets#inspect-and-clean-up).

Next, [run a build](/getting-started#run-a-build).
