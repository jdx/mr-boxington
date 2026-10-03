---
description: Set up rust-analyzer, watch loops, laptop budgets, and debugging with mbx.
---
# Local development

mbx works underneath the tools already in your Rust development loop. Editors,
file watchers, terminals, and worktrees keep invoking ordinary Cargo, and their
compilations share the same cache and machine-wide scheduler. Start with
[Cargo and editor setup](/setup), then use the recipes on this page for your
editor, watch loop, machine budget, and debugger.

## Put editor checks through mbx

The native mise Rust option wraps Cargo but does not configure rust-analyzer.
Run `mbx setup` to send rust-analyzer's background check through mbx:

```sh
mbx setup
```

With the native option, follow the native-integration steps in
[rust-analyzer](/setup#rust-analyzer): if `mbx setup` asks where plain `cargo`
commands should use mbx, choose **Create the shim without activating it**, and
check the result with `env -u MISE_SHELL mbx setup --status`. Any other choice
adds mise's Cargo wrapper, which takes precedence over the native option, and
`--yes`, `--global`, or `--local` can add it too, so leave them off.

Restart the editor after `mbx setup` so that rust-analyzer reloads its
configuration. After the restart, rust-analyzer reports
`check/overrideCommand: unexpected field` for its user configuration file,
where `mbx setup` writes the override. The override still applies; see
[Cargo and editor setup: rust-analyzer](/setup#rust-analyzer).

`mbx setup` leaves an existing rust-analyzer check configuration untouched. If
it says it left the rust-analyzer file unchanged, or `mbx setup --status` says
that rust-analyzer keeps its existing check settings, choose one:

- Keep the existing check settings, and prepend the directory of the Cargo
  shim that `mbx setup` reports to the `PATH` the editor inherits.
- Change the `overrideCommand` executable to the Cargo shim's absolute path,
  keeping the command's existing arguments.

Point the editor at the Cargo shim, not a versioned mbx executable. On Unix, if
mise removes the mbx version that `mbx setup` recorded, run `mbx setup` again so
the editor's checks keep working; `mbx doctor` reports the shim as outdated
until you do.

See [Verify plain Cargo](/setup#verify-plain-cargo) for shell and
non-interactive `PATH` setup.

## Run a watch loop

A watcher needs no mbx-specific integration. Once plain `cargo` runs through
mbx, every Cargo command the watcher starts shares the same cache and compiler
pool. For example, with
[`cargo-watch`](https://github.com/watchexec/cargo-watch):

```sh
cargo install cargo-watch
cargo watch -x 'check --workspace'
```

`cargo watch` is itself a Cargo command, so mbx runs the whole watcher as one
build session. Each `cargo check` it starts joins that session instead of
starting its own, so mbx prints no build summary after each iteration. Follow
the iterations from a second terminal instead:

```sh
mbx tui
```

The TUI lists the watcher as one build session, adds each iteration's results
to it, and shows each crate's outcome.

The changed crate still has to compile, but mbx can restore unchanged
dependencies from any build or worktree on the machine. If another loop is
already running the identical compilation, mbx waits for it and restores that
result instead of running the compiler twice. In the TUI, a crate restored
after that wait appears as a hit, like any other restore.

An editor background check and a watcher may request much of the same work.
Running both is safe, but Cargo still plans both builds. Keep both only when
the watcher runs a different command, such as tests, Clippy, or another
feature set.

Do not add `cargo clean` to a watch loop. Cargo already rebuilds changed inputs,
and deleting the target directory throws away its local freshness information.
When a loop looks colder than expected, diagnose one iteration instead:

```sh
mbx explain check --workspace
```

## Keep a laptop responsive

All simultaneous mbx builds share one compiler pool. Its defaults use every
logical CPU and 85% of physical memory, which favors throughput. A smaller
machine-wide budget trades some cold-build speed for lower fan noise and more
headroom for the editor and browser.

Set a budget:

```sh
mbx settings set scheduler.cpus 4
mbx settings set scheduler.memory 8GiB
```

These numbers are an example; choose values for your machine. The CPU setting
limits concurrent real compiler work across every terminal and worktree; cache
hits do not consume permits. The memory setting prevents known large
compilations from filling all CPU permits at once. See
[Machine-wide compile scheduling](/scheduling#machine-wide-compile-scheduling)
for how compilations take permits.

To give one watch session a smaller share of the pool, cap it with Cargo's
job limit, `CARGO_BUILD_JOBS`. Child Cargo processes inherit the variable, and
mbx limits the session to that many permits of the shared pool, leaving the
rest to other builds. A single compilation heavier than the limit still runs,
but only while nothing else in that session is compiling:

```sh
CARGO_BUILD_JOBS=2 cargo watch -x 'check --workspace'
```

Setting `MBX_SCHEDULER_CPUS` or `MBX_SCHEDULER_MEMORY` for one session works
differently: it changes only how that session measures the shared pool. Its
compilations wait until the permits held by every build fit within its smaller
budget, so other builds can keep it waiting. Builds started with larger
settings are not limited by it and can still fill the machine.

`scheduler.priority = "low"` suits unattended or background builds, but it is
not a power limit. A low-priority build yields capacity to normal-priority
builds and may still use the whole configured pool when nothing else is
waiting.

## Debug a binary restored from another checkout

Restored artifacts behave the same, but are not guaranteed to contain the same
bytes as a local compilation. In particular, debug information can contain the
absolute source path of the checkout that populated the cache. A debugger may
therefore open an old worktree path or fail to bind a breakpoint even though
the program itself is correct.

To link the program locally while still restoring dependency compilations from
the cache, build into a separate target directory with native-link caching
disabled:

```sh
CARGO_TARGET_DIR=target/debugger MBX_CACHE_LINKS=0 \
  cargo build --bin my-program
```

Point the debugger at `target/debugger/debug/my-program`. The separate target
directory makes Cargo run a new build without disturbing the normal one, and
disabling link caching makes the program itself local. This is
usually enough to debug application code.

If source paths inside every dependency must also belong to the current
checkout, run Cargo without mbx for the isolated build:

```sh
CARGO_TARGET_DIR=target/debugger-uncached MBX_DISABLE=1 \
  cargo build --bin my-program
```

This build is cold, so reserve it for path-sensitive debugger or artifact
investigations. Remove the extra target directory when you finish; the store
remains intact. To check whether restored bytes diverge, run with
`MBX_VERIFY=1`. See [Verify restored outputs](/troubleshooting#verify-mode).
