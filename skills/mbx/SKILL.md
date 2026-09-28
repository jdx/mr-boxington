---
name: mbx
description: Set up and use mr-boxington (mbx) for cached Cargo builds, verify real cache reuse, and diagnose misses, bypasses, and Cargo wrapper activation.
---

# mr-boxington (mbx)

mbx wraps Cargo and reuses compiler work across projects and worktrees. Cargo
still chooses what to build. Inspect the project's toolchain pins, build tasks,
mise configuration, Cargo configuration, and existing compiler wrappers before
changing its integration. Preserve the Rust version and build flags.

## Choose the integration

Start with `mbx --version` and `mbx doctor`. Explicit commands work without
shell or editor setup and accept ordinary Cargo arguments:

```sh
mbx build --workspace
mbx test --workspace --all-features
mbx clippy --workspace --all-targets -- -D warnings
mbx +stable check --workspace
```

Use the project's actual toolchain rather than substituting `stable` for an
existing pin. If mise manages Rust, its `mr_boxington = true` tool option
(mise 2026.9.2 or newer) enables wrapping for `mise exec -- cargo ...`, mise
tasks, and activated shells. Retain the existing Rust version and other tool
options when adding it. A project configuration might contain:

```toml
[tools]
rust = { version = "1.90.0", mr_boxington = true }
mr-boxington = "latest"
```

The version above is illustrative, not an instruction to change the project's
Rust pin. A project may instead already use `[wrappers.cargo]` with `command =
"mbx"` and `MBX_CARGO_SHIM_MODE = "1"`; preserve a working integration rather
than layering a second wrapper onto it.

Standalone `mbx setup` changes user-level Cargo shim and editor configuration.
Use it when that broader setup is requested; explicit mbx commands do not need
it. Native mise wrapping alone does not configure desktop editors. `mbx doctor`
can report a missing standalone Cargo shim even when mise wrapping works, so
verify the actual build route before changing PATH to clear that warning.

## Verify real reuse

Use the same source, toolchain, profile, features, and shared cache in two fresh
target directories. Choose paths that do not already contain build outputs:

```sh
mbx build --target-dir target/cache-demo-first
mbx build --target-dir target/cache-demo-second
mbx explain --last
```

Adapt both builds to the project's representative command. If the goal is
transparent Cargo wrapping, run both through `mise exec -- cargo build ...`
or the configured route instead of invoking mbx directly. Use the agent's own
environment: installation alone does not prove its Cargo processes use mbx.

Read the second build's cache counters. A hit means a matching compilation was
restored. A miss means a key was looked up but absent. `not looked up` means no
usable input prediction was available; `bypassed` means the action ran without
shared caching. Cargo skipping up-to-date outputs is not an mbx cache hit.
Report observed hits and remaining blockers; do not promise every action is
cacheable. Remove only temporary targets created for this check when finished.

## Diagnose before changing cache policy

```sh
mbx doctor --json
mbx explain --last
mbx explain build --workspace
mbx cache stats
mbx gc --dry-run
```

`explain --last` inspects a recorded build without running Cargo; `explain
build` runs a build with diagnostic collection. Follow the reported reason for
a miss or bypass. Different features, profiles, toolchains, and `RUSTFLAGS`
can legitimately prevent reuse. Do not start by deleting caches or normal
target directories; `mbx clean` and cache removal commands discard state.

For a suspected wrapper failure, compare the same command with `MBX_DISABLE=1`
as a one-off diagnostic. Keep that bypass out of committed configuration and
report both results. Review logs for credentials and private remote URLs before
sharing them.

Use the [setup guide](https://mr-boxington.jdx.dev/setup),
[cache results](https://mr-boxington.jdx.dev/cache-results), and
[configuration reference](https://mr-boxington.jdx.dev/configuration) for editor
integration, cache policy, scheduling, and remote-cache details. Add remote
credentials or machine-wide scheduler changes only when the task calls for them.
