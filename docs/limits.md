# Limits

mr boxington favors correct uncached work over risky cache reuse.

## Incremental compilations are not cached

Cargo's normal incremental workspace compilations bypass the action cache.
Dependencies, which Cargo builds non-incrementally, remain cacheable — and they
are the bulk of a cold build. See [incremental
builds](/configuration#incremental-builds) for the trade `MBX_INCREMENTAL=1`
makes.

## Native linking is not cached

Native binaries and dynamic libraries always link. Their results can depend on
an external linker, startup objects, and system libraries that rustc dep-info
does not enumerate, so mbx bypasses them.

The narrow exception is a binary, test, or `cdylib` for one of these built-in
targets using its compiler-bundled self-contained linker:

- `wasm32-unknown-unknown`
- `wasm32-wasip1` and `wasm32-wasip1-threads`
- `wasm32-wasip2`
- `wasm32v1-none`
- `wasm64-unknown-unknown`

mbx caches those links because all explicit artifacts are modeled inputs and
the linker, CRT objects, and bundled libc are covered by the Rust toolchain
identity. Native targets, custom target specifications, external WebAssembly
toolchains, native libraries, custom linkers, disabled WASI CRT bundling, and
non-affirmative `link-self-contained` modes remain uncached.

## `OUT_DIR` sharing is opt-in

A generated source path can contain an absolute checkout-specific `OUT_DIR`.
`MBX_SHARE_OUT_DIR=1` remaps the path and inspects outputs before choosing a
shared key, but generated sources then appear in debug info under a placeholder
path. It cannot detect a value derived from the path without embedding the path
itself.

## Incremental output reduces sharing

`MBX_INCREMENTAL=1` can improve a local edit/rebuild loop, but an incremental
artifact changes the content inputs of dependent crates. Those crates then miss
even if another checkout built the same source. CI disables incremental builds.

## Collection is approximate

Object eviction prefers abandoned checkout data and then older access times.
Filesystems using `relatime` coarsen that order; `noatime` removes it. A poor
choice costs a recompile, not correctness.

The configured size budget covers action objects and results. Prediction data,
checkout records, temporary downloads, and managed target directories make the
whole cache directory somewhat larger.
