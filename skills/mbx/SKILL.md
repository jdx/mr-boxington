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

Start with `mbx --version` and `mbx doctor`. Explicit mbx commands work without
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
tasks, activated shells, and mise shims. Retain the existing Rust version and
other tool options when adding it. A project configuration might contain:

```toml
[tools]
rust = { version = "1.90.0", mr_boxington = true }
mr-boxington = "latest"
```

The version above is illustrative, not an instruction to change the project's
Rust pin. A project may instead already use mise's Cargo wrapper, a
`[wrappers.cargo]` entry with `command = "mbx"` and `MBX_CARGO_SHIM_MODE = "1"`.
Preserve a working integration rather than layering a second wrapper onto it.

Standalone `mbx setup` installs a user-level Cargo shim and rust-analyzer
override. It can also add `[wrappers.cargo]` to a mise config and run
`mise reshim`, and that config can be the project's committed `mise.toml`.
`mbx setup` chooses the config as follows:

- `--global` and `--local` choose the global or current project's mise config.
- `--yes` accepts the recommendation: the config named by `MISE_CONFIG_FILE`
  when it is set; otherwise, in a mise-activated shell, the config that defines
  `mr-boxington`, then the nearest project mise config, then the global config.
- Without a flag, `mbx setup` prompts in an interactive mise-activated shell and
  preselects the recommended config.

When `mbx setup` selects no mise config, it changes only the shim and editor
configuration. Whichever config it selects, it also removes the check override
that older mbx releases wrote to the project's `rust-analyzer.toml`.

Use `mbx setup` when the task asks for the Cargo shim, mise's Cargo wrapper, or
editor setup; explicit mbx commands do not need it. Native mise wrapping alone
does not configure
[rust-analyzer](https://mr-boxington.jdx.dev/setup#rust-analyzer). If the
project already uses `mr_boxington = true`, do not pass `--yes`, `--local`, or
`--global`, and choose **Create the shim without activating it** if
`mbx setup` prompts. Selecting a mise config adds `[wrappers.cargo]`, which takes
precedence over the `mr_boxington` option, including `mr_boxington = false`.

With native wrapping in a mise-activated shell, `mbx setup --status` fails with
`mbx setup is installed but is not active in the selected mise config` before
it checks rust-analyzer. Run `env -u MISE_SHELL mbx setup --status` instead,
and do not select a mise config to clear that message.

Even when mise wrapping works, the `setup` check in `mbx doctor` can warn that
the standalone Cargo shim is not installed, or that the shim is current but not
active and its directory should be prepended to `PATH`. `mbx doctor` does not
recognize native mise wrapping. Verify the actual build route before changing
`PATH` to clear either warning.

## Verify real reuse

Use the same source, toolchain, profile, features, and cache directory
in two fresh target directories. Choose paths that do not already contain
build outputs:

```sh
mbx build --target-dir target/cache-demo-first
mbx build --target-dir target/cache-demo-second
mbx explain --last
```

Adapt both builds to the project's representative command. If the goal is
transparent Cargo wrapping, run both through `mise exec -- cargo build ...`
or the configured route instead of invoking mbx directly. Run them in your own
environment: installing mbx does not prove that your Cargo processes use it.

Read the second build's cache counters:

- A hit means mbx restored a matching compilation.
- A miss means mbx looked up the key and found no result.
- `not looked up` means no usable input
  [prediction](https://mr-boxington.jdx.dev/how-it-works#prediction-and-dep-info)
  was available.
- `bypassed` means the action ran without shared caching.

Cargo skipping up-to-date outputs is not an mbx cache hit. Report observed hits
and remaining blockers; do not promise every action is cacheable. When
finished, remove only the temporary target directories created for this check.

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

For a suspected wrapper failure, run the same plain `cargo` command with and
without `MBX_DISABLE=1` (for example, `MBX_DISABLE=1 cargo build`) as a one-off
diagnostic. mbx reads `MBX_DISABLE` only when it runs as plain `cargo`, so
`MBX_DISABLE=1 mbx build` still builds through mbx. See
[Run Cargo without mbx](https://mr-boxington.jdx.dev/troubleshooting#bypass-mbx-for-one-command)
for the PowerShell form. Keep `MBX_DISABLE` out of committed configuration and
report both results. Review logs for credentials and private remote URLs
before sharing them.

Use the [setup guide](https://mr-boxington.jdx.dev/setup),
[cache results](https://mr-boxington.jdx.dev/cache-results), and
[configuration reference](https://mr-boxington.jdx.dev/configuration) for editor
integration, cache policy, scheduling, and remote-cache details. Add remote
credentials or machine-wide scheduler changes only when the task calls for them.
