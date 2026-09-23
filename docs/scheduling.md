---
description: Run concurrent Cargo builds with separate target directories and one shared compiler budget.
---
# Parallel builds

Give independent Cargo commands their own target directories. mbx coordinates
their real compiler processes through a shared CPU and memory budget; Cargo
continues to plan dependencies within each build.

## Run independent tasks

Any task runner can start multiple mbx commands at the same time. For example,
mise runs these two lint configurations together:

```toml
# mise.toml
[tasks."lint:default"]
run = "mbx clippy --workspace -- -D warnings"
env.CARGO_TARGET_DIR = "target/clippy-default"

[tasks."lint:all"]
run = "mbx clippy --workspace --all-features --all-targets -- -D warnings"
env.CARGO_TARGET_DIR = "target/clippy-all"
```

```sh
mise run lint:default ::: lint:all
```

Separate target directories keep Cargo's directory lock from serializing the
commands. mbx shares one machine-wide compiler pool and deduplicates identical
work in flight without further configuration. See
[how it works](/how-it-works#machine-wide-scheduling) for the mechanism and
the same shape [inside GitHub Actions](/github-action#parallel-cargo-steps).

## Machine-wide compile scheduling

Each Cargo process sets its own concurrency. Without coordination, several
builds can collectively start more compiler processes than the machine can
comfortably run. mbx coordinates them through a shared pool under the cache
directory. Scheduling is on by default; `MBX_SCHEDULER=0` disables it.

A permit represents a share of the pool's CPU and memory budget. The pool has
`scheduler.cpus` permits (default: logical CPUs), minus
`scheduler.reserve_cpus` (default: 0), with at least one permit remaining.
`scheduler.memory` defaults to 85% of physical memory. In a Linux container,
the cgroup's memory limit constrains that budget.

Compiler processes take permits according to their estimated memory use.
Unmeasured compilations start at one permit; native links start at two and can
use estimates from earlier links. Measurements refine later admissions.
Set `scheduler.memory = "none"` to use CPU permits without memory weighting.

Cache hits do not need compiler permits. If a process dies, the kernel releases
its permits. For the weighting and recovery details, see
[how it works](/how-it-works#machine-wide-scheduling).

Live pressure control is enabled by default (`scheduler.pressure = true`). It
pauses additional admissions, including compilations with no memory history,
while memory headroom is low or Linux memory-stall measurements show sustained
pressure. macOS uses available-memory headroom; it has no Linux PSI signal.
At least one compilation can run when the pool is idle. After five healthy
seconds, admissions resume gradually for five seconds. Running processes are
not suspended by this setting.

Set `scheduler.pressure = false` (`MBX_SCHEDULER_PRESSURE=0`) to retain the
existing estimate-based admission policy. Disabling the scheduler or setting
`scheduler.memory = "none"` also disables pressure control. If pressure probes
fail, scheduling falls back to the existing permit and estimate checks.

Use `scheduler.priority = "low"` (`MBX_SCHEDULER_PRIORITY=low`) for an editor's
background check or CI on a shared machine. While normal-priority work is
waiting, low-priority builds leave a quarter of the pool available for it.

## Schedule test binaries

Compile permits stop at the compiler. When several `cargo test` commands reach
their test runs together, each libtest harness starts a thread per CPU. Set
`scheduler.tests = true` (`MBX_SCHEDULER_TESTS=1`) to run test binaries through
the same pool:

```sh
MBX_SCHEDULER_TESTS=1 mbx test --workspace
```

mbx becomes Cargo's target runner for `cargo test` and calls any runner you
have configured. Each test binary waits for permits before it starts:

- Once a binary has been measured, it asks for the average number of cores it
  kept busy: CPU time divided by wall time, rounded to the nearest core.
- Before that, `--test-threads=N` or `RUST_TEST_THREADS=N` asks for `N`
  permits, and otherwise the binary asks for half the pool, rounded up.
- A binary whose measured memory needs more permits takes that many instead.

Only complete runs of at least a second are measured; a run narrowed by a test
name, `--skip`, or `--ignored` is not. A recorded core count only goes up,
because a suite measured on a busy machine gets fewer cores than it would use.
History is kept per Git repository, package, and test binary. Worktrees of one
repository share it, and separate clones and unrelated projects do not. Outside
Git, projects that share a package and test name share history. A run with a stated
thread count keeps separate history from the default width.
CPU is measured on Unix only; on Windows the other rules apply.

Builds a test starts, such as trybuild or compile-fail suites, are charged to
the test's permits and run without taking permits of their own.

Doctests, `cargo test --no-run`, commands with `--config`, a `+toolchain`
override, or a directory change (`-C`, `--directory`), and test runners other
than `cargo test` run unscheduled.

## Choose the scope of a limit

| Limit | Affects |
| --- | --- |
| `scheduler.cpus`, `scheduler.memory` | The shared compiler pool |
| `scheduler.reserve_cpus` | Capacity left outside that pool |
| Cargo `-j` or `CARGO_BUILD_JOBS` | How much of the pool one build may hold |
| `scheduler.priority = "low"` | Whether a build yields to waiting normal-priority work |
| `scheduler.tests = true` | Whether `cargo test` binaries take permits |

A memory budget schedules work using measurements; it is not an operating-system
memory limit. A compiler process can still exceed its estimate. For laptop
settings, see [Keep a laptop responsive](/cookbook/local-development#keep-a-laptop-responsive).

## Experimental Linux compiler supervision

`scheduler.suspend = true` (`MBX_SCHEDULER_SUSPEND=1`) opts compiler processes
into cgroup supervision. Set `scheduler.cgroup_root` (`MBX_SCHEDULER_CGROUP_ROOT`)
to an absolute path to a writable, delegated cgroup v2 directory. mbx creates
its own descendants and does not change limits on the supplied directory.
Pressure control and memory scheduling must also be enabled. These two settings
are user/environment configuration only; repository `.mbx.toml` policy cannot
opt a contributor into supervision or choose their delegated directory.

```toml
[scheduler]
suspend = true
cgroup_root = "/sys/fs/cgroup/my-delegated-builds"
```

After pressure persists for two seconds, mbx can freeze the newest eligible
compiler tree, at most one per second. The oldest eligible action keeps running;
when it finishes its successor is resumed to preserve progress. After pressure
recovers, suspended work resumes oldest first before new admissions. Frozen
compilers retain their permits. Freezing stops execution but retains allocated
memory; it cannot rescue a compilation that is too large to run alone.

A supervisor and an independent watchdog stay outside the compiler cgroups. Losing the supervisor's heartbeat thaws its compiler groups.
Losing the watchdog's heartbeat disables supervision for new compilers and
starts a replacement watchdog for the compilers still running. The next
supervisor removes cgroups and state left by earlier ones once their compilers
have exited.
Compiler cancellation thaws the owned group before terminating leftover
processes. Custom Rust wrappers, build-script binaries, test binaries, and
compilers nested inside a supervised compiler are not eligible.

If delegation is unavailable or the platform is unsupported, mbx warns once per
session and continues with admission scheduling. Enabling this option does not
provision systemd units, grant permissions, or modify host cgroup limits.

Suspension/resumption events and per-action memory and suspended-time statistics
are recorded beneath `scheduler/supervision-*/` in the cache directory. Compiler
completion reports time spent suspended. Probe failures or a lost watchdog
heartbeat thaw the generation and stop its supervisor. Suspension never kills
and retries a compilation; termination of orphaned descendants is cancellation
cleanup only.
