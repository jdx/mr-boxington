# How it works

mbx is a Cargo wrapper, not a Cargo replacement. Run Cargo subcommands directly
through it: `mbx build`, `mbx test`, `mbx clippy`, or any installed Cargo
subcommand.

1. mbx resolves the workspace and target roots through Cargo metadata.
2. It starts an in-process cache agent and creates a rustc shim for the build.
3. Cargo runs normally with the shim set as `RUSTC_WRAPPER`.
4. The shim analyzes each rustc invocation and derives a content-addressed action key.
5. A hit restores the action's outputs; a miss runs the real compiler and publishes the result.
6. The agent exits with the build, draining any remote uploads it still owes.
   There is no persistent daemon.

Every mbx command works this way; there is no separate mode to turn on and no
component to keep up to date.

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

## Correctness first

Unsupported crate types, unmodeled search paths, native linking, and
incremental compilations bypass the shared action cache. Linked WebAssembly
binaries, tests, and `cdylib`s are admitted only for a fixed allowlist of
built-in targets whose default linker and system inputs ship with rustc.
`MBX_VERIFY=1` compiles while also consulting the cache and compares the result,
providing a deliberately expensive qualification mode.
