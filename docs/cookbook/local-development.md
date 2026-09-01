# Local development

mbx can sit underneath the tools already in a Rust development loop. Editors,
file watchers, terminals, and worktrees may all keep invoking ordinary Cargo;
their compilations share the same cache and machine-wide scheduler.

## Put editor checks through mbx

Run setup once in the same scope in which mbx is installed:

```sh
mbx setup
mbx setup --status
```

Setup gives rust-analyzer's background check an absolute path to mbx's stable
Cargo shim. That matters for editors launched from a desktop icon: they often
do not inherit the shell activation that puts mise's Cargo wrapper on `PATH`.
Restart the editor after setup so rust-analyzer reloads its configuration.

Setup leaves an existing rust-analyzer check configuration untouched. If
`mbx setup --status` says that the editor kept its existing settings, either
keep that command and make the editor inherit the shim directory printed by
`mbx setup`, or change its `overrideCommand` executable to that absolute Cargo
shim while preserving the command's existing arguments. Do not point the
editor at a versioned mbx executable: the stable shim continues to work across
upgrades.

To check the path seen by a terminal, run:

```sh
command -v cargo
mbx doctor
```

On Windows, use `(Get-Command cargo).Path` instead of `command -v cargo`.
See [Install and run](/getting-started#verify-plain-cargo) for the shell and
non-interactive `PATH` setup.

## Run a watch loop

A watcher does not need mbx-specific integration. Once plain Cargo resolves to
the shim, every Cargo process it starts participates. For example, with
[`cargo-watch`](https://github.com/watchexec/cargo-watch):

```sh
cargo install cargo-watch
cargo watch -x 'check --workspace'
```

Use a second terminal for the live view:

```sh
mbx tui
```

The changed crate still has to compile, but unchanged dependencies can restore
from any build or worktree on the machine. If another loop is already compiling
the identical action, mbx waits for it and restores that result instead of
running a duplicate compiler. The TUI makes both decisions visible by crate.

An editor background check and a watcher may request much of the same work.
That is safe, but Cargo still plans both builds; keep both only when the watcher
runs a meaningfully different command such as tests, Clippy, or another feature
set.

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

Add a budget to the platform config file listed on the
[Configuration](/configuration) page:

```toml
[scheduler]
cpus = 4
memory = "8GiB"
```

Choose values for the machine rather than copying those numbers blindly. The
CPU setting limits concurrent real compiler work across every terminal and
worktree; cache hits do not consume permits. The memory setting prevents known
large compilations from filling all CPU permits at once.

For a temporary low-power watch session, set the same values in its
environment. Child Cargo processes inherit them:

```sh
MBX_SCHEDULER_CPUS=2 MBX_SCHEDULER_MEMORY=4GiB \
  cargo watch -x 'check --workspace'
```

`scheduler.priority = "low"` is useful for unattended or background builds,
but it is not a power limit: it yields capacity to normal-priority builds and
may still use the whole configured pool when nothing else is waiting.

## Debug a binary restored from another checkout

Restored artifacts behave the same, but are not guaranteed to contain the same
bytes as a local compile. In particular, debug information can contain the
absolute source path of the checkout that populated the cache. A debugger may
therefore open an old worktree path or fail to bind a breakpoint even though
the program itself is correct.

For a local final link while retaining cached dependency compilations, build
into a separate target directory with native-link caching disabled:

```sh
CARGO_TARGET_DIR=target/debugger MBX_CACHE_LINKS=0 \
  cargo build --bin my-program
```

Point the debugger at `target/debugger/debug/my-program`. The separate target
directory forces Cargo to materialize a new build without disturbing the
normal one, and disabling link caching makes the program itself local. This is
usually enough to debug application code.

If source paths inside every dependency must also belong to the current
checkout, bypass mbx for the isolated build:

```sh
CARGO_TARGET_DIR=target/debugger-uncached MBX_DISABLE=1 \
  cargo build --bin my-program
```

This is deliberately cold and is best reserved for path-sensitive debugger or
artifact investigations. Remove the extra target directory when finished; the
shared action cache remains intact. To investigate whether restored bytes
diverge rather than merely obtain a local debug build, use
[`MBX_VERIFY=1`](/configuration#verify-mode).
