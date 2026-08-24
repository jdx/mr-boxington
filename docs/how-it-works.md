# How it works

`mbx build` is a cargo wrapper, not a cargo replacement.

1. mbx resolves the workspace and target roots through Cargo metadata.
2. It starts an in-process cache agent and creates a rustc shim for the build.
3. Cargo runs normally with the shim set as `RUSTC_WRAPPER`.
4. The shim analyzes each rustc invocation and derives a content-addressed action key.
5. A hit restores the action's outputs; a miss runs the real compiler and publishes the result.
6. The agent exits with the build. There is no persistent daemon.

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

## Correctness first

Unsupported crate types, unmodeled search paths, linking, and incremental
compilations bypass the shared action cache. `MBX_VERIFY=1` compiles while also
consulting the cache and compares the result, providing a deliberately expensive
qualification mode.
