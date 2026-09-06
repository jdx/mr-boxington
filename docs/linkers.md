---
description: Select system, toolchain LLD, mold, or Wild linkers by Cargo profile and target.
---
# Managed linkers

The default `system` selection preserves Cargo's linker. Change it when you
want to try another linker or share a pinned choice across a workspace.
Managed selections require `clang` on `PATH`. Keep `system` on Windows;
managed `rust-lld` selection is not supported there.

## Try a linker

On Linux, try a pinned mold release for one build:

```sh
MBX_LINKER=mold@2.42.0 mbx build
```

This is an example version, not an automatically updated recommendation. On
Linux or macOS, `rust-lld` uses the active Rust toolchain's bundled LLD without
a download:

```sh
MBX_LINKER=rust-lld mbx build
```

## Selectors

mbx can select a linker by Cargo profile and target triple, install an exact
version from its official GitHub releases, and route native Rust links through
it. The built-in selectors are:

- `system`, which leaves Cargo's linker selection alone;
- `rust-lld` or `lld`, which uses the LLD shipped with the active Rust
  toolchain;
- `mold@<version>` or `wild@<version>`, which downloads the
  matching release asset and verifies GitHub's SHA-256 digest; and
- `path:<executable>`, which selects a linker mbx does not install.

Managed GitHub linkers require an exact version. Downloads are installed once
beneath `<cache_dir>/tools`; concurrent builds share the same installation
lock. `GITHUB_TOKEN` may authenticate GitHub API and download requests.

Within a profile table, an exact target triple wins over `default`. The
top-level `linker.default` applies when the active profile has no entry. Cargo's
ordinary profile is `dev`, `--release` selects `release`, `cargo bench` selects
`bench`, and `--profile <name>` selects that custom profile.

```toml
[linker]
default = "system"

[linker.profiles.dev]
x86_64-unknown-linux-gnu = "mold@2.42.0"
aarch64-unknown-linux-gnu = "wild@0.10.0"

[linker.profiles.release]
default = "rust-lld"
```

`MBX_LINKER` overrides every file for one invocation:

```sh
MBX_LINKER=mold@2.42.0 cargo build
MBX_LINKER=system cargo build --release
```

mold and Wild currently provide managed releases for Linux. Unsupported host
platforms fail before Cargo starts rather than silently changing the linker.
The selected executable remains part of mbx's native-link cache identity.

Store persistent selections in your [global configuration](/configuration) or
in the [workspace policy](/configuration#workspace-policy). The `path:` selector
is accepted only outside repository-owned policy. A linker change can
change cache keys; compare equivalent builds when measuring it.
