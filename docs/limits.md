---
description: Find out which Rust, native-link, build-script, and C/C++ invocations mbx can safely cache.
---
# Caching limits {#limits}

When mbx cannot model a compilation exactly, it runs the compiler and does not
cache the result. That outcome is a bypass: the output is the same, but the
action gets no reuse. Run the build through `mbx explain` (for example,
`mbx explain build`) to see why actions bypassed; see
[Bypass](/cache-results#bypass).

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
directives become the [input prediction](/how-it-works#prediction-and-dep-info)
for later runs. The action key combines the build-script binary, Cargo's
implicit unit environment (target, profile, features, configuration, and
package metadata), the recursively hashed declared paths, and the declared
environment values. Directories and missing paths are inputs too, matching
Cargo's directive model. A hit restores the complete `OUT_DIR` tree and replays
the script's stdout directives and stderr without starting the script.

Set `build_script_execution = false` or `MBX_BUILD_SCRIPT_EXECUTION=0` to turn
off build-script caching while keeping Rust and C/C++ compilation caching.

A script that emits neither kind of rerun directive uses Cargo's package-wide
default. mbx hashes the package tree into the action key, excluding the target
directory and version-control metadata. This content key is stricter than
Cargo's timestamp check while remaining portable across equivalent checkouts.

A script that declares an empty `rerun-if-changed` path, or one inside its own
`OUT_DIR` (shadow-rs does this to force a rerun), asks Cargo to run it every
time. mbx keeps that contract: the script runs on every build and is recorded
as a `build-script-always-rerun` bypass.

Cached directives remap `OUT_DIR`, `CARGO_HOME`, and the package, workspace,
and target roots to the restoring environment. Output trees that contain none
of those paths can therefore cross target directories. An output file that
embeds the output directory, the package or workspace root, the target
directory, or `CARGO_HOME` puts the literal output directory into the action
key, so the result is reused only at the same location. A symlink that may
escape `OUT_DIR` makes the execution uncacheable.

The launcher mbx leaves in a target directory is transparent when the build
later runs under plain Cargo, outside a build session.

<span id="incremental-output-reduces-sharing"></span>

## Incremental compilations are not cached

Cargo builds dependencies non-incrementally, so they remain cacheable, and they
are the bulk of a cold build.

By default mbx forces `CARGO_INCREMENTAL=0`. Instead, it gives a workspace crate
its own learned incremental state from its first source edit, and any other
crate after three consecutive misses with changed sources; see
[learned incremental reuse](/incremental#learned-incremental-reuse). Those
compilations are never published to the shared cache. See
[Cargo incremental mode](/incremental#cargo-incremental-mode) for the trade-off
`MBX_INCREMENTAL=1` makes.

## Native linking is cached only where the linker can be described

Native binaries and dynamic libraries link against an external linker, startup
objects, and system libraries that rustc's
[dep-info](/how-it-works#prediction-and-dep-info) does not list.

### WebAssembly links

WebAssembly needs nothing extra. A binary, test, or `cdylib` for one of these
built-in targets uses its compiler-bundled self-contained linker, so mbx caches
it on every platform:

- `wasm32-unknown-unknown`
- `wasm32-wasip1` and `wasm32-wasip1-threads`
- `wasm32-wasip2`
- `wasm32v1-none`
- `wasm64-unknown-unknown`

mbx caches those links because all explicit artifacts are modeled inputs and
the linker, CRT objects, and bundled libc are covered by the Rust toolchain
identity. A WebAssembly link still bypasses when it uses:

- A custom target specification or an external WebAssembly toolchain
- A native library
- Any `-C linker` override
- Any `-C link-arg` or `-C link-args`, including `-fuse-ld`
- Disabled WASI CRT bundling
- A non-affirmative `link-self-contained` mode

### Host links

mbx caches host test binaries, executables, and proc macros on Linux, macOS,
and Windows by putting the rest of the link into the key:

| Host | The key also identifies |
| --- | --- |
| Linux | The resolved `cc` driver and its version, the linker it selects, and the startup objects and libc it resolves (hashed) |
| macOS | The resolved `cc` driver and its version, the linker it selects, and the SDK |
| Windows | `link.exe` or `lld-link`, the MSVC toolset and Windows SDK versions, and the selected VC and Universal CRT libraries |

Two hosts that differ in any of those produce different keys and miss.
[`cache_links`](/configuration#cache-links) (`MBX_CACHE_LINKS=0`) turns
native-link caching off.

A host `cdylib` is cached the same way on Linux. On macOS a `cdylib` records
its own output path as its install name, and on Windows it leaves an import
library beside the DLL, so it bypasses on both.

Some hosts cannot be described. They get no linker identity, so none of their
native links are cached:

- On Linux, mbx asks the driver to place a startup object and a libc. A host
  where either one does not resolve is refused.
- On macOS, a host is refused when `xcrun` cannot report the SDK's version,
  its build version, or (when `SDKROOT` is unset) its path.
- On Windows, a host is refused when mbx cannot identify the MSVC toolset and
  Windows SDK versions, or cannot find both the VC and Universal CRT libraries.

The same goes for a driver that names no linker or reports no version. mbx
refuses these hosts rather than leaving the missing input out of the key: two
hosts failing the same probe would otherwise agree on a key without either
having pinned what it stood for. Those links appear in `mbx explain` like any
other bypass.

### Linker overrides and link arguments

Even on a host mbx can describe, a link bypasses when it:

- Names a native library (`-l`, which a build script emits as
  `cargo:rustc-link-lib`)
- Selects a linker mbx cannot identify
- Carries a flag that would embed this checkout's paths (`-Crpath`,
  `-Cprefer-dynamic`)
- Carries a flag that would leave a file beside the binary that mbx does not
  store (`-Csplit-debuginfo=packed`, or `unpacked` outside macOS)
- Passes an explicit `--target`, even one that spells the host triple: rustc
  without one links for the host, and that is the only linker mbx identifies

A `-C linker` override is identified rather than refused when its file name is
`clang` or `clang++` (on Windows, `link.exe` or `lld-link`). Any other
`-C linker` bypasses. On Linux and macOS, mbx also models
`-C link-arg=-fuse-ld=<name>`: it resolves the `ld.<name>` the driver would
run, or takes an absolute path as given, and puts that linker's path and
version into the key. [Managed linkers](/linkers) route native links this way,
so those links stay cacheable.

Most other `-C link-arg` values bypass, because their text cannot say whether
they name a file the key would need to hash. A few options read no file, so
they enter the key as text:

- **Linux:** `-Wl,-z,` followed by `defs`, `lazy`, `nodelete`, `nodlopen`,
  `noexecstack`, `norelro`, `now`, `origin`, or `relro`. Each sets a flag in
  the output and reads nothing. Node-API addons built with napi-build pass
  `-z nodelete` and cache this way.
- **Windows MSVC:** `-C link-arg=/STACK:<size>` (or `/STACK:<reserve>,<commit>`)
  and `/Brepro`. A `rustflags` entry such as `-C link-arg=/STACK:8000000`
  reaches every proc macro, build script, and binary in a workspace, so
  refusing it would leave all of them linking on every build. An MSVC link is
  not reproducible, so each rebuilt proc-macro DLL would hash differently and
  make every crate that uses it miss. Other options, such as `/DEF:` or
  `/LIBPATH:`, can name a file and still bypass.

macOS needs one adjustment of its own. A debug-info link records absolute
object paths and their timestamps in the binary's debug map, so mbx passes
ld64 `-oso_prefix` for the build's target directory, which lets those links
cache. The same prefix covers `-Csplit-debuginfo=unpacked`, which keeps debug
information in the objects rather than beside the binary.

## Native libraries are inputs where nothing links

A library compilation runs no linker, so a `-l` flag on it is not a linker
argument. `-sys` crates whose build script emits `cargo:rustc-link-lib` (for
example `zstd-sys`, `ring`, `aws-lc-sys`, `libz-sys`, and `openssl-sys` when
linking statically) compile their rlib this way, and mbx caches those
compilations.

For `-l static=NAME`, rustc reads the archive and bundles it into the rlib.
mbx resolves the file the way rustc does: it takes the first `-L native`
directory, in command-line order, that holds `libNAME.a` (`NAME.lib` on Windows
MSVC and UEFI targets, or the literal name with `+verbatim`). mbx hashes that
archive into the action key exactly like an `--extern` artifact, so a rebuilt
archive with the same name gives the library a new key, on a fresh compilation
and on a predicted restore alike. If no search directory holds the archive, the
compilation bypasses as `missing-native-library`.

Only `-L native=` and `-L dependency=` search paths are modeled. A build script
that emits `cargo:rustc-link-search=PATH` with no kind (which rustc reads as
`all`), or with `all=`, `crate=`, or `framework=`, hands rustc a search path
mbx cannot describe. Cargo passes that path to the package's own compilations
and to every crate that depends on it, and each of those compilations bypasses
as `unsupported-search-path`, even when it links nothing. Write the directive
as `cargo:rustc-link-search=native=PATH` to avoid this bypass.

Explicit external `-L native` directories, including Homebrew installations,
are tracked by their location and file contents. Predictions rescan those
directories, so changing, adding, or removing an archive invalidates reuse even
when the library is named by a source-level `#[link]` attribute. Regular-file
symlinks within the searched tree are hashed through their referents; directory
symlinks within the tree, dangling links, and file links escaping it still
bypass. The search root itself may be a symlink, such as a Homebrew `opt` path.
Existing directory size and input-count limits still apply. Installation paths
remain part of the key, so different installations do not share artifacts just
because their files match.

A workspace crate that bypasses for one of these reasons, such as a dependent of
a `-sys` crate that found a system library through pkg-config, still gets
[learned incremental reuse](/incremental#learned-incremental-reuse): its first
source edit switches it to learned incremental state. Only its result stays out
of the shared cache.

A custom target specification names its archives its own way, so a `-l static`
compilation for one bypasses as `custom-target-native-library` unless the flag
is `+verbatim`. A bare target name counts as a custom target when a
specification file for it exists under `RUST_TARGET_PATH` or in the toolchain's
`lib/rustlib` directory.

For `-l static:-bundle=NAME`, `-l dylib=NAME`, `-l framework=NAME`,
`-l link-arg=...`, and a plain `-l NAME`, rustc reads nothing and records the
name in the crate's metadata for a later link, so the flag enters the key as
text.

An archive built with debug information usually records the checkout's C
source paths. When the build script that produces it reruns in another
checkout, the archive differs and so does the library's key. When
[build-script execution](#build-script-execution-follows-cargo-s-freshness-inputs)
restores the archive from the cache instead, the bytes match and the library
hits there too. Either way the library is reused across builds at the same
path, including after the target directory is removed.

Apple's `ar` and `ranlib` also stamp the current time into an archive. The `cc`
crate sets `ZERO_AR_DATE` for the archivers it knows, but a CMake-based `-sys`
crate runs `ar` directly, so rebuilding unchanged objects still moves the
archive's digest and every crate that depends on it misses. mbx sets
`ZERO_AR_DATE=1` for the build scripts it runs, which makes those tools write
zeros instead. The [`ar_determinism`](/configuration#ar-determinism) setting
(`MBX_AR_DETERMINISM`) chooses when:

- `auto` (the default) covers every build whose Cargo `PROFILE` is not
  `release`, so `--release` builds and profiles that inherit from `release`,
  such as `bench`, stay byte-for-byte as the toolchain made them.
- `always` covers those builds too.
- `off` leaves the archive tools alone.

A `ZERO_AR_DATE` you set yourself always wins. mbx sets the variable through
its build-script wrapper, so the setting has no effect on build scripts
compiled while
[build-script execution](#build-script-execution-follows-cargo-s-freshness-inputs)
is off. Under the default, a macOS release or bench build with CMake-built
native dependencies can still miss downstream of those archives;
`mbx settings set ar_determinism always` covers it. The `archives` check in
`mbx doctor` reports whether this host's archive tools stamp a timestamp.

Linked programs, tests, and proc macros that name a native library still
bypass; see [Linker overrides and link arguments](#linker-overrides-and-link-arguments).

## Restored artifacts are equivalent, not always identical

On restore, mbx rewrites dep-info and diagnostics to use the current checkout
and target paths. Cargo can then read them as if the compilation ran locally.

The compiled artifacts are reused as they were produced, and a few things can
make them differ from what a fresh compilation here would have written. rustc
records absolute source paths in metadata and debug information, so artifacts
built from two checkouts differ even when the sources are identical.

A C or C++ object compiled with debug information records the directory the
compiler ran in. For GCC and Clang, mbx passes `-fdebug-prefix-map` for that
directory, so the object records a placeholder instead and matches a fresh
compilation here. An MSVC object keeps the real directory; like any C or C++
object that still embeds a checkout path, it is reused only at that path.

A C or C++ object also records the absolute include directories it was given,
so a `-sys` crate whose build script generates headers into `OUT_DIR` would
otherwise produce a different object in every target directory. With
[`OUT_DIR` sharing](/configuration#share-out-dir) on (the default), mbx also
passes the compiler `-fdebug-prefix-map` for that directory, so the object
records the same placeholder the key does and two target directories produce
the same bytes.

An object that still embeds a checkout or target path is stored under a
checkout-specific key by default. `mbx settings set cc_store_path_specific
false` (or `MBX_CC_STORE_PATH_SPECIFIC=0`) skips storing those objects; existing
entries remain readable. Windows debug information can also record the object
output path.

These path differences can affect debugging and byte-for-byte comparisons.
`MBX_VERIFY=1` reports them as divergences, but a divergence alone does not
establish its cause. Inspect the named output and mismatch before attributing it
to paths, and report unexplained differences. See
[Verify restored outputs](/troubleshooting#verify-mode) for a controlled
comparison and
[Debug a binary restored from another checkout](/cookbook/local-development#debug-a-binary-restored-from-another-checkout)
for a build using local source paths.

## C and C++ caching covers the host compilations mbx drives {#c-and-c-caching-covers-the-host-compiles-mbx-drives}

mbx caches C and C++ compilations from three places:

- Build scripts that compile for the host through the `cc` crate
- Build scripts that run CMake through the `cmake` crate
- Commands run under [`mbx exec`](/standalone-builds), which puts shims for the
  plain driver names on `PATH` for that command alone

When a `cmake` crate configure names one of mbx's compiler shims as its C or
C++ compiler, mbx's CMake wrapper gives CMake the real compiler and adds a
compiler launcher. CMake's C and C++ compilations are then cached for the same
compilers as the `cc` crate's.

A compilation outside these three places is not reached. mbx also leaves
alone a cross-compilation the build did not name a compiler for. In a Cargo
build, mbx installs its shims as `HOST_CC` and `HOST_CXX`, which the `cc`
crate reads only when host and target match. `mbx exec` shims only `cc`,
`c++`, `gcc`, `g++`, `clang`, and `clang++` on Unix, plus `cl.exe` on Windows,
so a versioned compiler such as `gcc-13` stays with the build that chose it.
A CMake configure run directly by `mbx exec` is the exception: its compiler
launchers cache whichever C and C++ compiler CMake uses.

A cross-compilation is cached when the build names its own compiler through
`CC_<target>`, `CXX_<target>`, `TARGET_CC`, or `TARGET_CXX`: mbx wraps what was
named. A cross-compiling build that names nothing is left alone, because which
compiler a target implies lives in the `cc` crate's own tables, and a wrong
guess would build the object with the wrong compiler. A value that is a
command, such as `ccache gcc`, is left alone for the same reason.

### Supported compiler calls

mbx caches single-source object compilations through GCC-, Clang-, and
MSVC-style drivers. Preprocessed assembly (`.S` or `-x assembler-with-cpp`) is
supported: its includes participate in dependency discovery, and the assembler
is part of the GCC toolchain identity. Known assembler options that add no
inputs, such as `-Wa,--noexecstack`, are also supported.

GCC/Clang preprocessing with `-E ... -o file` is cached too. The file is
restored byte-for-byte, preserving line markers and `__FILE__` values. These
entries put literal argument paths, the working directory, and mapped roots in
their keys, so they cannot be shared across different checkout paths. Header
discovery and include-directory invalidation still apply.

With `-E` and `-MD`/`-MMD`, an explicit `-MF` is required. Without it, GCC gives
`-o` a different meaning and mbx bypasses the invocation.

### Calls that bypass the cache

mbx leaves these C and C++ calls uncached:

- Links, multi-source calls, preprocessing to stdout, and dependency output
  to stdout (`-MF -`)
- Assembly without preprocessing (`.s`), Objective-C, precompiled headers,
  coverage instrumentation, compiler plugins, and response files
- Unmodeled sub-tool options, including `-Wp,`, `-Wl,`, `-Xclang`, and
  unsupported `-Wa,` options
- Assembler-time `.include`, `.sinclude`, and `.incbin` inputs
- MSVC compiler PDBs, modules, and other unsupported extra outputs
- Sources or headers that expand `__DATE__`, `__TIME__`, or `__TIMESTAMP__`,
  because the result depends on when compilation runs
- Host CPU tuning such as `-march=native`, because the cache key does not
  identify the host processor
- Any other flag or input mbx cannot model

Setting `CC`, `CXX`, `HOST_CC`, or `HOST_CXX` leaves the selected host compiler
outside mbx's compiler shims. A target compiler named through `CC_<target>`,
`CXX_<target>`, `TARGET_CC`, or `TARGET_CXX` is still wrapped unless its value
is a command; see
[C and C++ caching covers the host compilations mbx drives](#c-and-c-caching-covers-the-host-compiles-mbx-drives).
`MBX_CC=0` disables C and C++ caching entirely.

## Shadowing is modeled by name, not by content

Each directory outside the system roots that a C or C++ compilation searches
for headers adds a manifest of its file names to the key. A header that newly
shadows one the compilation read therefore changes the key, even when every
file it read is unchanged.

The manifest lists the names that could answer an `#include`: headers, sources
(`#include "generated.c"` is unusual but legal), names without an extension,
and precompiled headers, which GCC prefers over the header they were built from
without anything on the command line saying so.

Objects, dependency files, and archives are left out, because they cannot
answer an `#include`. A build writes those into the directory a generated
header lives in, and counting them would make the key depend on how many
sibling compilations had finished.

mbx takes manifests once before the compiler runs and again before publishing.
If a search directory changed in between, the compilation bypasses: a header
that appeared while the compiler ran is one it never saw, and the key would
otherwise claim a state that did not produce this object.

System roots are exempt from manifests. Enumerating an SDK on every compilation
costs more than the risk, and anything read from one is digested like any other
input.

<span id="out-dir-sharing-remaps-generated-source-paths"></span>
<span id="out-dir-sharing-copies-generated-sources-under-the-cache"></span>

## Share compilations that read `OUT_DIR` {#out-dir-sharing}

By default, mbx can reuse Rust compilations across checkouts when their
build-script output matches. This includes crates that load generated code:

```rust
include!(concat!(env!("OUT_DIR"), "/generated.rs"));
```

Cargo normally places `OUT_DIR` under each checkout's target directory. That
path becomes a cache-key input when a crate reads it, causing a miss in a new
checkout even if `generated.rs` is identical. mbx copies the output to
`out-dirs/v1/<digest>` under the cache directory and gives rustc that shared
path as `OUT_DIR`. The digest covers the directory layout, file names,
contents, and executable bits. Matching output trees therefore use the same
path and can reuse the compilation when its other inputs also match.

The literal `OUT_DIR` value remains part of the key. Code that stores the path
or derives a value from it sees the same value in both checkouts.

### When sharing is unavailable

- **Generated output differs.** A build script that embeds a checkout path in
  its output produces a different tree and cache key for each checkout.
- **The source scan misses the reference.** mbx scans Rust files beneath the
  compiler's input file directory. References found only in external sources,
  skipped directories, or beyond the scan limit may leave rustc using Cargo's
  original `OUT_DIR`.
- **The output cannot be copied.** Trees containing symlinks or unsupported
  entries are left in place. Copy errors also fall back to Cargo's `OUT_DIR`.
- **Cache locations differ.** Reuse across machines requires the same absolute
  cache directory path, as well as matching output and other compilation inputs.

### Compatibility and cleanup

For eligible compilations, `env!("OUT_DIR")` names the cached copy. Its files
and directories are marked read-only; code that needs to modify generated
output during compilation should disable sharing. Build scripts still write
their output to the directory Cargo gives them before mbx makes the copy.

`mbx cache stats` reports these copies as **generated source trees**. Automatic
collection and `mbx gc` remove copies according to `target.max_age`, based on
when mbx last used them for a compilation or cache lookup. The copies share
`gc.incremental_max_size` with learned incremental state, least recently used
first, and the remaining copies count toward `gc.max_total_size`. A compilation
holds a lease on the copy it reads until rustc exits, and collection leaves a
leased copy in place. A build Cargo considers fresh does not refresh the use
timestamp. mbx recreates an evicted copy when a later compilation needs it, so
an embedded `OUT_DIR` path should not be treated as permanent runtime storage.

To preserve Cargo's original `OUT_DIR` and literal generated source paths:

```sh
MBX_SHARE_OUT_DIR=0 mbx build
```

Or set `share_out_dir = false` in the workspace's `.mbx.toml`. Rust
compilations that read `OUT_DIR` then remain checkout-specific.

The setting also controls generated source path remapping in debug information:
`--remap-path-prefix` for Rust and `-fdebug-prefix-map` for C/C++. C and C++
compilations use the original build-script output directory; only rustc
receives the shared copy.

## A rebuilt workspace crate records its checkout

Cargo runs rustc with the crate's own directory as the working directory, and
rustc stores that directory in the artifact. Two checkouts of the same commit
therefore produce different bytes for the same workspace crate whenever it is
compiled rather than restored, and every crate that depends on it misses too,
because one of its inputs differs.

Most builds never see this: a workspace crate that can be restored is restored,
byte for byte. The problem appears when a crate's key includes its checkout
path, so every checkout must compile it. One crate low in the graph that reads
`CARGO_MANIFEST_DIR`, or reads `OUT_DIR` without sharing, can cause much of a
large workspace to rebuild.

Set `MBX_SHARE_WORKSPACE_ROOT=1` (or `share_workspace_root = true`) to map the
workspace root to a placeholder, so the two checkouts produce the same bytes.
The crate that read the path is still keyed to its checkout, but its dependents
now get the same keys in every checkout and can hit the cache. The cost is that
source paths under the workspace are recorded as the placeholder in debug
information, `file!()`, and panic locations, which is why the setting is off
by default.

<span id="collection-is-approximate"></span>
For eviction order and what the action-store budget covers, see
[Collection is approximate](/managed-targets#collection-is-approximate).
