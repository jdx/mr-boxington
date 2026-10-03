---
description: Select system, toolchain LLD, mold, or Wild linkers by Cargo profile and target.
---
# Managed linkers

The default `system` selection preserves Cargo's linker. Change it when you
want to try another linker or share a pinned choice across a workspace.
Every selection other than `system` links through `clang`, so it requires
`clang` on `PATH`. Keep `system` on Windows, where mbx does not support
`rust-lld` selection.

## Try a linker

On Linux, try a pinned mold release for one build:

```sh
MBX_LINKER=mold@2.42.0 mbx build
```

This is an example version, not an automatically updated recommendation.

On Linux or macOS, `rust-lld` uses the active Rust toolchain's bundled LLD
without a download:

```sh
MBX_LINKER=rust-lld mbx build
```

mbx includes the selected linker executable in the cache key for native links
(see [Native linking is cached only where the linker can be described](/limits#native-linking-is-cached-only-where-the-linker-can-be-described)).
Changing the linker can therefore change cache keys, so compare equivalent
builds when you measure a linker's effect.

## Selectors

mbx selects a linker by Cargo profile and target triple and routes native Rust
links through it. For mold and Wild, it installs the exact version you name,
downloading it from the linker's official GitHub releases.

| Selector | Linker | Downloads |
| --- | --- | --- |
| `system` | Cargo's own linker selection, unchanged | No |
| `rust-lld` or `lld` | The LLD shipped with the active Rust toolchain | No |
| `mold@<version>` or `wild@<version>` | The named mold or Wild release, checked against GitHub's SHA-256 digest | Yes |
| `path:<executable>` | A linker mbx does not install | No |

The mold and Wild selectors require an exact version, not `latest`. mbx
installs each download once beneath `<cache_dir>/tools`, and concurrent builds
wait on the same installation lock. When `GITHUB_TOKEN` is set, mbx sends it
with its GitHub API and download requests.

mbx installs mold and Wild only on Linux. On an unsupported host, the build
fails before Cargo starts rather than silently changing the linker.

## Choose a linker by profile and target

Store persistent selections in your [global configuration](/configuration) or
in the [workspace policy](/configuration#workspace-policy). Workspace policy,
which the repository owns, cannot use the `path:` selector.

```toml
[linker]
default = "system"

[linker.profiles.dev]
x86_64-unknown-linux-gnu = "mold@2.42.0"
aarch64-unknown-linux-gnu = "wild@0.10.0"

[linker.profiles.release]
default = "rust-lld"
```

Within a profile table, an exact target triple wins over `default`. The
top-level `linker.default` applies when the active profile has no matching
entry. mbx reads the active profile from the Cargo command:

| Cargo command or flag | Profile |
| --- | --- |
| `--profile <name>` | `<name>` |
| `--release` | `release` |
| `cargo bench` | `bench` |
| `cargo install` | `release` |
| `cargo install --debug` | `debug` |
| Any other command | `dev` |

From Cargo 1.99, `cargo install --debug` builds with the `debug` profile. When
`linker.profiles.debug` is absent, mbx uses the `dev` table instead.

`MBX_LINKER` overrides the global configuration and workspace policy for
one invocation:

```sh
MBX_LINKER=mold@2.42.0 cargo build
MBX_LINKER=system cargo build --release
```
