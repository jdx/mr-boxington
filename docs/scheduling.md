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

The pool has two independent budgets: CPU cores and memory bytes. The CPU
budget is `scheduler.cpus` (default: the machine's online logical CPUs, or a
lower CPU quota on a parent cgroup) minus `scheduler.reserve_cpus` (default:
0), with at least one core remaining.
The memory budget is `scheduler.memory`, which defaults to 85% of physical
memory. If a parent cgroup has a lower memory limit, the default is 85% of that
limit instead, because sibling cgroups under that parent share it. Each
container also has its own CPU and memory ceilings, described below.
Reserving CPUs does not reduce the memory budget.

A request must fit both budgets. CPU charges follow the compiler's thread
count, or a test's learned core use. Memory charges use predicted bytes from
past measurements. A compile predicted to use 24 GiB on one core therefore
leaves other cores available for work that fits the remaining memory.
Each budget allows an oversize request when no running request consumes that
resource: an oversize memory request still needs CPU room, and an oversize
CPU request still needs memory room.

For unmeasured work, the memory guess is one share: the memory ceiling of the
build's own domain divided by its CPU ceiling, rounded down. In a container,
these are the container's own ceilings, and the host budgets only limit what
all containers run together. `scheduler.reserve_cpus` comes off both the
shared pool and each build's own CPUs. Ordinary single-threaded
compiles start at one such share, native links at two, and unmeasured nextest
tests at two. Higher thread counts retain at least their old memory guess.
Measurements refine later admissions and use raw predicted bytes, without
rounding to CPU shares. mbx measures compiler memory on Unix only, so on
Windows, compilations keep the starting guesses. Set
`scheduler.memory = "none"` for CPU-only scheduling.
Cargo's `-j` or `CARGO_BUILD_JOBS` adds a CPU ceiling for one build; it does
not impose another memory ceiling.

For compiles, mbx keeps separate histories for each set of emitted outputs,
crate types, explicit targets, test compilation, optimization, LTO, codegen
units, debug info, incremental compilation and compiler thread settings.
Repeated emit and crate-type flags are combined, sorted and deduplicated;
output paths and dep-info do not distinguish modes. With no emit flag, rustc
emits a link output. Clippy has separate modes. Links retain their existing
crate and code-generation-profile keys. Native C/C++ compiles keep their
never-lowered legacy maximum: their filename-only keys can collide across libraries, so small compiles must not
push another library’s heavy peak out of history. Other keys without a Rust
compile mode or link identity also retain the legacy rule.

Each Rust compile-mode or link key keeps the last **16 successful, unwrapped
peak-RSS measurements**.
The charge is the larger of the window maximum and
`window_max * exp(q)`. The calibration score is
`ln(actual_peak / admission_time_window_max)`, so a given percentage increase
has the same score for every key, regardless of its measured spread.

The target probability is **0.95**, selected by causal admission replay.
For each kind, the latest 256 scores calibrate `q` using finite-sample rank
`ceil(0.95 * (N + 1))`, clamped to the `N` available scores. Only successful,
unwrapped runs calibrate the estimate. Scores saved by older versions used a
different scale and are discarded; peak windows and sibling calibration remain.

Before 50 scores, mbx keeps its spread-based upper bound:
`exp(mean(log peaks) + t * s * sqrt(1 + 1/n))`, with the same window maximum
floor. Here `t` is a one-sided Student-t multiplier with `4 + n - 1` degrees
of freedom. The spread `s` shrinks the key's sample variance toward a pooled
spread with four prior degrees of freedom. The pool uses the median log-space
sample standard deviation over keys of the same kind with at least four
samples; fewer than ten qualifying keys use 0.5. This allows the bound to
exceed the measured peak even for a key with just one measurement. The key's reported `q` expresses this
bound as a log ratio to its window maximum.

Waiting requests that use the compiler ledger check it on every poll and
re-predict their charge when the cached ledger reloads. The parsed ledger and prediction are reused until
the file changes, for at most one second.
On Unix, file identity also detects replacements with matching timestamps and sizes. When a request is admitted,
its prediction features are retained for calibration after the run, even if
other jobs update the ledger in the meantime. Nextest requests read their
separate per-test history directly and do not load the compiler ledger.

