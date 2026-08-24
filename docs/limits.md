# Limits

mr boxington favors correct uncached work over risky cache reuse.

## Use `mbx build`

Plain `cargo build` does not start the session agent, so it gets no mbx cache.
Use `mbx build build`, `mbx build test`, and the same pattern for other cargo
subcommands.

## Linking is not cached

Binaries and dynamic libraries always link. mbx caches supported rustc
compilations, not the final link action.

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
