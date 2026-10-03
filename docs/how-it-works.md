---
description: Follow a Cargo build through compiler shims, portable action keys, output restoration, and shared scheduling.
---
# How it works

mbx works at the compiler boundary. Cargo decides which tools need to run;
mbx decides whether each eligible invocation can be restored from cached work.
An **action** is one modeled invocation and its inputs. The **store** is
content-addressed: it holds outputs under digests of their content.

Run `mbx build` directly, or use ordinary `cargo build` after
[enabling automatic wrapping](/setup).
Both follow the same build lifecycle.

## From command to result

1. mbx resolves the workspace and target roots through Cargo metadata.
2. It starts an in-process cache agent and creates shims for the build. The
   cache agent and shims of one mbx command make up its **build session**.
3. Cargo runs normally with the rustc shim set as `RUSTC_WRAPPER`, and build
   scripts inherit `HOST_CC` and `HOST_CXX` pointing at the C and C++ shims.
4. Each shim analyzes its compiler invocation and derives a content-addressed
   action key.
5. A hit restores the action's outputs; a miss runs the real compiler and
   publishes the result. The shim forwards the compiler's diagnostics and
   artifact notifications to Cargo as rustc prints them, so Cargo starts a
   crate's dependents against its metadata while code generation and
   publication continue, the same as without mbx. To diagnose the shim, set
   `forward_compiler_notifications = false`
   (`MBX_FORWARD_COMPILER_NOTIFICATIONS=0`) to hold that output until the
   result is stored.
6. The cache agent exits with the build, draining any remote uploads it still
   owes. There is no persistent daemon.

