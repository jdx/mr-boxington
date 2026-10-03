---
description: Run concurrent Cargo builds with separate target directories and one shared compiler budget.
---
# Parallel builds

Give independent Cargo commands their own target directories. mbx coordinates
their real compiler processes through a shared CPU and memory budget; Cargo
continues to plan dependencies within each build.

`check` and `clippy` need no setup. In a managed target directory, they
already write to their own [check lane](/managed-targets#check-lanes), so
`mbx clippy` runs beside `mbx build`. The rest of this page is for commands
that would otherwise share a target directory, such as two builds or several
lint configurations.

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
[Machine-wide scheduling](/how-it-works#machine-wide-scheduling) for the
mechanism and [Parallel Cargo steps](/github-action#parallel-cargo-steps) for
the same setup in GitHub Actions.

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
use estimates from earlier links. Measurements refine later admissions. mbx
measures compiler memory on Unix only, so on Windows, compilations keep these
starting weights. Set `scheduler.memory = "none"` to use CPU permits without
memory weighting.

Cache hits do not need compiler permits. If a process dies, the kernel
releases its permits. For the weighting and recovery details, see
[Machine-wide scheduling](/how-it-works#machine-wide-scheduling).

Live pressure control is on by default (`scheduler.pressure = true`). It
pauses further admissions, including compilations with no memory history, while
available memory is low or memory stalls show sustained pressure. The signals
mbx reads depend on the platform:

- Linux: available-memory headroom and pressure stall information (PSI), the
  kernel's measure of time lost waiting for memory
- macOS: available-memory headroom only
- Windows: neither, so pressure control does not pause admissions there

At least one compilation can run when the pool is idle. After five healthy
seconds, mbx admits at most one compilation every half second for five seconds,
then lifts that limit. Pressure control does not suspend running processes. To
freeze them, see
[Experimental Linux compiler supervision](#experimental-linux-compiler-supervision).

Containers that share a cache directory share permits, but mbx measures each
container's pressure from its own memory readings and pauses only that
container's admissions. Suspended work is the exception: while compiler
supervision in any container has frozen a compilation, every container sharing
the cache waits for it to resume.

Set `scheduler.pressure = false` (`MBX_SCHEDULER_PRESSURE=0`) to admit
compilations on permits and memory estimates alone. Disabling the scheduler or
setting `scheduler.memory = "none"` also disables pressure control. If the
pressure probes fail, scheduling falls back to the same permit and estimate
checks.

Use `scheduler.priority = "low"` (`MBX_SCHEDULER_PRIORITY=low`) for an editor's
background check or CI on a shared machine. While normal-priority work is
waiting, low-priority builds leave a quarter of the pool available for it.

## Reserve capacity for external work

Use `mbx reserve` for a command whose work consumes the same host CPU or
memory but does not run through mbx, such as a build in a VM or container:

```sh
mbx reserve --memory 6GiB --cpus 4 -- ./run-windows-build-in-vm.sh
```

The command waits until the requested capacity is free, then a dedicated worker
holds it until the external command exits. Killing the invoking `mbx reserve`
process alone does not release capacity while that worker still waits for the
command; a crashed worker does release its lease through the operating system.
The command must wait for work it launches: detached background processes can
outlive the reservation and are not covered after the command exits.
`--memory` is measured against `scheduler.memory`; omit it when memory
scheduling is disabled. Use `--priority low` for background work that
should yield at admission time to waiting normal-priority builds; an admitted
reservation, like an admitted compiler, is never preempted.

## Schedule test binaries

By default, permits cover compilers only. When several `cargo test` commands
reach their test runs together, each libtest harness starts a thread per CPU.
Set `scheduler.tests = true` (`MBX_SCHEDULER_TESTS=1`) to run test binaries
through the same pool:

```sh
MBX_SCHEDULER_TESTS=1 mbx test --workspace   # this run
mbx settings set scheduler.tests true        # every run from now on
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
mbx measures CPU and memory on Unix only. On Windows, a binary always asks for
its stated thread count or half the pool.

History is kept per Git repository, package, and test binary. Worktrees of one
repository share it; separate clones and unrelated projects do not. Outside
Git, projects that share a package and test name share history. A run with a
stated thread count keeps its history apart from runs at the default thread
count.

Builds a test starts, such as trybuild or compile-fail suites, are charged to
the test's permits and run without taking permits of their own.

These run unscheduled:

- doctests
- `cargo test --no-run`
- commands with `--config`, a `+toolchain` override, or a directory change
  (`-C`, `--directory`)
- test runners other than `cargo test`

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
settings, see
[Keep a laptop responsive](/cookbook/local-development#keep-a-laptop-responsive).

## Experimental Linux compiler supervision

Pressure control only holds back new compilations. With supervision, mbx can
also freeze running compilers while memory is under pressure and resume them
when pressure recovers.

::: warning Experimental
Compiler supervision is experimental and off by default. It needs Linux and a
delegated cgroup v2 directory. If you enable it where delegation is unavailable
or on another platform, mbx warns once per build and continues with admission
scheduling.
:::

`scheduler.suspend = true` (`MBX_SCHEDULER_SUSPEND=1`) opts compiler processes
into cgroup supervision. Supervision also needs:

- `scheduler.cgroup_root` (`MBX_SCHEDULER_CGROUP_ROOT`) set to the absolute
  path of a writable, delegated cgroup v2 directory
- pressure control and memory scheduling, which are both on by default

Set `scheduler.suspend` and `scheduler.cgroup_root` in your global
configuration file or the environment. A repository's `.mbx.toml` cannot opt a
contributor into supervision or choose their delegated directory. mbx creates
its own cgroups inside the delegated directory and does not change limits on
it. Enabling supervision does not provision systemd units, grant permissions,
or modify host cgroup limits.

```toml
[scheduler]
suspend = true
cgroup_root = "/sys/fs/cgroup/my-delegated-builds"
```

After pressure persists for two seconds, mbx can freeze the newest eligible
compiler tree (the compiler and the processes it starts), at most one per
second. The oldest eligible compilation keeps running; when it finishes, mbx
resumes its successor to preserve progress. After pressure recovers, mbx
resumes suspended work oldest first, before it admits anything new. Frozen
compilers keep their permits. Freezing stops execution but keeps allocated
memory, so it cannot rescue a compilation that is too large to run alone.

Custom compiler wrappers, build-script binaries, test binaries, and compilers
nested inside a supervised compiler are not eligible.

mbx runs a supervisor and an independent watchdog outside the compiler cgroups.
Each watches the other's heartbeat, and the supervisor also checks its
pressure probes:

- If the supervisor's heartbeat stops, the watchdog thaws the supervisor's
  compilers and disables further suspension.
- If the watchdog's heartbeat stops, the supervisor does the same and starts a
  replacement watchdog for the compilers still running.
- If a pressure probe fails, the supervisor thaws its compilers and disables
  further suspension.

After suspension is disabled, mbx keeps cleaning up the cgroups it owns until
their compilations finish. Once an earlier supervisor's compilers have exited,
the next supervisor removes the cgroups and state it left behind.

When a compilation is cancelled, mbx thaws its cgroup before terminating any
leftover processes. Suspension itself never kills and retries a compilation;
mbx terminates orphaned descendants only to clean up after a cancellation.

mbx records suspend and resume events, plus each compilation's peak memory and
suspended time, under `scheduler/supervision-*/` in the cache directory. When a
suspended compilation finishes, mbx reports how long it spent suspended.
