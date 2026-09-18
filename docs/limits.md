---
description: Find out which Rust, native-link, build-script, and C/C++ invocations mbx can safely cache.
---
# Caching limits {#limits}

When mbx cannot model a compilation exactly, it runs the compiler and does not
cache the result. A bypass preserves the build; it reduces reuse. Run
`mbx explain --last` to identify the reason for a particular action.

| Work | Cache behavior |
| --- | --- |
| Rust compilations without a link | Eligible when inputs can be modeled |
| Rust libraries naming a native library (`-l`) | Eligible; a `-l static` archive is hashed into the key |
| Native executables, tests, and proc macros | Eligible on described Linux, macOS, and Windows hosts |
| Built-in self-contained WebAssembly links | Eligible for the targets listed below |
| Build-script execution | Eligible using Cargo's declared freshness inputs |
| C and C++ object compilation | Eligible through supported compiler wrappers |
| GCC/Clang preprocessing to a file | Eligible, with checkout-specific keys |
| Incremental state | Private to the checkout; never shared |
| Unknown flags, inputs, or extra outputs | Runs without shared caching |

The sections below spell out the boundaries. For an unexpected result, start
with [Troubleshooting](/troubleshooting).

## Build-script execution follows Cargo's freshness inputs

mbx caches running `build.rs`, not only compiling it. After a successful first
run, the script's `cargo:rerun-if-changed` and `cargo:rerun-if-env-changed`
directives become the input prediction for later runs. The build-script binary,
Cargo's implicit unit environment (target, profile, features, configuration,
and package metadata), the recursively hashed declared paths, and the declared
environment values form the action key. A hit restores the complete `OUT_DIR`
tree and replays the script's stdout directives and stderr without starting the
script. Directories and missing paths are inputs too, matching Cargo's directive
model.

Set `build_script_execution = false` or `MBX_BUILD_SCRIPT_EXECUTION=0` to turn
off this layer while retaining ordinary Rust and C/C++ compilation caching.

A script that emits neither kind of rerun directive uses Cargo's package-wide
default. mbx hashes the package tree into the action key, excluding the target
directory and version-control metadata. This content key is stricter than
Cargo's timestamp check while remaining portable across equivalent checkouts.

Cached directives remap `OUT_DIR`, manifest/workspace and target roots, and
`CARGO_HOME` to the restoring environment. Output trees that do not contain the
literal output directory path can therefore cross target directories;
an output file that embeds that path keeps it in the action key and only reuses
the result at the same location. A symlink that may escape `OUT_DIR` makes the
execution uncacheable. The launcher left in a target directory is transparent
when the build later runs under plain Cargo, outside an mbx session.

## Incremental compilations are not cached

Incremental compilations bypass the action cache. Dependencies, which Cargo
builds non-incrementally, remain cacheable, and they are the bulk of a cold
build.