Because every compiler call passes through a shim, mbx can also coordinate
several Cargo builds running at once. Their compiler shims share a permit pool
and an in-flight-work registry under the cache directory, so those builds do
not multiply the machine's CPU and memory budgets or repeat an identical cold
compilation. [Machine-wide scheduling](#machine-wide-scheduling) describes the
mechanism. For ready-to-use recipes, see the
[mise task example](/scheduling#run-independent-tasks) and the
[parallel GitHub Actions example](/github-action#parallel-cargo-steps).

## Build-script C and C++

Cargo has no `CC_WRAPPER`, so the shims arrive as compiler variables
themselves, resolved to the platform compilers when the build session starts.
mbx sets them as `HOST_CC` and `HOST_CXX` rather than `CC` and `CXX`. The `cc`
crate consults the host pair only when it is not cross-compiling, and these
shims wrap the host compiler, so a `cargo build --target` keeps the
cross-compiler it would have found on its own. mbx leaves an explicit host
compiler in `CC`, `CXX`, `HOST_CC`, or `HOST_CXX` alone. Explicit target
compilers can be wrapped; see the
[C and C++ limits](/limits#c-and-c-caching-covers-the-host-compiles-mbx-drives).
`MBX_CC=0` turns this caching off.

The C shim requests a private dependency list and keys the compilation on the
files it names. A cold call may need to compile before that list is known; the
stored result and [prediction](#prediction-and-dep-info) can serve later builds.

If the build requests its own depfile with `-MD` or `-MMD`, mbx writes one on
both a compilation and a cache hit. Depfile formatting flags affect that file
without changing the cache key.

The key also records include-directory names that could change which header
an `#include` resolves to. Adding a header that shadows an existing one must
invalidate the result even if every previously read file is unchanged. See
[include shadowing](/limits#shadowing-is-modeled-by-name-not-by-content) for the
manifest rules.

## Portable keys

mbx maps known workspace, target, Cargo registry, toolchain, and sysroot paths
to stable placeholders before they enter a key. That is what lets equivalent
worktrees share an action even though their absolute paths differ.

The key also covers compiler inputs and relevant environment. If mbx cannot
model something exactly, it bypasses the action.

## Prediction and dep-info

A rustc action key depends on the files that compilation actually reads. mbx
learns that set from the dep-info files Cargo and rustc wrote in an earlier
build, which list the files each compilation read. It records the set as a
**prediction** for later invocations. A cold compilation may therefore have no
key to look up yet. mbx still stores it after compiling, so it can warm the
next build.

Predictions are grouped by the `Cargo.lock` digest. When a dependency update
creates a new group, mbx looks for earlier predictions in up to eight lockfile
states from Git history, then in recent local store records. Each borrowed
prediction is checked against the current inputs: unchanged crates can hit,
while changed crates compile and record new predictions.

mbx saves the borrowed record under the new lockfile for subsequent commands,
and eligible trusted CI builds publish it remotely. A shallow clone limits
which history is available. For GitHub Actions' default pull-request merge
checkout, `fetch-depth: 2` includes the base parent; a depth-one checkout relies
on local store records.

### Shared file hashing

The cache agent also shares the work of hashing the files each compilation
reads. It keeps a ledger of every file a shim has hashed, keyed by the file's
length, modification time, and change time, so a dependency's rlib is read
once however many crates link it. The ledger is saved with the checkout's
private state when the build finishes and loaded by its next build, avoiding
repeated reads of unchanged dependencies. Reuse depends on the file-identity
checks supported by that filesystem; network filesystems receive additional
content validation. If an entry cannot be validated, mbx hashes the file again.

## Documentation actions {#rustdoc-actions}

Cargo invokes rustdoc through a separate shim. Each documentation action keys
the rustdoc version and arguments, the package source tree, explicit compiler
artifacts, and Cargo's compile-time environment. In mergeable-output mode,
rustdoc separates deterministic per-crate pages from files, such as the search
index, that combine every documented crate. mbx caches the per-crate pages
with the crate's merge metadata. After restoring them, it runs rustdoc's
inexpensive finalization step, so cached dependency documentation remains
composable and the shared indexes do not depend on restore order.

## Output restoration

The cache agent verifies each local store object against its digest before
returning it to the compiler shim. The shim stages the output in a directory
beside Cargo's destination and atomically renames it into place. It uses the
first of these methods that works:

1. **Reflink.** The output is an ordinary file that shares its data blocks
   with the store object until either is written, so Cargo sees the complete
   output immediately without the shim re-reading or allocating all of its
   data after verification. Reflinks require support from the filesystem and
   generally require the store and the target directory to be on the same
   filesystem.
2. **Hard link.** Where cloning is unavailable and
   [`restore_hardlink`](/configuration#restore-hardlink) is enabled, which it
   is by default, mbx hard links the store object into place. The output is
   then the store object itself; see
   [Hard-linked outputs](#hard-linked-outputs).
3. **Copy.** mbx copies the bytes when `restore_hardlink` is off, when it can
   neither clone nor link, and when the output needs a mode the store object
   cannot carry (see [Hard-linked outputs](#hard-linked-outputs)).

None of these methods produces a placeholder or relies on a userspace
on-demand filesystem: every restored output is a complete, regular file in the
target directory before Cargo sees it. A reflinked or copied output is a file
of its own, carrying the mode its compilation recorded, and writing to it never
reaches the store.

Most Linux CI runners and many Linux developer machines use ext4, which has no
clone support at all, so on those machines linking is what keeps a warm
restore from writing every cached byte. Windows never hard links a restored
output: a read-only file there cannot be deleted until its attribute is
cleared, and the store object relies on staying read-only. Without ReFS
cloning, restores on Windows copy whatever `restore_hardlink` says.

The `full` [build summary](/configuration#build-summaries) (`summary = "full"`
or `MBX_SUMMARY=full`) reports the file count and logical size each method
handled. `MBX_STATS_REPORT` includes the same values as
`reflinked_output_files`, `reflinked_output_bytes`, `hardlinked_output_files`,
`hardlinked_output_bytes`, `copied_output_files`, and `copied_output_bytes`.
An output whose destination already holds the same bytes stays where it is;
mbx refreshes the modification time of a Rust output kept this way. The
summary counts it as `already in place`, and the report as
`reused_output_files` and `reused_output_bytes`.
`mbx doctor` reports which of the three a restore to a given target directory
will use. On Windows, a hard link in that report means a copy, since restores
there never hard link.

### Hard-linked outputs

A hard link is another name for the store object rather than a copy of it, so
it shares that object's mode and modification time with every other path
linked to it. mbx makes the object read-only before linking to it, so the
filesystem refuses a compiler that would overwrite a restored output instead
of letting it rewrite bytes every other checkout shares. mbx unlinks such an
output before it runs a real compiler, so rebuilding through mbx
is unaffected.

Running Cargo without mbx (for example with `MBX_DISABLE=1`) in a target
directory mbx filled can report `output file ... is not writeable` for a unit
Cargo decides to rebuild. To resolve it, run `mbx clean` (for a managed target
directory) or `cargo clean`, or remove the file. `restore_hardlink = false`
avoids the error by giving every restored output a file of its own. The error
occurs only where cloning is unavailable, since a reflinked output is already
independent of the store.

Because the object's mode is the mode of every path linked to it, mbx links an
output only where that mode can serve it. It copies the output instead when:

- the output is executable and the object is not, or the reverse
- the output needs read permissions the object does not already carry, since
  granting them would widen every other link
- the output's recorded mode is unreadable, which would leave the object
  itself unopenable

Restores of one object may take readership away (the owner running the build
still reads it) but never add it, and never change whether the object
may be run.

## Machine-wide scheduling

Every compiler mbx starts takes a permit from one pool shared by every build
on the machine that uses the same cache directory, so simultaneous builds do
not multiply the machine's CPU and memory budgets. Cache hits never wait, and
Cargo keeps its own dependency scheduling. If a process dies, the kernel
releases its permits, so a crashed build cannot wedge its siblings.

Concurrent builds also stop repeating each other. Separate CI jobs with fresh
targets can otherwise repeat the same dependency compilations. With mbx, a
compilation identical to one already running against the same cache directory
waits for that one to finish and restores its result from the cache. The
finished compilation also leaves its input list behind, so a job arriving
after it is already done can build the cache key it would otherwise lack and
hit where it would have compiled cold. Both paths rehash every input before
trusting anything, so the worst a stale record can do is fall back
to compiling.

Permits are weighted by memory. Native links start at two permits, and every
compilation is thereafter weighted by what it actually used, so admission uses
the estimated memory cost of the running work. This is a scheduling estimate,
not an operating-system memory limit. A link that turns out to fit in one
permit stops being charged for two, which keeps the link-heavy tail of a build
from running at half concurrency. A link mbx has never seen is weighed by the
heaviest of this machine's recent links. That matters for tests: each test
binary has its own crate name, so a cold `cargo test --no-run` has no
per-crate history for the links in front of it. A compilation the Linux OOM
killer stops is recorded heavier than it measured, so its retry runs with more
room. Compiler memory use is measured only on Unix. On Windows, native links
keep their two-permit weight and other compilations take one permit.

The pool size, memory budget, and priority are settings; see
[machine-wide compile scheduling](/scheduling#machine-wide-compile-scheduling).

## What remains local

Cargo's target state belongs to a workspace. Learned incremental state belongs
to a checkout and never enters the shared cache. A configured remote receives
eligible shared actions according to the
[write policy](/remote-cache#read-and-write-policy).
[Managed target directories](/managed-targets) and
[collection](/managed-targets#collection) control local retention.

## Correctness first

Unsupported crate types, unmodeled search paths, and compilations under
[Cargo incremental mode](/incremental#cargo-incremental-mode) bypass the shared
cache. Compilations that keep private incremental state, through
[learned](/incremental#learned-incremental-reuse) or
[eager incremental reuse](/incremental#eager-incremental-reuse), are never
published either, but the build summary counts them as `incremental`, not as
bypasses.

A compilation that links nothing is cached whatever its crate type:
`cargo check` and `cargo clippy` compile every binary and test target that
way. mbx caches a native link only when it can describe the linker:

- host binaries, tests, and proc macros on Linux, macOS, and Windows, and host
  `cdylib`s on Linux, with the resolved linker, startup objects, C runtime,
  and SDK in the key
- a fixed allowlist of built-in WebAssembly targets whose default linker and
  system inputs ship with rustc

Everything else (other native libraries, custom linkers, and unrecognized
toolchains) links as it always did; see
[Caching limits](/limits#native-linking-is-cached-only-where-the-linker-can-be-described).

A library compilation that names a native library is cached. It runs no
linker, so a `-l static` archive is hashed into its key like an `--extern`
artifact, and other `-l` kinds are keyed by name; see
[Caching limits](/limits#native-libraries-are-inputs-where-nothing-links).

`MBX_VERIFY=1` compiles while also consulting the cache and compares the
results. It is expensive, so use it to investigate correctness rather than for
everyday builds; see [Verify restored outputs](/troubleshooting#verify-mode).
