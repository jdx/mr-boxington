# Limits

mr boxington favors correct uncached work over risky cache reuse.

## Incremental compilations are not cached

Cargo's normal incremental workspace compilations bypass the action cache.
Dependencies, which Cargo builds non-incrementally, remain cacheable — and they
are the bulk of a cold build. See [incremental
builds](/configuration#incremental-builds) for the trade `MBX_INCREMENTAL=1`
makes.

## Native linking is cached only where the linker can be described

Native binaries and dynamic libraries link against an external linker, startup
objects, and system libraries that rustc dep-info does not enumerate. mbx
caches such a link only when it can put all of that into the key, and bypasses
it otherwise.

WebAssembly is the case that needs nothing extra: a binary, test, or `cdylib`
for one of these built-in targets uses its compiler-bundled self-contained
linker, so it is cached on every platform:

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

Host test binaries and executables are cached on Linux and macOS, by putting
the rest of the link into the key: the resolved `cc` driver and its version,
the linker it selects, the startup objects and libc it resolves (hashed), and
on macOS the SDK. Two hosts that differ in any of those produce different keys
and miss, rather than sharing a binary neither of them built. `cache_links`
(`MBX_CACHE_LINKS=0`) turns it off; Windows has no such tier, and a build
there links its programs as it always did.

Some hosts cannot be described at all. mbx asks the driver to place a startup
object and a libc, and a host where neither resolves gets no linker identity
and therefore no cached links — refusing is the only safe answer, because two
hosts failing the same probe would otherwise agree on a key without either of
them having pinned what it stood for. The same goes for a driver that names no
linker or reports no version. Those links appear in `mbx explain` like any
other bypass.

Even then, a link bypasses if it names a native library, overrides the linker,
or carries a flag that would embed this checkout's paths (`-Crpath`,
`-Cprefer-dynamic`) or leave a file beside the binary that mbx does not store
(`-Csplit-debuginfo`). On macOS a debug-info link records absolute object
paths and their timestamps in the binary's debug map, so the shim passes ld64
`-oso_prefix` for its own output directory, which is what lets those links
cache rather than bypass. An explicit
`--target` bypasses too, even when it spells the host triple: rustc without one
links for the host by construction, and that is the only linker mbx identifies.

## Restored artifacts are equivalent, not always identical

A restore writes this checkout's spelling of the outputs that describe where a
compilation ran — its dep-info and its diagnostics — so the files cargo reads
name the directory it is building into.

The compiled artifacts are reused as they were produced, and a few things can
make them differ from what a fresh compilation here would have written. rustc
records absolute source paths in metadata and debug information, so artifacts
built from two checkouts differ even when the sources are identical. A C or C++
object compiled with debug information does the same, recording the directory
the compiler ran in.

A C or C++ object also records the absolute include directories it was given,
which is how a `-sys` crate whose build script generates headers into
`OUT_DIR` used to produce a different object in every target directory. With
[`OUT_DIR` sharing](/configuration#share-out-dir) on — the default — mbx
passes the compiler `-fdebug-prefix-map` for that directory, so the object
records the same placeholder the key does and two target directories produce
the same bytes. An object that keeps the path anyway, in a string the source
holds rather than in debug information, is not published at all rather than
shared under a key that says the path does not matter. The object path alone
never did this on Linux or macOS; on Windows it does, because the debug
information records where the object was written as well.

None of these changes what the artifact does. All of them are visible to
`MBX_VERIFY=1`, which compares bytes: a divergence it reports for a
compilation restored from another checkout, or from another target directory
whose paths the object records, is that difference rather than a fault. The
divergence names the file and what differed about it, so a run's divergences
can be told apart from one another rather than counted together.

## C and C++ caching covers the host compiles mbx drives

mbx caches the C and C++ a cargo build script compiles for the host through
the `cc` crate, and the C and C++ of a command run under
[`mbx exec`](/standalone-builds), which puts shims for the plain driver names
on `PATH` for that command alone. A compile mbx never stood in for — one
outside both paths, or on Windows, where no shims are installed — is not
reached. Neither are cross compilations: a cargo build installs the shims as
`HOST_CC` and `HOST_CXX`, which the `cc` crate consults only when host and
target agree, and `mbx exec` shims only `cc`, `c++`, `gcc`, `g++`, `clang`,
and `clang++`, leaving a versioned toolchain to the build that chose it.

A cross compile is cached when the build names its own compiler, through
`CC_<target>`, `CXX_<target>`, `TARGET_CC`, or `TARGET_CXX`: mbx wraps what
was named rather than replacing it. A cross build that names nothing is left
alone, because which compiler a target implies lives in the `cc` crate's own
tables — guessing wrong would not cost a cache hit, it would build the object
with the wrong compiler. A value that is a command rather than a path, such as
`ccache gcc`, is left alone for the same reason.

Only a plain single-source `-c` compile through a gcc-style or clang-style
driver is admitted. Linking, preprocessing, assembly, Objective-C, precompiled
headers, coverage instrumentation, compiler plugins, options forwarded to a
sub-tool with `-Wp,`/`-Wa,`/`-Wl,`/`-Xclang`, response files, and MSVC all
bypass, as does any flag the adapter does not model. A source or header that
expands `__DATE__`, `__TIME__`, or `__TIMESTAMP__` bypasses too: its object is
not a function of its inputs.

A flag that tunes for the machine's own processor — `-march=native` and its
relatives — bypasses as well, since the object it produces is not a function
of anything the key names.

## Shadowing is modeled by name, not by content

An include directory contributes the names in it that could answer an
`#include`: headers, sources — `#include "generated.c"` is unusual but legal —
names without an extension, and precompiled headers, which GCC prefers over
the header they were built from without anything on the command line saying so.

What is left out is what cannot answer an `#include` at all: an object, a
dependency file, an archive. That distinction is what keeps the key stable,
because a build writes those into the very directory a generated header lives
in, and counting them would make the key depend on how many sibling
compilations had finished rather than on anything about this one.

Manifests are taken once before the compiler runs and again before publishing.
A header that appeared while the compilation was in flight is one the compiler
never saw, so recording it would claim a state that did not produce this
object.

System roots are exempt from manifests entirely: enumerating an SDK on every
compile costs more than the risk, and anything actually read from one is
digested like any other input.

The shims are only installed when the build has not chosen its own compiler.
Setting `CC`, `CXX`, `HOST_CC`, `HOST_CXX`, `TARGET_CC`, or `TARGET_CXX` leaves
that build's C compilations uncached, and `MBX_CC=0` disables the feature.

## `OUT_DIR` sharing remaps generated source paths

A generated source path can contain an absolute checkout-specific `OUT_DIR`.
mbx remaps the path and inspects outputs before choosing a shared key, but
generated sources then appear in debug info under a placeholder path. It cannot
detect a value derived from the path without embedding the path itself. Set
`MBX_SHARE_OUT_DIR=0` to keep generated source paths literal at the cost of
cross-checkout cache sharing for their dependent crates.

This covers C and C++ as well as Rust. A build script that generates headers
into `OUT_DIR` passes that directory to its own compilations, which record it
in debug information, so the same remapping applies — rustc is told
`--remap-path-prefix` and the C compiler `-fdebug-prefix-map`, and in both
cases an output that kept the literal path is left uncached rather than
shared. `MBX_SHARE_OUT_DIR=0` turns both off together.

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