A compile mode without window samples first uses its retained legacy per-mode
entry, preserving the old headroom and crate-wide fallback rules. If that entry
is absent, mbx treats the new mode as another draw from its siblings' window
maxima: the estimate uses their mean and pooled between-mode spread, stays at
least the largest current sibling charge, and is capped at the previous
no-history charge (the legacy crate-wide maximum, or the default one-share
guess when the crate has none). The cap takes precedence over the floor. With
no sampled siblings it uses the legacy crate-wide maximum, then the default
one-share memory guess. A link without window samples first uses
its legacy per-key maximum, then its profile's recent link maximum if available (the global link window for links
without a profile), then the two-share link floor. Test-binary and nextest
histories keep their existing behavior.

Estimates can fall as old peaks roll out of the window. An OOM kill appends
its doubled measured peak, even for wrapped work, so the charge rises to at
least that value. Ordinary failures and wrapped successes do not teach the
new windows. Each window map is capped at 1024 keys, evicting the smallest
window maximum first. Legacy all-time peaks, compile sample counts and link
windows are still written with their original rules for older mbx versions.
Old ledgers read without migration; older writers can discard the new fields,
which makes new readers return to the cold-start hierarchy until they relearn.

`mbx stats --scheduler` shows each key's window size, window maximum, bound,
charge and source, alongside its legacy peak and sample count. It also shows
pooled spreads, score counts and log-ratio quantiles for each kind.
Before calibration the kind summary shows the cold-key log ratio (n=1);
JSON includes each measured key's actual log ratio. Sibling summaries keep
their standardized multipliers. Test-run charges still
defer to admission. Add `--json` for raw bytes. Without `--scheduler`, the
stats output omits this section. Charges use global scheduler settings;
a workspace `.mbx.toml` can change them for that workspace's builds.
The read-only activity snapshot shows each lease and waiter with its name, mode
key, build ID, pressure domain, age, and the charge it was really admitted or
queued with. Older records show unknown names; abandoned records can remain
visible until an admission reclaims them.

With memory weighting enabled, completed admitted compiles, links, test binaries,
and nextest tests append estimate-accuracy observations to
`<cache_dir>/scheduler/observations.jsonl`. The existing `events` setting gates
this log (on by default); `events = false` disables observations too. Each line
pairs the prediction and its source with charged CPU cores and memory bytes,
plus legacy admitted permits and bytes per permit,
measured child peak RSS (or null), outcome, wrapper status, and wait/run times.
Observations include build identity and applicable build and domain capacities.
`mbx reserve` records each reservation's CPU and memory charges, legacy weight,
wait and run times, and command
outcome; reservations have no peak measurement.
Peak RSS has the same child-process measurement limitations as the memory ledger.
Lines are smaller than 1 KiB; exceptionally long names, keys, or pressure domains
are shortened with a hash suffix to preserve distinct identities.

Before appending to a file over 8 MiB, a writer tries `observations.lock` without
waiting, then rotates to `observations.jsonl.1`, replacing the previous generation.
The usual retention bound is two approximately 8 MiB files. A busy rotation lock
can temporarily allow growth; concurrent appends and rotation can lose a few lines.
Observation failures are debug-logged and do not prevent the workload from running.

Admission transitions are recorded in `<cache_dir>/scheduler/decisions.jsonl`,
also gated by `events`, with the same best-effort 8 MiB rotation to `.1`.
Enqueue, dequeue (including gate or error reason), admission, and reclamation of
dead waiters record identity, resources, and wait age. A request gated before it
joins the queue records a `gated` event when its gate reason changes, rather than
on every retry. A dequeue records that reason too, so continued gating does not
repeat it. Request arrival time survives requeues. Admissions include the
ordered/passing rule, free resources in each scope, and the oldest overtaken
waiters with exception reasons or the actual CPU/memory slack bounds (legacy
scalar checks include their permit size). Events are collected while admission
is locked, then encoded and appended after the registrar is released. Concurrent
writers can therefore append events in a different order from their decisions;
use their timestamps and tickets to follow a request.

