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
identity. Custom target specifications, external WebAssembly toolchains,
native libraries, custom linkers, disabled WASI CRT bundling, and
non-affirmative `link-self-contained` modes remain uncached.

`MBX_CACHE_LINKS=1` adds host test binaries and executables on Linux and
macOS, by putting the rest of the link into the key: the resolved `cc` driver
and its version, the linker it selects, the startup objects and libc it
resolves (hashed), and on macOS the SDK. Two hosts that differ in any of those
produce different keys and miss, rather than sharing a binary neither of them
built. It is experimental — qualify it on your own workload as described in
[verify mode](/configuration#verify-mode) before relying on it.

Some hosts cannot be described at all. mbx asks the driver to place a startup
object and a libc, and a host where neither resolves gets no linker identity
and therefore no cached links — refusing is the only safe answer, because two
hosts failing the same probe would otherwise agree on a key without either of
them having pinned what it stood for. The same goes for a driver that names no
linker or reports no version. Those links appear in `mbx explain` like any
other bypass.

Even then, a link bypasses if it names a native library, overrides the linker,
or carries a flag that would embed this checkout's paths (`-Crpath`,
`-Cprefer-dynamic`), leave a file beside the binary that mbx does not store
(`-Csplit-debuginfo`), or — on macOS — record absolute object paths and their
timestamps in the binary's debug map (`-Cdebuginfo` above `0`). An explicit
`--target` bypasses too, even when it spells the host triple: rustc without one
links for the host by construction, and that is the only linker mbx identifies.

## Restored artifacts are equivalent, not always identical

A restore writes this checkout's spelling of the outputs that describe where a
compilation ran -- its dep-info and its diagnostics -- so the files cargo reads
name the directory it is building into.

The compiled artifacts are reused as they were produced, and two things can
make them differ from what a fresh compilation here would have written. rustc
records absolute source paths in metadata and debug information, so artifacts
built from two checkouts differ even when the sources are identical. On
Windows they differ between two target directories as well, because the debug
information also records where the objects were written.

Neither changes what the artifact does. Both are visible to `MBX_VERIFY=1`,
which compares bytes: a divergence it reports for a compilation restored from
another checkout, or on Windows from another target directory, is that
difference rather than a fault.
## C and C++ caching covers build-script compiles only

mbx caches the C and C++ a cargo build script compiles through `CC` and `CXX`,
which is what the `cc` crate uses. Standalone C projects driven by make or
CMake are outside a cargo build and are not reached.

Only a plain single-source `-c` compile through a gcc-style or clang-style
driver is admitted. Linking, preprocessing, assembly, Objective-C, precompiled
headers, coverage instrumentation, compiler plugins, options forwarded to a
sub-tool with `-Wp,`/`-Wa,`/`-Wl,`/`-Xclang`, response files, and MSVC all
bypass, as does any flag the adapter does not model. A source or header that
expands `__DATE__`, `__TIME__`, or `__TIMESTAMP__` bypasses too: its object is
not a function of its inputs.

The shims are only installed when the build has not chosen its own compiler.
Setting `CC`, `CXX`, `TARGET_CC`, or `TARGET_CXX` leaves that build's C
compilations uncached, and `MBX_CC=0` disables the feature.

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