By default mbx forces `CARGO_INCREMENTAL=0` and instead gives a crate whose
sources keep changing its own private incremental state; see [learned
incremental reuse](/incremental#learned-incremental-reuse). Those
compilations are never published to the shared cache. See [incremental
builds](/incremental) for the trade `MBX_INCREMENTAL=1`
makes.

## Native linking is cached only where the linker can be described

Native binaries and dynamic libraries link against an external linker, startup
objects, and system libraries that rustc dep-info does not enumerate. mbx
caches such a link only when it can put all of that into the key, and bypasses
it otherwise.

WebAssembly needs nothing extra: a binary, test, or `cdylib` for one of these
built-in targets uses its compiler-bundled self-contained linker, so it is
cached on every platform:

- `wasm32-unknown-unknown`
- `wasm32-wasip1` and `wasm32-wasip1-threads`
- `wasm32-wasip2`
- `wasm32v1-none`
- `wasm64-unknown-unknown`

mbx caches those links because all explicit artifacts are modeled inputs and
the linker, CRT objects, and bundled libc are covered by the Rust toolchain
identity. Custom target specifications, external WebAssembly toolchains,
native libraries, unrecognized custom linkers, disabled WASI CRT bundling, and
non-affirmative `link-self-contained` modes remain uncached.

Host test binaries, executables, and proc macros are cached on Linux, macOS,
and Windows by putting the rest of the link into the key: the resolved `cc`
driver and its version, the linker it selects, the startup objects and libc it
resolves (hashed), and on macOS the SDK. On Windows the key identifies
`link.exe` or `lld-link`, the MSVC toolset and Windows SDK versions, and the
selected VC and Universal CRT libraries. Two hosts that differ in any of those
produce different keys and miss. `cache_links` (`MBX_CACHE_LINKS=0`) turns it
off.

Some hosts cannot be described. mbx asks the driver to place a startup object
and a libc; a host where neither resolves gets no linker identity and no cached
links, because two hosts failing the same probe would otherwise agree on a key
without either having pinned what it stood for. The same goes for a driver that
names no linker or reports no version. Those links appear in `mbx explain` like
any other bypass.

Even then, a link bypasses if it names a native library (`-l`, which a build
script emits as `cargo:rustc-link-lib`), overrides the linker, or carries a
flag that would embed this checkout's paths (`-Crpath`, `-Cprefer-dynamic`) or
leave a file beside the binary that mbx does not store (`-Csplit-debuginfo`).
On macOS a debug-info link records absolute object paths and their timestamps
in the binary's debug map, so the shim passes ld64 `-oso_prefix` for its own
output directory, which lets those links cache. An explicit `--target`
bypasses too, even when it spells the host triple: rustc without one links for
the host, and that is the only linker mbx identifies.

## Native libraries are inputs where nothing links

A library compilation runs no linker, so a `-l` flag on it is not a linker
argument. `-sys` crates whose build script emits `cargo:rustc-link-lib` (for
example `zstd-sys`, `ring`, `aws-lc-sys`, `libz-sys`, and `openssl-sys` when
linking statically) compile their rlib this way, and mbx caches those
compilations.

For `-l static=NAME`, with any modifiers, rustc reads the archive and bundles
it into the rlib. mbx resolves the file the way rustc does, taking the first
`-L native` directory in command-line order that holds `libNAME.a` (or
`NAME.lib` on MSVC targets; the literal name with `+verbatim`), and hashes it
into the action key exactly like an `--extern` artifact. A rebuilt archive
with the same name therefore gives the library a new key, on a fresh
compilation and on a predicted restore alike. If no search directory holds
the archive, the compilation bypasses as `missing-native-library`; an archive
outside the workspace, target, Cargo, toolchain, and home roots bypasses as
`unmapped-absolute-path`, like any other input there.

For `-l dylib=NAME`, `-l framework=NAME`, `-l link-arg=...`, and a plain
`-l NAME`, rustc reads nothing and records the name in the crate's metadata
for a later link, so the flag enters the key as text.

An archive built with debug information usually records the checkout's C
source paths. When the build script that produces it reruns in another
checkout, the archive differs and so does the library's key; when
[build-script execution](#build-script-execution-follows-cargos-freshness-inputs)
restores the archive from the cache instead, the bytes match and the library
hits there too. Either way the library is reused across builds at the same
path, including after the target directory is removed. Linked programs, tests,
and proc macros that name a native library still bypass, as described above.

## Restored artifacts are equivalent, not always identical

On restore, mbx rewrites dep-info and diagnostics to use the current checkout
and target paths. Cargo can then read them as if the compilation ran locally.

The compiled artifacts are reused as they were produced, and a few things can
make them differ from what a fresh compilation here would have written. rustc
records absolute source paths in metadata and debug information, so artifacts
built from two checkouts differ even when the sources are identical. A C or C++
object compiled with debug information does the same, recording the directory
the compiler ran in.

A C or C++ object also records the absolute include directories it was given,
which is how a `-sys` crate whose build script generates headers into `OUT_DIR`
used to produce a different object in every target directory. With
[`OUT_DIR` sharing](/configuration#share-out-dir) on (the default), mbx passes
the compiler `-fdebug-prefix-map` for that directory, so the object records the
same placeholder the key does and two target directories produce the same bytes.
An object that still embeds a checkout or target path is stored under a checkout-specific
key by default. Set `MBX_CC_STORE_PATH_SPECIFIC=0` to skip storing those
objects; existing entries remain readable. Windows debug information can also
record the object output path.

These path differences can affect debugging and byte-for-byte comparisons.
`MBX_VERIFY=1` reports them as divergences, but a divergence alone does not
establish its cause. Inspect the named output and mismatch before attributing it
to paths, and report unexplained differences. See
[Verify mode](/configuration#verify-mode) for a controlled comparison and the
[debugger recipe](/cookbook/local-development#debug-a-binary-restored-from-another-checkout)
for a build using local source paths.

## C and C++ caching covers the host compiles mbx drives

mbx caches the C and C++ a Cargo build script compiles for the host through
the `cc` crate, and the C and C++ of a command run under
[`mbx exec`](/standalone-builds), which puts shims for the plain driver names
on `PATH` for that command alone. A compile outside both paths is not reached.
Neither are cross compilations the build did not name a compiler for: a Cargo
build installs the shims as `HOST_CC` and `HOST_CXX`, which the `cc` crate
consults only when host and target agree, and `mbx exec` shims only `cc`,
`c++`, `gcc`, `g++`, `clang`, and `clang++` on Unix, plus `cl.exe` on Windows,
leaving a versioned toolchain to the build that chose it.

A cross compile is cached when the build names its own compiler through
`CC_<target>`, `CXX_<target>`, `TARGET_CC`, or `TARGET_CXX`: mbx wraps what was
named. A cross build that names nothing is left alone, because which compiler a
target implies lives in the `cc` crate's own tables, and a wrong guess would
build the object with the wrong compiler. A value that is a command, such as
`ccache gcc`, is left alone for the same reason.

### Supported compiler calls

mbx caches single-source object compiles through GCC-, Clang-, and MSVC-style
drivers. Preprocessed assembly (`.S` or `-x assembler-with-cpp`) is supported:
its includes participate in dependency discovery, and the assembler is part
of the GCC toolchain identity. Known assembler options that add no inputs,
such as `-Wa,--noexecstack`, are also supported.

GCC/Clang preprocessing with `-E ... -o file` is cached too. The file is
restored byte-for-byte, preserving line markers and `__FILE__` values. These
entries key literal argument paths, the working directory, and mapped roots,
so they cannot be shared across different checkout paths. Header discovery
and include-directory invalidation still apply.

With `-E` and `-MD`/`-MMD`, an explicit `-MF` is required. Without it, GCC gives
`-o` a different meaning and mbx bypasses the invocation.

### Calls that bypass the cache

The C/C++ adapter leaves these calls uncached:

- Links, multi-source calls, preprocessing to stdout, and dependency output
  to stdout (`-MF -`).
- Assembly without preprocessing (`.s`), Objective-C, precompiled headers,
  coverage instrumentation, compiler plugins, and response files.
- Unmodeled sub-tool options, including `-Wp,`, `-Wl,`, `-Xclang`, and
  unsupported `-Wa,` options; assembler-time `.include`, `.sinclude`, and
  `.incbin` inputs.
- MSVC compiler PDBs, modules, and other unsupported extra outputs.
- Sources or headers that expand `__DATE__`, `__TIME__`, or `__TIMESTAMP__`:
  the result depends on when compilation runs.
- Host CPU tuning such as `-march=native`: the cache key does not identify
  the host processor.
- Any other flag or input the adapter cannot model.

Setting `CC`, `CXX`, `HOST_CC`, or `HOST_CXX` leaves the selected host compiler
outside mbx's wrappers. Explicit target compilers are handled as described
above. `MBX_CC=0` disables C and C++ caching entirely.

## Shadowing is modeled by name, not by content

An include directory contributes the names in it that could answer an
`#include`: headers, sources (`#include "generated.c"` is unusual but legal),
names without an extension, and precompiled headers, which GCC prefers over
the header they were built from without anything on the command line saying so.

Objects, dependency files, and archives are left out, because they cannot
answer an `#include`. A build writes those into the directory a generated
header lives in, and counting them would make the key depend on how many
sibling compilations had finished.

Manifests are taken once before the compiler runs and again before publishing.
If a search directory changed in between, the compilation bypasses: a header
that appeared while the compiler ran is one it never saw, and the key would
otherwise claim a state that did not produce this object.

System roots are exempt from manifests. Enumerating an SDK on every compile
costs more than the risk, and anything read from one is digested like any other
input.

## `OUT_DIR` sharing remaps generated source paths

A generated source path can contain an absolute checkout-specific `OUT_DIR`.
A compilation that reads one is cached for the checkout it ran in, and
recompiles the first time each new checkout builds it. mbx cannot tell whether
such an artifact depends on the path: reading the outputs back finds a path
that was kept verbatim, but not one the crate derived a value from.

mbx remaps the path so rustc records a placeholder instead, which is what lets
a dependency recompiled in a second checkout come out byte-identical, so the
crates above it still share. Two cases stay checkout-specific anyway. A
workspace member records its own directory, which `MBX_SHARE_WORKSPACE_ROOT`
covers and the `OUT_DIR` remapping does not. A crate that keeps the value it
read through `env!` embeds it. Both produce a different artifact in each
checkout, and their dependents recompile with them.

Set `MBX_SHARE_OUT_DIR=0` to keep generated source paths literal in debug
information, at the cost of that dependent sharing.

This covers C and C++ as well as Rust. A build script that generates headers
into `OUT_DIR` passes that directory to its own compilations, which record it
in debug information, so the same remapping applies: rustc is told
`--remap-path-prefix` and the C compiler `-fdebug-prefix-map`.
`MBX_SHARE_OUT_DIR=0` turns both off together.

## A rebuilt workspace crate records its checkout

Cargo runs rustc with the crate's own directory as the working directory, and
rustc stores that directory in the artifact. Two checkouts of the same commit
therefore produce different bytes for the same workspace crate whenever it is
actually compiled rather than restored, and every crate above it misses too,
because what it consumes differs.

Most builds never see this: a workspace crate that can be restored is restored,
byte for byte. It shows up where a crate has to be compiled in each checkout
anyway, which is what a compilation keyed to its checkout does. One crate low in
the graph that reads `OUT_DIR` or `CARGO_MANIFEST_DIR` can carry most of a large
workspace with it.

Set `MBX_SHARE_WORKSPACE_ROOT=1` (or `share_workspace_root = true`) to map the
workspace root to a placeholder, which leaves the two checkouts producing the
same bytes. The compilation that read the value is still keyed to its checkout;
what changes is that its dependents stop paying for that. The cost is that
source paths under the workspace are recorded as the placeholder, in debug
information, `file!()` and panic locations, which is why it is off by default.

## Incremental output reduces sharing

`MBX_INCREMENTAL=1` can improve a local edit/rebuild loop, but an incremental
artifact changes the content inputs of dependent crates. Those crates then miss
even if another checkout built the same source. CI disables incremental builds.

## Collection is approximate

Object eviction prefers abandoned checkout data and then older access times.
Filesystems using `relatime` coarsen that order; `noatime` removes it. A poor
choice costs a recompile, not correctness.

The action-store budget covers action objects and results. Prediction data,
checkout records, and temporary downloads add overhead. Managed targets have
their own budget and can account for substantial space; use the optional
combined budget to bound the action store, managed targets, and learned
incremental state together. See
[Managed target directories](/managed-targets).
