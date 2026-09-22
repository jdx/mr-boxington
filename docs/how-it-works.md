---
description: Follow a Cargo build through compiler shims, portable action keys, output restoration, and shared scheduling.
---
# How it works

mbx works at the compiler boundary. Cargo decides which tools need to run;
mbx decides whether each eligible invocation can be restored from cached work.
An **action** is one modeled invocation and its inputs. The **content-addressed
store** (CAS) holds outputs under digests of their content.

Run `mbx build` directly, or use ordinary `cargo build` after
[enabling automatic wrapping](/setup).
Both follow the same build lifecycle.

## From command to result

1. mbx resolves the workspace and target roots through Cargo metadata.
2. It starts an in-process cache agent and creates shims for the build.
3. Cargo runs normally with the rustc shim set as `RUSTC_WRAPPER`, and build
   scripts inherit `HOST_CC` and `HOST_CXX` pointing at the C and C++ shims.
4. Each shim analyzes its compiler invocation and derives a content-addressed action key.
5. A hit restores the action's outputs; a miss runs the real compiler and publishes the result.
   The compiler's diagnostics and artifact notifications reach Cargo as rustc
   prints them, so Cargo starts a crate's dependents against its metadata while
   code generation and publication continue, the same as without mbx.
   `forward_compiler_notifications = false` (`MBX_FORWARD_COMPILER_NOTIFICATIONS=0`)
   holds the output until the result is stored, for diagnosing the shim.
6. The agent exits with the build, draining any remote uploads it still owes.
   There is no persistent daemon.