A waiter that releases its place empties its waiter file before unlocking it, so
a scan skips it instead of reporting it as abandoned. An abandoned event therefore
means the waiting process ended without releasing its place: it was killed,
exited without unwinding, or panicked (release builds abort on panic). Emptying
the file is best effort and has not been tested on Windows; if it fails, a normal
release can still be reported as abandoned. Abandoned events are incomplete: an
older mbx, or a process with `events = false`, can reclaim a dead waiter without
logging it. Logging errors and rotation can also lose events. In an abandoned
event, `waited_ms` is the age of the last queue stint, which a gate requeue
resets; `first_arrival_ms` gives the request's total age, and `mbx stats` uses it
to list abandoned waits over 15 minutes. With `events =
false`, admission does no decision-log work; the only remaining cost is emptying
the waiter file on release.

Transitions are under 1 KiB; admission records fit within 4 KiB including the
newline and retain at most 16 overtaken waiters, oldest first. The total and
logged counts expose truncation. A normal append uses one write. After a short
write, the writer appends a newline to end the partial line; this is a separate
write, so a line from another process can still land in between and be lost.
`mbx stats --scheduler` summarizes admissions, overtakes, dequeues, gate reasons,
and abandoned waits over 15 minutes; its JSON retains all recorded ranked
overtakes. Missing entries make overtake reason counts and rankings incomplete.
Logging never changes admission policy.

The estimate-accuracy section of `mbx stats --scheduler` reads both generations,
ignoring malformed lines, and shows the retained time span as UTC dates and
times. Timestamp fields in `--json` are Unix milliseconds.
It shows total observations, reservation exceedances, the worst peak/reservation
ratio and its timestamp, and the 20 keys with the highest worst-case ratios.
`--json` includes all keys, counts, highest peaks, median and nearest-rank p90
peak/prediction ratios, and average reserved-but-unused bytes (clamped to zero
when a run exceeds its reservation). Failed and OOM-killed runs have separate
counts and still contribute to exceedances and worst cases; median, p90, and
unused-byte averages use only successful unwrapped measurements. Missing peaks
count as observations but do not contribute to measured statistics. Without
observations, this section shows a short empty-state line.

When a compilation cannot fit, it queues before newer requests, so a large
compilation still runs while other builds keep submitting small ones. Newer
requests may use capacity a waiting request cannot use when it next becomes
able to start, only when both CPU and memory fit the safe slack in every
shared scope. The memory calculation rounds completion subsets up in 16 MiB
units and subtracts their maximum rounding error; a calculation that exceeds
the bounded subset bitset refuses the pass. Within one scope this never delays an older request. When a request
waits on its own build or container limit, the existing rule already lets
other builds or containers start, which can rarely delay the waiting request
after a pass. Cancelling a waiter or killing its process releases its queue
position.

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

Containers sharing a cache directory default to the machine's CPU and physical
memory budgets for their common pool. A CPU quota or memory limit set on each
container's own cgroup does not lower that pool; one on a parent cgroup above
the containers does. Explicit `scheduler.cpus` and
`scheduler.memory` settings override that host budget. For example:

```toml
[scheduler]
cpus = 64
memory = "256GiB"
```

mbx also enforces a separate ceiling for each container. The ceiling is the
container's available CPUs, less `scheduler.reserve_cpus`, and a memory
ceiling. With the default memory budget, the memory ceiling is 85% of the
container's memory limit. A configured budget larger than the limit is also
held to 85% of it; a configured budget that fits within the limit applies as
configured. All builds in that container share the
ceiling; other containers draw from their own ceilings and the common host pool.
Pressure control remains local to each container. During an upgrade, older
compiler leases without a container identity count against every container
until they finish. Records from older wrappers reserve their full legacy
weight in cores and that weight times their original memory share in bytes.
New wrappers still write the original weight and memory share for older readers;
new records carry exact CPU and memory charges as well. Older mbx versions
read the same single-number charge as today, including its cap at the writing
process's own capacity; new versions read the exact cores and bytes. Upgrade
all wrappers sharing the pool to get ordered admission.

Set `scheduler.pressure = false` (`MBX_SCHEDULER_PRESSURE=0`) to admit
compilations on permits and memory estimates alone. Disabling the scheduler or
setting `scheduler.memory = "none"` also disables pressure control. If the
pressure probes fail, scheduling falls back to the same permit and estimate
checks.

Use `scheduler.priority = "low"` (`MBX_SCHEDULER_PRIORITY=low`) for an editor's
background check or CI on a shared machine. While normal-priority work is
waiting, low-priority builds leave a quarter of each budget available for it,
without withholding the whole CPU or memory budget.

