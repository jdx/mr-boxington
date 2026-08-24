# Limits

mr boxington favors correct uncached work over risky cache reuse.

## Plain Cargo has local-only caching

After `mbx setup`, plain Cargo commands use the local action store. They do not
start a session agent, so remote transfers, build statistics, managed targets,
and automatic collection still require `mbx build`, `mbx test`, or the same
pattern for another Cargo subcommand. Cargo's normal incremental workspace
compilations continue to bypass the action cache; dependencies, which Cargo
normally builds non-incrementally, remain cacheable.

## Native linking is not cached

Native binaries and dynamic libraries always link. Their results can depend on
an external linker, startup objects, and system libraries that rustc dep-info
does not enumerate, so mbx bypasses them.

The narrow exception is a binary or test for `wasm32-unknown-unknown` using its
default compiler-bundled linker. mbx caches that link because all explicit
artifacts are modeled inputs and the linker is covered by the Rust toolchain
identity. Native targets, custom target specifications, native libraries,
custom linkers, and custom `link-self-contained` modes remain uncached.

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