That wrapper boundary also covers multiple Cargo builds running at the same
time. Their compiler shims share a machine-wide permit pool and an
in-flight-work registry, so those builds do not multiply the machine's CPU and
memory budgets or repeat an identical cold compilation.
[Machine-wide scheduling](#machine-wide-scheduling) below describes the
mechanism, and the
[mise task example](/scheduling#run-independent-tasks)
and
[parallel GitHub Actions example](/github-action#parallel-cargo-steps) are
ready-to-use recipes.

## Build-script C and C++

Cargo has no `CC_WRAPPER`, so the shims arrive as compiler variables
themselves, resolved to the platform compilers when the session starts. They
are set as `HOST_CC` and `HOST_CXX` rather than `CC` and `CXX`: the `cc` crate
consults the host pair only when it is not cross-compiling, and these shims
wrap the host compiler, so a `cargo build --target` keeps the cross compiler it
would have found on its own. An explicit host compiler in `CC`, `CXX`, `HOST_CC`, or `HOST_CXX` is left
alone. Explicit target compilers can be wrapped; see the
[C and C++ limits](/limits#c-and-c-caching-covers-the-host-compiles-mbx-drives).
`MBX_CC=0` turns this caching off.

The C shim requests a private dependency list and keys the compilation on the
files it names. A cold call may need to compile before that list is known;
the stored result and prediction can serve later builds.

If the build requests its own depfile with `-MD` or `-MMD`, mbx writes one on
both a compilation and a cache hit. Depfile formatting flags affect that file
without changing the object-cache key.

The key also records include-directory names that could change which header
an `#include` resolves to. Adding a header that shadows an existing one must
invalidate the result even if every previously read file is unchanged. See
[include shadowing](/limits#shadowing-is-modeled-by-name-not-by-content) for the
manifest rules.

## Portable keys

Known workspace, target, Cargo registry, toolchain, and sysroot paths are mapped
to stable placeholders before they enter a key. That is what lets equivalent
worktrees share an action even though their absolute paths differ.

The key also covers compiler inputs and relevant environment. If mbx cannot
model something exactly, it bypasses the action.

## Prediction and dep-info

A rustc action key depends on the files that compilation actually reads. mbx
learns that set from Cargo/rustc dep-info left by an earlier build and records a
prediction for later invocations. A cold compilation may therefore have no key
to look up yet. It still gets stored after compiling and can warm the next
build.

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

Hashing those files is shared too. The agent keeps a ledger of every file a
shim has hashed, keyed by the file's length, modification time, and change
time, so a dependency's rlib is read once however many crates link it. The
ledger is saved with the checkout's private state when the build finishes and
loaded by its next build, avoiding repeated reads of unchanged dependencies.
Reuse depends on the file-identity checks supported by that filesystem; network
filesystems receive additional content validation. If an entry cannot be
validated, mbx hashes the file again.

## Rustdoc actions

Cargo invokes rustdoc through a separate shim. Each documentation action keys
the rustdoc version and arguments, the package source tree, explicit compiler
artifacts, and Cargo's compile-time environment. Rustdoc's mergeable-output
mode separates deterministic per-crate pages from files such as the search
index that combine every documented crate. mbx caches the former with the
crate's merge metadata and runs rustdoc's inexpensive finalization step after
restoring them, so cached dependency documentation remains composable and the
shared indexes do not depend on restore order.

## Output restoration

The cache agent verifies each local CAS blob against its digest before returning
it to the rustc wrapper. The wrapper first tries to reflink that verified blob
into a staging directory beside Cargo's destination, applies the expected file
mode, and atomically renames it into place. A reflink is an ordinary file that
shares its data blocks with the CAS until either copy is written, so Cargo sees
the complete output immediately without the wrapper re-reading or allocating
all of its data after verification. Writes to a restored output cannot change
the CAS object.

Reflinks require support from the filesystem and generally require the cache
and target directory to be on the same filesystem. Where cloning is
unavailable and `restore_hardlink` is enabled, which it is by default, mbx
hard links the cached object into place instead; it copies the bytes when the
setting is off or when it can do neither. ext4, which most Linux CI runners
and many Linux developer machines use, has no clone support at all, so on
those machines linking is what keeps a warm restore from writing every cached
byte.

A hard link is the stored object rather than a copy of it, so mbx makes the
object read-only before linking to it: a compiler that would overwrite a
restored output is refused by the filesystem instead of rewriting bytes every
other checkout shares. mbx unlinks such an output before it runs a real
compiler, so rebuilding through mbx is unaffected. Running `cargo` directly in
a target directory mbx filled can report `output file ... is not writeable` for
a unit it decides to rebuild; `mbx clean` or removing the file resolves it, and
`restore_hardlink = false` avoids it by keeping every restored output a private
writable copy. Reflinked outputs are private copies already and are made
writable on restore, so this applies only where cloning is unavailable.

The session summary reports the file count and logical size handled by each
path; `MBX_STATS_REPORT` includes the same values as `reflinked_output_files`,
`reflinked_output_bytes`, `hardlinked_output_files`, `hardlinked_output_bytes`,
`copied_output_files`, and `copied_output_bytes`. `mbx doctor` reports which of
the three a restore to a given target directory will use.

This is filesystem copy-on-write, not a placeholder or userspace on-demand
filesystem. Restored paths retain normal file semantics on every supported
platform, including when mbx has to use the copy fallback.

## Machine-wide scheduling

Every compiler mbx starts takes a permit from one pool shared by every build
on the machine, so simultaneous builds do not multiply the machine's CPU and
memory budgets. Cache hits never wait, Cargo keeps its own dependency
scheduling, and permits are released by the kernel if a process dies, so a
crashed build cannot wedge its siblings.

Concurrent builds also stop repeating each other. Separate CI jobs with fresh
targets can otherwise repeat the same dependency compilations. With mbx, a
compilation identical to one already running against the same local cache waits
for that one to finish and restores its result from the cache. The finished
compilation also leaves its input list behind, so a job arriving after it is
already done can build the cache key it would otherwise lack and hit where it
would have compiled cold. Both paths rehash every input before trusting
anything, so the worst a stale record can do is fall back to compiling.

Permits are weighted by memory. Native links start at two permits, and every
compilation is thereafter weighted by what it actually used, so admission uses
the estimated memory cost of the running work. This is a scheduling estimate,
not an operating-system memory limit. A link that turns out to fit in one permit
stops being charged for two, which keeps the link-heavy tail of a build from
running at half concurrency. A link mbx has never seen is weighed by the
heaviest of this machine's recent links; test binaries each have their own crate
name, so a cold `cargo test --no-run` has no per-crate history for the links in
front of it. A compilation the Linux OOM killer stops is recorded heavier than
it measured, so its retry runs with more room.

The pool size, memory budget, and priority are settings; see
[machine-wide compile scheduling](/scheduling#machine-wide-compile-scheduling).

## What remains local

Cargo's target state belongs to a workspace. Learned incremental state belongs
to a checkout and never enters the shared cache. A configured remote receives
eligible shared actions according to the [write policy](/remote-cache#read-and-write-policy).
[Managed targets](/managed-targets) and garbage collection control local retention.

## Correctness first

Unsupported crate types, unmodeled search paths, and incremental compilations
bypass the shared action cache. That includes the private incremental state of
[learned incremental reuse](/incremental#learned-incremental-reuse), which is
never published. A compilation that links nothing is cached whatever its crate
type: `cargo check` and clippy compile every binary and test target that way.
A native link is admitted only when its linker can be described: host binaries,
tests, and proc macros on Linux, macOS, and Windows, where mbx puts the
resolved linker, startup objects, C runtime, and SDK into the key, and a fixed
allowlist of built-in WebAssembly targets whose default linker and system
inputs ship with rustc. Everything else (native libraries, custom linkers,
unrecognized toolchains) links as it always did; see
[limits](/limits#native-linking-is-cached-only-where-the-linker-can-be-described).
A library that names a native library is cached: nothing links, so a
`-l static` archive is hashed into its key like an `--extern` artifact, and
other `-l` kinds are keyed by name; see
[limits](/limits#native-libraries-are-inputs-where-nothing-links).
`MBX_VERIFY=1` compiles while also consulting the cache and compares the
result, an expensive qualification mode.