## Reserve capacity for external work

Use `mbx reserve` for a command whose work consumes the same host CPU or
memory but does not run through mbx, such as a build in a VM or container:

```sh
mbx reserve --memory 6GiB --cpus 4 -- ./run-windows-build-in-vm.sh
```

CPU and memory requests reserve exactly their separate amounts; an omitted
resource reserves zero of that resource. The command waits until both amounts
fit, then a dedicated worker
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

By default, permits cover compilers only. When several test commands reach
their test runs together, each libtest harness can start threads across the
machine. Set `scheduler.tests = true` (`MBX_SCHEDULER_TESTS=1`) to run test
binaries through the same pool:

```sh
MBX_SCHEDULER_TESTS=1 mbx test --workspace   # this run
mbx settings set scheduler.tests true        # every run from now on
MBX_SCHEDULER_TESTS=1 mbx nextest run        # schedule each nextest test
```

mbx becomes Cargo's target runner for `cargo test` and `cargo nextest`, and
calls any runner you have configured. Cargo test binaries wait for permits as
a suite; nextest waits separately for each test. Listing tests does not take a
permit.

For Cargo test, each test binary waits for permits before it starts:

- Once a binary has been measured, it asks for the average number of cores it
  kept busy: CPU time divided by wall time, rounded to the nearest core.
- Before that, `--test-threads=N` or `RUST_TEST_THREADS=N` asks for `N`
  cores, and otherwise the binary asks for half the CPU budget, rounded up.
- Memory is charged separately using the binary's measured peak. Before a
  measurement exists, its memory guess preserves the old thread-based charge.

For Cargo test, only complete suite runs are measured; a run narrowed by a
test name, `--skip`, or `--ignored` is not. A run under a second is too short
to average, so the binary is recorded as one core. A recorded core count only goes up,
because a suite measured on a busy machine gets fewer cores than it would use.
mbx measures CPU and memory on Unix only. On Windows, a Cargo test binary asks
for its stated thread count or half the pool; an unmeasured nextest test asks
for one core and, when memory scheduling is on, two default memory shares.

History is kept per Git repository, package, and test binary. Worktrees of one
repository share it; separate clones and unrelated projects do not. Outside
Git, projects that share a package and test name share history. A run with a
stated thread count keeps its history apart from runs at the default thread
count.

Nextest history is kept separately for each Git repository, nextest binary ID,
and test name. A test with history uses its own measurements. A new test uses
the largest measurements of other tests in that binary; when the binary has
no history, it starts with one core and, when memory scheduling is on, two
memory permits. Successful tests record their measured peak even when they use
less than one default memory share, so later runs can use a smaller estimate. On Unix, peak
memory is the largest single process reported by `rusage`; a descendant's
peak is folded in when it exits, but concurrent child processes are not added
together.

nextest starts a test's `slow-timeout` clock, including any `terminate-after`
limit, when it starts mbx's runner. Time spent waiting for a permit therefore
counts as test time, and nextest offers runners no way to pause the clock. A
test that waits can be reported as SLOW, and with `terminate-after` it can be
killed before it starts. After one second of waiting, mbx writes a note to the
test's stderr, which nextest shows beside the report. If your nextest profile
sets `terminate-after`, leave room in it for queueing, or lower nextest's
`--test-threads` so fewer tests queue at once.

Builds a test starts, such as trybuild or compile-fail suites, are charged to
the test's permits and run without taking permits of their own.

These run unscheduled:

- doctests
- `cargo test --no-run`
- nextest test listings
- commands with `--config`, a `+toolchain` override, or a directory change
  (`-C`, `--directory`)
- tests run from `--archive-file` when their binaries target a triple other
  than the requested or host target
- test binaries run directly, without `cargo test` or `cargo nextest`
- other test runners

## Choose the scope of a limit

| Limit | Affects |
| --- | --- |
| `scheduler.cpus`, `scheduler.memory` | The shared compiler pool |
| `scheduler.reserve_cpus` | CPU cores left outside that pool |
| Cargo `-j` or `CARGO_BUILD_JOBS` | CPU cores one build may hold |
| `scheduler.priority = "low"` | Whether a build yields to waiting normal-priority work |
| `scheduler.tests = true` | Whether Cargo test binaries and nextest tests take permits |

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
