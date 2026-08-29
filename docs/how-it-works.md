# How it works

mbx is a Cargo wrapper, not a Cargo replacement. Run Cargo subcommands directly
through it: `mbx build`, `mbx test`, `mbx clippy`, or any installed Cargo
subcommand.

1. mbx resolves the workspace and target roots through Cargo metadata.
2. It starts an in-process cache agent and creates shims for the build.
3. Cargo runs normally with the rustc shim set as `RUSTC_WRAPPER`, and build
   scripts inherit `HOST_CC` and `HOST_CXX` pointing at the C and C++ shims.
4. Each shim analyzes its compiler invocation and derives a content-addressed action key.
5. A hit restores the action's outputs; a miss runs the real compiler and publishes the result.
6. The agent exits with the build, draining any remote uploads it still owes.
   There is no persistent daemon.

Every mbx command works this way; there is no separate mode to turn on and no
component to keep up to date.

That wrapper boundary also covers multiple Cargo builds running at the same
time. Their compiler shims share a machine-wide permit pool and an
in-flight-work registry, so those builds do not multiply the machine's CPU and
memory budgets or repeat an identical cold compilation.
[Machine-wide scheduling](#machine-wide-scheduling) below describes the
mechanism, and the
[mise task example](/getting-started#run-multiple-cargo-builds-at-the-same-time)
and
[parallel GitHub Actions example](/github-action#parallel-cargo-steps) are
copyable shapes.

## Build-script C and C++

Cargo has no `CC_WRAPPER`, so the shims arrive as compiler variables
themselves, resolved to the platform compilers when the session starts. They
are set as `HOST_CC` and `HOST_CXX` rather than `CC` and `CXX`: the `cc` crate
consults the host pair only when it is not cross-compiling, and these shims
wrap the host compiler, so a `cargo build --target` keeps the cross compiler it
would have found on its own. A build that already chose a compiler through any
of those variables is left alone, and `MBX_CC=0` turns the shims off
entirely.

Unlike rustc, a C compile leaves no dependency record behind for a later build
to read, and publishing one would add a file the uncached build never produced.
So the shim asks for its own dependency list, keeps it private, and keys the
compilation on the files that list names. A cold compilation therefore has no
key to look up yet — it is stored after compiling and warms the next build. The
directories the compile searched also contribute a manifest of the names in
them that could answer an `#include`, so a header appearing where it would
*shadow* one that was read changes the key even though every file that was read
is unchanged; what a manifest counts and leaves out is covered in
[limits](/limits#shadowing-is-modeled-by-name-not-by-content).

## Portable keys

Known workspace, target, Cargo registry, toolchain, and sysroot paths are mapped
to stable placeholders before they enter a key. That is what lets equivalent
worktrees share an action even though their absolute paths differ.

The key also covers compiler inputs and relevant environment. If mbx cannot
model something exactly, it bypasses the action instead of guessing.

## Prediction and dep-info

A rustc action key depends on the files that compilation actually reads. mbx
learns that set from Cargo/rustc dep-info left by an earlier build and records a
prediction for later invocations. A genuinely cold compilation may therefore
have no key to look up yet. It still gets stored after compiling and can warm
the next build.

## Copy-on-write output restoration

The cache agent verifies each local CAS blob against its digest before returning
it to the rustc wrapper. The wrapper first tries to reflink that verified blob
into a staging directory beside Cargo's destination, applies the expected file
mode, and atomically renames it into place. A reflink is an ordinary file that
shares its data blocks with the CAS until either copy is written, so Cargo sees
the complete output immediately without the wrapper re-reading or allocating
all of its data after verification. Writes to a restored output cannot change
the CAS object.

Reflinks require support from the filesystem and generally require the cache
and target directory to be on the same filesystem. When cloning is unavailable,
mbx transparently copies the bytes instead. The session summary reports the
file count and logical size handled by each path; `MBX_STATS_REPORT` includes
the same values as `reflinked_output_files`, `reflinked_output_bytes`,
`copied_output_files`, and `copied_output_bytes`.

This is filesystem copy-on-write, not a placeholder or userspace on-demand
filesystem. Restored paths retain normal file semantics on every supported
platform, including when mbx has to use the copy fallback.

## Machine-wide scheduling

Every compiler mbx starts takes a permit from one pool shared by every build
on the machine, so simultaneous builds do not multiply the machine's CPU and
memory budgets. Cache hits never wait, Cargo keeps its own dependency
scheduling, and permits are released by the kernel if a process dies, so a
crashed build cannot wedge its siblings.

Concurrent builds also stop repeating each other. Four CI jobs building one
commit compile the same dependency graph four times; under the scheduler, a
compilation identical to one already running anywhere on the machine waits for
that one to finish and restores its result from the cache instead of burning a
core on it. The finished compilation also leaves its input list behind, so a
job arriving after it is already done can build the cache key it would
otherwise lack and hit where it would have compiled cold. Both paths rehash
every input before trusting anything, so the worst a stale record can do is
fall back to compiling.

Permits are weighted by memory. Native links start at two permits, and every
compilation is thereafter weighted by what it actually used, so the predicted
memory of everything running stays inside the budget; a link that turns out to
fit in one permit stops being charged for two, which is what keeps the
link-heavy tail of a build from running at half concurrency. A link mbx has
never seen is weighed by the heaviest of this machine's recent links —
guessing low costs an out-of-memory kill and guessing high costs a wait, and
test binaries make the case for remembering: each is its own crate name, so a
cold `cargo test --no-run` has no per-crate history for the links in front of
it. A compilation the Linux OOM killer stops is recorded heavier than it
measured, so its retry runs with more room instead of repeating the crash.

The pool size, memory budget, and priority are settings; see
[machine-wide compile scheduling](/configuration#machine-wide-compile-scheduling).

## Correctness first

Unsupported crate types, unmodeled search paths, and incremental compilations
bypass the shared action cache. A compilation that links nothing is cached
whatever its crate type — `cargo check` and clippy compile every binary and
test target that way, and metadata is metadata. A native link is admitted only
when its linker can be described: host binaries and tests on Linux and macOS,
where mbx puts the resolved linker, startup objects, libc, and SDK into the
key, and a fixed allowlist of built-in WebAssembly targets whose default
linker and system inputs ship with rustc. Everything else — native libraries,
custom linkers, Windows — links as it always did; see
[limits](/limits#native-linking-is-cached-only-where-the-linker-can-be-described).
`MBX_VERIFY=1` compiles while also consulting the cache and compares the result,
providing a deliberately expensive qualification mode.
