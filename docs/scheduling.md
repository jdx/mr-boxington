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

Use `scheduler.priority = "low"` (`MBX_SCHEDULER_PRIORITY=low`) for an editor's
background check or CI on a shared machine. While normal-priority work is
waiting, low-priority builds leave a quarter of the pool available for it.

## Choose the scope of a limit

| Limit | Affects |
| --- | --- |
| `scheduler.cpus`, `scheduler.memory` | The shared compiler pool |
| `scheduler.reserve_cpus` | Capacity left outside that pool |
| Cargo `-j` or `CARGO_BUILD_JOBS` | How much of the pool one build may hold |
| `scheduler.priority = "low"` | Whether a build yields to waiting normal-priority work |

A memory budget schedules work using measurements; it is not an operating-system
memory limit. A compiler process can still exceed its estimate. For laptop
settings, see [Keep a laptop responsive](/cookbook/local-development#keep-a-laptop-responsive).
