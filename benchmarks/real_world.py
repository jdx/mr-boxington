#!/usr/bin/env python3
"""Build a pinned real-world project through the situations CI actually hits.

`measure_builds.py` measures this workspace against itself, which answers
"did mbx regress" but not "what does mbx do to somebody else's build". This
driver clones a pinned third-party subject and runs the same Cargo invocation
under raw cargo, mbx, and kache across scenarios drawn from the live hk and
mise cache workflows: a warm store with a fresh target, the next commit on the
branch, and a one-line edit rebuilt in place.

Every timing is wall clock around one build, the way CI experiences it. A
timed scenario is repeated (`--trials`) and reports the median trial with
every trial's timing beside it, because a scenario whose tools finish within
the run-to-run spread has not measured a difference between them. Scenarios
that assert rather than race -- a second checkout, a changed compiler -- are
guards and publish no timing at all. The registry is fetched once up front and
shared, so no cell is timed while it downloads crates. Toolchains are pinned
per subject: a runner-image Rust bump changes every invocation digest at once,
which reads as a cache that stopped working. See `benchmarks/README.md`.
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

# Pinned subjects. `parent` and `child` are adjacent real commits: seeding at
# the parent and building the child is the push-to-push CI case, so the pair
# has to be a genuine source change rather than two arbitrary revisions.
SUBJECTS: dict[str, dict[str, object]] = {
    "hk": {
        "description": "jdx/hk, a mid-size Rust CLI with C dependencies",
        "url": "https://github.com/jdx/hk.git",
        "parent": "27bb615768b85c9ac88e2abf8895219b44462871",
        "child": "fc29ead1456ba7c1f62826c284126410a4014b00",
        # hk does not pin a toolchain of its own, so the benchmark pins one.
        # Without this the numbers stop being comparable across runner images.
        "toolchain": "1.97.1",
        "args": ["build", "--locked"],
        # The edit scenario appends a comment here. Named per subject rather
        # than discovered, so a moved pin fails loudly instead of quietly
        # editing some other crate than the one the numbers claim.
        "edit": "src/main.rs",
    },
}

TOOLS = ("cargo", "mbx-sequential", "mbx-unscheduled", "mbx", "kache")

# All three run the same binary. The contention scenario separates the current
# sequential lint shape, the proposed parallel shape, and a parallel control
# with the machine-wide scheduler off. Everywhere else they would just measure
# mbx repeatedly.
MBX_TOOLS = ("mbx", "mbx-sequential", "mbx-unscheduled")

# Overlapping compilation jobs from the check, lint, and test stages a Rust CI
# pipeline commonly stacks on one large runner. Two Cargo processes barely
# press a 30-vCPU machine; six distinct workloads create enough overlap to test
# whether the machine-wide pool helps without manufacturing identical jobs.
# Parallel cells need separate targets so Cargo's target lock does not
# serialize them; the sequential reference retains one shared target to show
# the shape a non-parallel job already gets.
CONTENTION_JOBS: tuple[tuple[str, list[str]], ...] = (
    ("check-default", ["check", "--locked"]),
    ("check-all", ["check", "--all-features", "--all-targets", "--locked"]),
    ("clippy-default", ["clippy", "--locked"]),
    ("clippy-all", ["clippy", "--all-features", "--all-targets", "--locked"]),
    ("test-default", ["test", "--no-run", "--locked"]),
    (
        "test-all",
        ["test", "--all-features", "--all-targets", "--no-run", "--locked"],
    ),
)

# Which tools each scenario asks for, and whether it publishes a timing.
#
# `repeatable` is what --trials multiplies. A scenario is only worth timing if
# the difference it reports can outrun the spread of repeating it, so a timed
# scenario has to be cheap enough to repeat; one that cannot be repeated is
# either a guard or the contention batch, which measures a machine rather than
# a race. cargo appears only where a no-cache baseline is meaningful: "warm"
# describes a cache being reused, and cargo would just repeat its cold number.
SCENARIOS: dict[str, dict[str, object]] = {
    "warm": {
        "tools": ("mbx", "kache"),
        "description": (
            "warm store, fresh target -- CI restoring the same commit; "
            "plain Cargo has no cache to restore"
        ),
        "repeatable": True,
    },
    "commit": {
        "tools": ("cargo", "mbx", "kache"),
        "description": (
            "cache tools warmed at the parent commit, build the child -- "
            "Cargo is the uncached push-to-push baseline"
        ),
        "repeatable": True,
    },
    "edit": {
        "tools": ("cargo", "mbx", "kache"),
        "description": (
            "one line changed and rebuilt in place, incremental on -- "
            "the local edit loop, where Cargo is the thing to beat"
        ),
        "repeatable": True,
    },
    "worktree": {
        "tools": ("mbx",),
        "description": (
            "store warmed in one checkout, rebuilt in a second at another path"
        ),
        "timed": False,
    },
    "toolchain": {
        "tools": ("mbx",),
        "description": "store warmed on the pinned toolchain, rebuild on another",
        "timed": False,
    },
    "contention": {
        "tools": ("mbx-sequential", "mbx-unscheduled", "mbx"),
        "description": (
            "six overlapping check, Clippy, and test jobs -- sequential for context, "
            "then parallel with and without mbx's machine-wide compiler limit"
        ),
        "kind": "contention",
    },
}


# What fraction of a manifest's predictions may still be looked up after the
# compiler changes. Anything above this means rustc work survived a change that
# invalidates every invocation digest.
TOOLCHAIN_SURVIVOR_LIMIT = 0.05


class Skipped(Exception):
    """A cell could not run for a reason that is not a benchmark failure."""


class MachineSampler:
    """What the whole machine was doing while a batch of builds ran.

    Cargo bounds the compilers *it* starts; nothing bounds the compilers of
    the job next to it, which is the problem the machine-wide scheduler
    exists to solve. So the measurement has to be taken from outside every
    build -- how many real compilers existed at once, and how close the
    machine came to running out of memory while they did.
    """

    INTERVAL = 0.05

    def __init__(self) -> None:
        self.peak_compilers = 0
        self.min_available_bytes: int | None = None
        self.samples = 0
        self._stop = threading.Event()
        self._thread = threading.Thread(target=self._sample, daemon=True)

    def __enter__(self) -> MachineSampler:
        self._thread.start()
        return self

    def __exit__(self, *_: object) -> None:
        self._stop.set()
        self._thread.join(timeout=5)

    def _sample(self) -> None:
        while not self._stop.is_set():
            compilers = count_compilers()
            if compilers is not None:
                self.peak_compilers = max(self.peak_compilers, compilers)
            available = available_memory_bytes()
            if available is not None:
                self.min_available_bytes = (
                    available
                    if self.min_available_bytes is None
                    else min(self.min_available_bytes, available)
                )
            self.samples += 1
            self._stop.wait(self.INTERVAL)


def count_compilers() -> int | None:
    """Real compiler processes running anywhere on this machine, or None.

    Counted by what the process actually is rather than by what started it:
    a shim is invoked under its own name and execs nothing, so matching the
    `rustc` executable itself is what separates the compilations from the
    wrappers in front of them. Cargo's target-info and stdin probes carry
    `--crate-name` too and compile nothing, so `--print` and a standalone
    stdin argument are excluded just as the scheduler excludes them.
    """
    try:
        listing = subprocess.run(
            ["ps", "-Ao", "args="], text=True, capture_output=True, timeout=5
        )
    except (OSError, subprocess.SubprocessError):
        return None
    if listing.returncode != 0:
        return None
    count = 0
    for line in listing.stdout.splitlines():
        arguments = line.split()
        has_crate_name = any(
            argument == "--crate-name" or argument.startswith("--crate-name=")
            for argument in arguments
        )
        if not arguments or (
            not has_crate_name
            or any(argument.startswith("--print") for argument in arguments)
            or "-" in arguments
        ):
            continue
        executable = arguments[0]
        if Path(executable).name in ("rustc", "rustc.exe"):
            count += 1
    return count


def available_memory_bytes() -> int | None:
    """Memory the machine could still hand out, on hosts that report it.

    `MemAvailable` counts reclaimable memory rather than free pages alone,
    which is the number that says whether another compiler would fit. Only
    Linux publishes it, and Linux is what CI runs; elsewhere the scenario
    reports concurrency without the memory column rather than guessing.
    """
    try:
        with open("/proc/meminfo", encoding="utf-8") as meminfo:
            for line in meminfo:
                if line.startswith("MemAvailable:"):
                    return int(line.split()[1]) * 1024
    except OSError:
        return None
    return None


def tool_version(command: str, toolchain: str | None = None) -> str | None:
    """What one tool calls itself, under the toolchain the builds used.

    `cargo` and `rustc` are rustup shims, so asking them without pinning
    reports the machine's default -- which is exactly the version the timed
    builds did not use.
    """
    executable = shutil.which(command)
    if executable is None:
        return None
    environment = os.environ.copy()
    if toolchain is not None:
        environment["RUSTUP_TOOLCHAIN"] = toolchain
    for flag in ("--version", "-V"):
        try:
            return (
                subprocess.check_output([executable, flag], text=True, env=environment)
                .strip()
                .splitlines()[0]
            )
        except (subprocess.CalledProcessError, OSError):
            continue
    return None


def git(*args: str | Path, cwd: Path | None = None) -> None:
    # --quiet goes right after the subcommand, never after a revision, where
    # checkout would read it as a pathspec. A clone per cell would otherwise
    # bury the phase progress this prints between builds.
    subcommand, *rest = [str(arg) for arg in args]
    subprocess.run(["git", subcommand, "--quiet", *rest], cwd=cwd, check=True)


def clone(subject: dict[str, object], revision: str, destination: Path) -> None:
    """Materialize the subject at one revision.

    A full clone is fetched once into a cache and every checkout is created
    from it, so the network is touched once per invocation rather than once
    per cell.
    """
    destination.parent.mkdir(parents=True, exist_ok=True)
    git("clone", "--no-checkout", "--shared", str(subject["mirror"]), destination)
    git("checkout", "--detach", revision, cwd=destination)


class Runner:
    """Runs one build and reports what the cache did with it."""

    def __init__(self, output: Path, cargo_home: Path, mbx: Path | None) -> None:
        self.output = output
        self.cargo_home = cargo_home
        self.mbx = mbx

    def base_environment(
        self,
        subject: dict[str, object],
        target: Path,
        incremental: bool = False,
    ) -> dict[str, str]:
        environment = os.environ.copy()
        environment.update(
            {
                "CARGO_HOME": str(self.cargo_home),
                "CARGO_TARGET_DIR": str(target),
                # Off for the CI scenarios, matching what CI sets. The edit
                # loop is the exception and turns it on: incremental state is
                # most of what makes a developer's rebuild fast, and measuring
                # that loop without it would compare against a cargo nobody
                # runs.
                "CARGO_INCREMENTAL": "1" if incremental else "0",
                "CARGO_TERM_COLOR": "never",
                "RUSTUP_TOOLCHAIN": str(subject["toolchain"]),
            }
        )
        # A wrapper inherited from the caller's shell would silently cache the
        # supposedly uncached baseline.
        for variable in ("RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"):
            environment.pop(variable, None)
        return environment

    def invocation(
        self,
        *,
        tool: str,
        cell: str,
        subject: dict[str, object],
        target: Path,
        store: Path,
        args: list[str] | None = None,
        toolchain: str | None = None,
        incremental: bool = False,
    ) -> tuple[list[str], dict[str, str]]:
        """The command and environment one cell runs the subject under.

        Shared by the single-build scenarios and the contention batch, so a
        parallel job is launched exactly the way a lone one is -- otherwise the
        two shapes could drift and the comparison between them would stop
        meaning anything.
        """
        environment = self.base_environment(subject, target, incremental)
        if toolchain is not None:
            environment["RUSTUP_TOOLCHAIN"] = toolchain
        args = list(subject["args"]) if args is None else args  # type: ignore[arg-type]

        if tool == "cargo":
            return ["cargo", *args], environment
        if tool in MBX_TOOLS:
            if self.mbx is None:
                raise Skipped("no mbx binary was given (--mbx)")
            environment.update(
                {
                    "MBX_CACHE_DIR": str(store),
                    "MBX_STATS_REPORT": str(self.output / f"{cell}-stats.json"),
                }
            )
            # The scheduler is on by default, so the unscheduled variant is
            # the one that has to say so. Both name it explicitly: an
            # inherited MBX_SCHEDULER would otherwise decide the comparison
            # the scenario exists to make.
            environment["MBX_SCHEDULER"] = "0" if tool == "mbx-unscheduled" else "1"
            return [str(self.mbx), *args], environment
        if tool == "kache":
            kache = shutil.which("kache")
            if kache is None:
                raise Skipped("kache is not on PATH")
            environment.update(
                {
                    "RUSTC_WRAPPER": kache,
                    "KACHE_CACHE_DIR": str(store),
                    "KACHE_RUNTIME_DIR": str(store.parent / f"{store.name}-runtime"),
                    # The benchmark measures local caching; a remote would
                    # measure a network instead, and mbx is run local-only too.
                    "KACHE_LOCAL_ONLY": "1",
                }
            )
            return ["cargo", *args], environment
        raise ValueError(f"unknown tool {tool}")

    def run(
        self,
        *,
        tool: str,
        cell: str,
        subject: dict[str, object],
        checkout: Path,
        target: Path,
        store: Path,
        toolchain: str | None = None,
        incremental: bool = False,
        fresh_target: bool = True,
    ) -> dict[str, object]:
        """Run one build and return its timing plus whatever the tool reported."""
        # Every scenario but the edit loop starts from a fresh target, which is
        # what a CI runner has. The edit loop keeps the one its seed build
        # produced, because rebuilding in place is the thing being measured.
        if fresh_target:
            shutil.rmtree(target, ignore_errors=True)
        extra: dict[str, object] = {}
        command, environment = self.invocation(
            tool=tool,
            cell=cell,
            subject=subject,
            target=target,
            store=store,
            toolchain=toolchain,
            incremental=incremental,
        )

        started = time.perf_counter_ns()
        completed = subprocess.run(
            command,
            cwd=checkout,
            env=environment,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        duration_ns = time.perf_counter_ns() - started

        log = self.output / f"{cell}.log"
        log.write_text(
            f"$ {' '.join(command)}\n\n{completed.stdout}\n{completed.stderr}", encoding="utf-8"
        )
        if completed.returncode != 0:
            raise RuntimeError(f"{cell}/{tool} build failed; see {log}")

        if tool in MBX_TOOLS:
            report = self.output / f"{cell}-stats.json"
            if report.is_file():
                extra["stats"] = json.loads(report.read_text(encoding="utf-8"))
            extra["summary"] = completed.stderr.strip().splitlines()[-8:]
        elif tool == "kache":
            # kache's stats output is prose we do not own; record it verbatim
            # rather than parsing a format that is free to change.
            stats = subprocess.run(
                [shutil.which("kache") or "kache", "stats"],
                cwd=checkout,
                env=environment,
                text=True,
                capture_output=True,
            )
            extra["summary"] = stats.stdout.strip().splitlines()
            # Each cell points kache at its own runtime directory, so each one
            # starts its own daemon. Stopping it keeps the cells independent
            # and stops a draining daemon from writing into a tree that is
            # about to be removed.
            subprocess.run(
                [shutil.which("kache") or "kache", "daemon", "stop"],
                cwd=checkout,
                env=environment,
                text=True,
                capture_output=True,
                check=False,
            )

        # An edit that failed to invalidate anything would render as a very
        # fast rebuild rather than as a broken scenario, so the gate needs to
        # know whether a compiler ran at all.
        extra["recompiled"] = "Compiling " in completed.stderr
        if tool == "cargo":
            # Clearing the wrapper variables is not proof that they were the
            # only way in. A `cargo` that is itself a shim for mbx, which is
            # what a developer running this on their own machine has, produces
            # a baseline that quietly benefits from the cache it is supposed
            # to be the control for -- and the faster that baseline looks, the
            # worse mbx looks, so nothing about the published numbers would
            # give it away.
            extra["wrapped"] = "mbx[" in completed.stderr
        return {"tool": tool, "wall_duration_ns": duration_ns, **extra}

    def run_batch(
        self,
        *,
        tool: str,
        cell: str,
        subject: dict[str, object],
        checkout: Path,
        work: Path,
        store: Path,
        permits: int,
    ) -> dict[str, object]:
        """Start every CI job at once and measure what the machine did.

        The batch is timed end to end rather than per job: what a developer
        waiting on CI experiences is when the last one finishes, and what the
        machine experiences is all of them at their overlap.
        """
        jobs = []
        prepared_targets: set[Path] = set()
        for name, args in CONTENTION_JOBS:
            # This is the before/after the real lint job experiences. Its
            # sequential commands reuse one Cargo target. Concurrent Cargo
            # processes instead need separate targets or Cargo's directory
            # lock turns the allegedly parallel run back into a serial one.
            target_name = "shared" if tool == "mbx-sequential" else name
            target = work / f"target-{cell}-{target_name}"
            if target not in prepared_targets:
                shutil.rmtree(target, ignore_errors=True)
                prepared_targets.add(target)
            command, environment = self.invocation(
                tool=tool,
                cell=f"{cell}-{name}",
                subject=subject,
                target=target,
                store=store,
                args=args,
            )
            if tool in MBX_TOOLS:
                # Stated rather than left to the default, so the bound the
                # scenario checks is the bound it asked for -- mbx counts the
                # CPUs it can actually use, which a container may limit.
                environment["MBX_SCHEDULER_CPUS"] = str(permits)
            jobs.append((name, command, environment))

        # Straight to files rather than pipes. A pipe holds 64KiB, and these
        # jobs are only read after they are all started, so a chatty one would
        # block on a full buffer instead of compiling -- which would land in
        # the timing as though the machine had been busy.
        logs = {name: self.output / f"{cell}-{name}.log" for name, _ in CONTENTION_JOBS}
        started = time.perf_counter_ns()
        with MachineSampler() as sampler:
            running = []
            for name, command, environment in jobs:
                handle = logs[name].open("w", encoding="utf-8")
                handle.write(f"$ {' '.join(command)}\n\n")
                handle.flush()
                process = subprocess.Popen(
                    command,
                    cwd=checkout,
                    env=environment,
                    stdout=handle,
                    stderr=subprocess.STDOUT,
                )
                running.append((name, process, handle))
                if tool == "mbx-sequential":
                    # Wait here rather than in the common loop below so the
                    # next Cargo process cannot overlap this one.
                    process.wait()
            finished = []
            for name, process, handle in running:
                returncode = process.wait()
                handle.close()
                finished.append((name, returncode))
        duration_ns = time.perf_counter_ns() - started

        job_results = []
        for name, returncode in finished:
            if returncode != 0:
                raise RuntimeError(f"{cell}/{tool} job {name} failed; see {logs[name]}")
            job_results.append({"job": name})

        result: dict[str, object] = {
            "tool": tool,
            "wall_duration_ns": duration_ns,
            "jobs": job_results,
            "peak_compilers": sampler.peak_compilers,
            "min_available_bytes": sampler.min_available_bytes,
            "permits": permits if tool == "mbx" else None,
            "samples": sampler.samples,
        }
        if tool in MBX_TOOLS:
            # One report per job; the batch's hits are their sum, since the
            # jobs shared a store and each one's hits are real restorations.
            total = 0
            for name, _ in CONTENTION_JOBS:
                report = self.output / f"{cell}-{name}-stats.json"
                if report.is_file():
                    total += int(
                        json.loads(report.read_text(encoding="utf-8")).get("hits", 0)
                    )
            result["stats"] = {"hits": total}
        return result


# What the checked-in results file keeps from each mbx stats report. The full
# report is in the run's own output directory; the published file is reviewed
# by a person in a pull request, so it carries what a reader needs to judge the
# timing beside it and nothing else.
PUBLISHED_STATS = (
    "lookups",
    "hits",
    "misses",
    "unconsulted",
    "predictions_loaded",
    "restored_output_files",
    "restored_output_bytes",
)

# What a contention cell publishes beyond its timing: the machine measurements
# are the point of that scenario, not a detail of it.
PUBLISHED_CONTENTION = ("peak_compilers", "min_available_bytes", "permits")

# Every trial's timing, published rather than reduced away. A reader cannot
# tell whether a gap between two tools means anything without knowing how far
# the same tool moved between its own runs, and the page suppresses the
# "fastest" label when the gap is inside that spread.
PUBLISHED_TRIALS = ("trials", "wall_durations_ns")


class Progress:
    """Durable progress lines for long-running benchmark logs."""

    def __init__(self, total: int, width: int = 24) -> None:
        self.total = total
        self.width = width
        self.completed = 0
        self.started = 0.0

    def bar(self) -> str:
        filled = self.width * self.completed // self.total
        percent = 100 * self.completed // self.total
        return (
            f"[{'#' * filled}{'-' * (self.width - filled)}] "
            f"{self.completed}/{self.total} ({percent}%)"
        )

    def preparing(self) -> None:
        print(f"{self.bar()} preparing benchmark subject", file=sys.stderr, flush=True)

    def start(self, cell: str) -> None:
        self.started = time.perf_counter()
        print(f"{self.bar()} running {cell}", file=sys.stderr, flush=True)

    def finish(self, cell: str, outcome: str = "completed") -> None:
        elapsed = time.perf_counter() - self.started
        self.completed += 1
        print(
            f"{self.bar()} {outcome} {cell} in {elapsed:.1f}s",
            file=sys.stderr,
            flush=True,
        )


def publishable(result: dict[str, object]) -> dict[str, object]:
    """Strip a run down to what the documentation site publishes."""
    scenarios = []
    for scenario in result["scenarios"]:  # type: ignore[index]
        cells = []
        for cell in scenario["results"]:
            trimmed = {
                "tool": cell["tool"],
                "wall_duration_ns": cell["wall_duration_ns"],
            }
            for field in PUBLISHED_CONTENTION + PUBLISHED_TRIALS:
                if field in cell:
                    trimmed[field] = cell[field]
            stats = cell.get("stats")
            if isinstance(stats, dict):
                trimmed["stats"] = {
                    field: stats[field] for field in PUBLISHED_STATS if field in stats
                }
            cells.append(trimmed)
        scenarios.append({**scenario, "results": cells})
    return {**result, "scenarios": scenarios}


def hits(cell: dict[str, object]) -> int:
    stats = cell.get("stats")
    if isinstance(stats, dict):
        return int(stats.get("hits", 0))
    return 0


def restored_files(cell: dict[str, object]) -> int:
    stats = cell.get("stats")
    if isinstance(stats, dict):
        return int(stats.get("restored_output_files", 0))
    return 0


def touch_source(subject: dict[str, object], checkout: Path) -> None:
    """Make the one-line source change the edit loop is built around.

    A trailing comment is enough: rustc recompiles the crate for any change to
    the file, and a larger edit would measure a different amount of the
    subject's own code every time the pinned revision moves.
    """
    source = checkout / str(subject["edit"])
    if not source.is_file():
        raise RuntimeError(f"{subject['edit']} is not in the pinned checkout")
    with source.open("a", encoding="utf-8") as handle:
        handle.write("\n// mbx benchmark edit\n")


def one_trial(
    scenario: str,
    tool: str,
    cell: str,
    subject: dict[str, object],
    runner: Runner,
    work: Path,
) -> dict[str, object]:
    """Run one tool through one scenario once, in a store and checkout of its own.

    Trials share nothing. A second trial that started from the first one's
    store would be measuring a different scenario than the first, which is the
    one thing a repeated measurement must not do.
    """
    store = work / f"store-{cell}"
    target = work / f"target-{cell}"

    if scenario == "warm":
        checkout = work / f"checkout-{cell}"
        clone(subject, str(subject["child"]), checkout)
        seed = runner.run(
            tool=tool,
            cell=f"{cell}-seed",
            subject=subject,
            checkout=checkout,
            target=target,
            store=store,
        )
        measured = runner.run(
            tool=tool,
            cell=cell,
            subject=subject,
            checkout=checkout,
            target=target,
            store=store,
        )
        # The seed is this tool's own cold build: an empty store, a fresh
        # target, the same checkout and machine. Carrying its timing is what
        # lets a warm build still be gated against a cold one without
        # publishing a cold scenario whose tools finish within a second of
        # each other.
        measured["seed_wall_duration_ns"] = seed["wall_duration_ns"]
        return measured

    if scenario == "commit":
        checkout = work / f"checkout-{cell}"
        if tool != "cargo":
            clone(subject, str(subject["parent"]), checkout)
            runner.run(
                tool=tool,
                cell=f"{cell}-seed",
                subject=subject,
                checkout=checkout,
                target=target,
                store=store,
            )
            git("checkout", "--detach", str(subject["child"]), cwd=checkout)
        else:
            # cargo has no store to seed and its target is wiped before every
            # run, so seeding it would only measure a cold build twice. The
            # baseline for this scenario is a cold build.
            clone(subject, str(subject["child"]), checkout)
        return runner.run(
            tool=tool,
            cell=cell,
            subject=subject,
            checkout=checkout,
            target=target,
            store=store,
        )

    if scenario == "edit":
        checkout = work / f"checkout-{cell}"
        clone(subject, str(subject["child"]), checkout)
        # The seed leaves its target in place, which is the whole scenario: a
        # developer edits inside the tree they just built, and the incremental
        # state in that target is what the rebuild is racing against. cargo is
        # the baseline here rather than a control, because a cache that slows
        # this loop down is a cache nobody leaves switched on.
        runner.run(
            tool=tool,
            cell=f"{cell}-seed",
            subject=subject,
            checkout=checkout,
            target=target,
            store=store,
            incremental=True,
        )
        touch_source(subject, checkout)
        return runner.run(
            tool=tool,
            cell=cell,
            subject=subject,
            checkout=checkout,
            target=target,
            store=store,
            incremental=True,
            fresh_target=False,
        )

    if scenario == "worktree":
        seed = work / f"checkout-{cell}-seed"
        clone(subject, str(subject["child"]), seed)
        runner.run(
            tool=tool,
            cell=f"{cell}-seed",
            subject=subject,
            checkout=seed,
            target=target,
            store=store,
        )
        # A different path, which is the whole point: absolute paths must not
        # have entered the keys. What this reports is the hits, not the
        # seconds. A cache that keyed on paths restores nothing here, and that
        # is a pass or a fail rather than a race.
        other = work / f"checkout-{cell}-elsewhere"
        clone(subject, str(subject["child"]), other)
        return runner.run(
            tool=tool,
            cell=cell,
            subject=subject,
            checkout=other,
            target=work / f"target-{cell}-elsewhere",
            store=store,
        )

    if scenario == "toolchain":
        alternate = os.environ.get("MBX_BENCH_ALTERNATE_TOOLCHAIN")
        if not alternate:
            raise Skipped("set MBX_BENCH_ALTERNATE_TOOLCHAIN to a second installed Rust")
        if tool_version("rustc", alternate) == tool_version("rustc", str(subject["toolchain"])):
            raise Skipped(
                f"MBX_BENCH_ALTERNATE_TOOLCHAIN ({alternate}) resolves to the "
                f"pinned {subject['toolchain']}, so nothing would change"
            )
        checkout = work / f"checkout-{cell}"
        clone(subject, str(subject["child"]), checkout)
        runner.run(
            tool=tool,
            cell=f"{cell}-seed",
            subject=subject,
            checkout=checkout,
            target=target,
            store=store,
        )
        return runner.run(
            tool=tool,
            cell=cell,
            subject=subject,
            checkout=checkout,
            target=target,
            store=store,
            toolchain=alternate,
        )

    if scenario == "contention":
        checkout = work / f"checkout-{cell}"
        clone(subject, str(subject["child"]), checkout)
        return runner.run_batch(
            tool=tool,
            cell=cell,
            subject=subject,
            checkout=checkout,
            work=work,
            store=store,
            permits=contention_permits(),
        )

    raise ValueError(f"unknown scenario {scenario}")


def discard(work: Path, cell: str) -> None:
    """Drop one cell's checkouts, targets, and store once it has been measured.

    Every scenario used to hold its scratch trees until the whole run ended,
    which was affordable while each tool ran a scenario once. Repeating them
    multiplies it, and the edit scenario keeps a target with incremental state
    in it, which is the largest tree the benchmark produces. Nothing after the
    trial reads any of it: the statistics report and the build logs are
    written under --output, which is not this directory.
    """
    for path in work.iterdir():
        if path.name.endswith(f"-{cell}") or f"-{cell}-" in path.name:
            shutil.rmtree(path, ignore_errors=True)


def median_cell(cells: list[dict[str, object]]) -> dict[str, object]:
    """One tool's trials reduced to the middle one, with all of them kept.

    The middle trial rather than an averaged number, so the statistics
    published beside a timing come from the build that actually produced it.
    An even number of trials takes the lower middle. Every trial's timing is
    carried through to the published file: the spread is what tells a reader
    which differences on the page are real, and it is not the benchmark's
    place to hide it.
    """
    ordered = sorted(cells, key=lambda cell: int(cell["wall_duration_ns"]))
    chosen = dict(ordered[(len(ordered) - 1) // 2])
    chosen["trials"] = len(cells)
    chosen["wall_durations_ns"] = [int(cell["wall_duration_ns"]) for cell in cells]
    return chosen


def guard_summary(scenario: str, results: list[dict[str, object]]) -> str | None:
    """What an untimed scenario asserted, in the words the page renders.

    Written here rather than in the template because the claim belongs with
    the code that checked it.
    """
    if not results:
        return None
    stats = results[0].get("stats")
    if not isinstance(stats, dict):
        return None
    if scenario == "toolchain":
        return (
            f"Of {stats.get('predictions_loaded', 0)} predicted compilations, a different "
            f"compiler let mbx look up {stats.get('lookups', 0)}: the ones that do not "
            "depend on rustc at all."
        )
    if scenario == "worktree":
        return (
            f"A second checkout at a different path restored "
            f"{stats.get('restored_output_files', 0)} output files on "
            f"{stats.get('hits', 0)} cache hits, so no absolute path reached the keys."
        )
    return None


def run_scenario(
    scenario: str,
    tools: tuple[str, ...],
    subject: dict[str, object],
    runner: Runner,
    work: Path,
    progress: Progress,
    trials: int = 1,
) -> dict[str, object]:
    """Run every tool through one scenario, repeating the ones worth repeating."""
    repeats = trials if SCENARIOS[scenario].get("repeatable") else 1
    results: list[dict[str, object]] = []
    notes: list[str] = []
    for tool in tools:
        measured: list[dict[str, object]] = []
        for trial in range(1, repeats + 1):
            cell = f"{scenario}-{tool}" if repeats == 1 else f"{scenario}-{tool}-{trial}"
            progress.start(cell)
            try:
                measured.append(one_trial(scenario, tool, cell, subject, runner, work))
            except Skipped as skip:
                notes.append(f"{tool}: {skip}")
                progress.finish(cell, "skipped")
                break
            except RuntimeError as failure:
                # A tool we do not own failing a scenario is a fact about that
                # tool, not a reason to publish nothing. It is reported as
                # unmeasured, the same as a tool that was not installed. mbx
                # and the cargo baseline still fail the run: those are ours,
                # and a broken harness must not look like a competitor's
                # limitation.
                if tool in MBX_TOOLS or tool == "cargo":
                    raise
                notes.append(f"{tool}: {failure}")
                progress.finish(cell, "failed")
                break
            else:
                progress.finish(cell)
                discard(work, cell)
        if measured:
            results.append(median_cell(measured))

    entry: dict[str, object] = {
        "scenario": scenario,
        "description": SCENARIOS[scenario]["description"],
        "timed": SCENARIOS[scenario].get("timed", True),
        "kind": SCENARIOS[scenario].get("kind", "build"),
        "results": results,
        "skipped": notes,
    }
    if not entry["timed"]:
        entry["guard"] = guard_summary(scenario, results)
    return entry


def contention_permits() -> int:
    """Permits the scheduled cell is given: what an unconfigured machine gets.

    The scenario is about the default anyone would actually run under, so this
    is the CPU count rather than a number chosen to make the bound look tight.
    """
    try:
        # Python 3.12's os.cpu_count() reports the host total inside a cpuset.
        # Affinity is the set this benchmark process can actually schedule on.
        return len(os.sched_getaffinity(0))
    except AttributeError:
        return os.cpu_count() or 1


def validate(scenarios: list[dict[str, object]]) -> list[str]:
    """Reject a run whose numbers cannot mean what the site would say they do.

    A benchmark that quietly measured nothing is worse than one that failed,
    because its numbers still render.
    """
    failures: list[str] = []
    by_name = {entry["scenario"]: entry for entry in scenarios}

    for name in ("warm", "worktree"):
        entry = by_name.get(name)
        if entry is None:
            continue
        for cell in entry["results"]:  # type: ignore[index]
            if cell["tool"] != "mbx":
                continue
            if hits(cell) <= 0 or restored_files(cell) <= 0:
                failures.append(f"{name}: mbx restored nothing, so the timing is not a cache result")

    warm = by_name.get("warm")
    if warm:
        # Against the build that seeded it rather than against a published
        # cold scenario: same tool, same checkout, same trial, empty store.
        # That is a stricter comparison than the cold cards ever were, and it
        # costs no extra build.
        for cell in warm["results"]:  # type: ignore[index]
            seed = cell.get("seed_wall_duration_ns")
            if seed is not None and int(cell["wall_duration_ns"]) >= int(seed):
                failures.append(
                    f"warm: {cell['tool']} was no faster than the cold build that seeded it"
                )

    edit = by_name.get("edit")
    if edit:
        # A rebuild that compiled nothing is a fast number with no content: it
        # would mean the edit never reached the compiler, not that the tool
        # rebuilt quickly.
        for cell in edit["results"]:  # type: ignore[index]
            if not cell.get("recompiled"):
                failures.append(
                    f"edit: {cell['tool']} rebuilt without compiling anything, so the "
                    "edit never reached the compiler"
                )

    for entry in scenarios:
        for cell in entry["results"]:  # type: ignore[index]
            if cell.get("wrapped"):
                failures.append(
                    f"{entry['scenario']}: the cargo baseline ran under mbx, so it is "
                    "not the uncached control the scenario compares against"
                )

    contention = by_name.get("contention")
    if contention:
        cells = {cell["tool"]: cell for cell in contention["results"]}  # type: ignore[index]
        scheduled = cells.get("mbx")
        unscheduled = cells.get("mbx-unscheduled")
        if scheduled is not None:
            permits = int(scheduled.get("permits") or 0)
            peak = int(scheduled.get("peak_compilers") or 0)
            if not scheduled.get("samples"):
                failures.append(
                    "contention: the machine was never sampled, so no bound was observed"
                )
            elif peak <= 0:
                failures.append(
                    "contention: no compiler was ever seen running, so the sampler "
                    "measured something other than the builds"
                )
            elif permits and peak > permits:
                failures.append(
                    f"contention: {peak} compilers ran at once against {permits} permits, "
                    "so the pool did not bound the machine"
                )
        if scheduled is not None and unscheduled is not None:
            # Without this the scenario could "pass" on a machine too small,
            # or too fast, for the jobs to ever overlap -- and a bound nothing
            # pushed against proves nothing about the bound.
            baseline = int(unscheduled.get("peak_compilers") or 0)
            permits = int(scheduled.get("permits") or 0)
            if permits and baseline <= permits:
                failures.append(
                    f"contention: unscheduled builds peaked at {baseline} compilers, within "
                    f"the {permits} permits, so this run never actually contended"
                )
    toolchain = by_name.get("toolchain")
    if toolchain:
        # A skipped guard is not a passed guard. The scenario was asked for, so
        # a run that could not perform it must not publish as though the
        # compiler-invalidation claim had been checked.
        measured = [
            cell
            for cell in toolchain["results"]  # type: ignore[index]
            if cell["tool"] == "mbx" and isinstance(cell.get("stats"), dict)
        ]
        if not measured:
            failures.append(
                "toolchain: the compiler-change guard did not run, so nothing "
                "checked that a new rustc invalidates the cache"
            )
        for cell in measured:
            stats = cell["stats"]
            assert isinstance(stats, dict)
            predictions = int(stats.get("predictions_loaded", 0))
            if predictions <= 0:
                failures.append(
                    "toolchain: no predictions were loaded, so the rebuild was cold for "
                    "some reason other than the compiler change"
                )
                continue
            # Not zero lookups: a handful of actions do not depend on rustc at
            # all -- a build script's C object is compiled by the C compiler,
            # which did not change -- and those legitimately still hit. What
            # must not survive is the rustc work the manifest predicted.
            if int(stats.get("lookups", 0)) > predictions * TOOLCHAIN_SURVIVOR_LIMIT:
                failures.append(
                    "toolchain: a different compiler reused rustc work it should have "
                    "invalidated"
                )

    return failures


def trial_range(cell: dict[str, object]) -> str:
    """How far one tool moved between its own runs, for the job summary."""
    durations = cell.get("wall_durations_ns")
    if not isinstance(durations, list) or len(durations) < 2:
        return "1"
    return f"{len(durations)} ({min(durations) / 1e9:.1f}-{max(durations) / 1e9:.1f} s)"


def summarize(result: dict[str, object]) -> str:
    lines = [
        f"## Real-world benchmark: {result['subject']}",
        "",
        f"{SUBJECTS[str(result['subject'])]['description']}, "
        f"`cargo {' '.join(SUBJECTS[str(result['subject'])]['args'])}`"  # type: ignore[arg-type]
        f" on Rust {result['toolchain']}.",
        "",
    ]
    for scenario in result["scenarios"]:  # type: ignore[index]
        lines += [
            f"### {scenario['scenario']}",
            "",
            f"{scenario['description']}",
            "",
        ]
        if scenario.get("kind") == "contention":
            lines += [
                "| Tool | Batch wall time | Peak compilers | Lowest free memory | Hits |",
                "| :--- | ---: | ---: | ---: | ---: |",
            ]
            for cell in scenario["results"]:
                permits = cell.get("permits")
                peak = f"{cell.get('peak_compilers', 0)}"
                if permits:
                    peak += f" / {permits} permits"
                # Zero is a reading, not a missing one: it says the machine
                # ran itself out, which is the whole point of the column.
                available = cell.get("min_available_bytes")
                memory = "-" if available is None else f"{available / 1e9:.1f} GB"
                lines.append(
                    f"| {cell['tool']} | {cell['wall_duration_ns'] / 1e9:.1f} s | {peak} "
                    f"| {memory} | {hits(cell)} |"
                )
            lines.append("")
            for note in scenario["skipped"]:
                lines.append(f"- skipped {note}")
            if scenario["skipped"]:
                lines.append("")
            continue
        if scenario.get("guard"):
            lines += [str(scenario["guard"]), ""]
        lines += [
            "| Tool | Median wall time | Trials | Hits | Restored files |"
            if scenario["timed"]
            else "| Tool | Ran for | Trials | Hits | Restored files |",
            "| :--- | ---: | ---: | ---: | ---: |",
        ]
        for cell in scenario["results"]:
            stats = cell.get("stats")
            hit = str(hits(cell)) if isinstance(stats, dict) else "-"
            files = str(restored_files(cell)) if isinstance(stats, dict) else "-"
            lines.append(
                f"| {cell['tool']} | {cell['wall_duration_ns'] / 1e9:.1f} s "
                f"| {trial_range(cell)} | {hit} | {files} |"
            )
        lines.append("")
        for note in scenario["skipped"]:
            lines.append(f"- skipped {note}")
        if scenario["skipped"]:
            lines.append("")
    if result["failures"]:
        lines.append("Validity failures:")
        lines += [f"- {failure}" for failure in result["failures"]]
        lines.append("")
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--subject", default="hk", choices=sorted(SUBJECTS))
    parser.add_argument("--tools", default=",".join(TOOLS))
    parser.add_argument("--scenarios", default="warm,commit,edit")
    parser.add_argument(
        "--trials",
        type=int,
        default=1,
        help=(
            "times to repeat each timed scenario; the median trial is published "
            "with every trial's timing beside it (CI uses 3)"
        ),
    )
    parser.add_argument("--mbx", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument(
        "--write-results",
        type=Path,
        help="rewrite this checked-in results file with the run (the docs site reads it)",
    )
    args = parser.parse_args()

    if args.trials < 1:
        parser.error("--trials must be at least 1")

    requested_tools = tuple(name.strip() for name in args.tools.split(",") if name.strip())
    unknown = set(requested_tools) - set(TOOLS)
    if unknown:
        parser.error(f"unknown tools: {', '.join(sorted(unknown))}")
    requested_scenarios = tuple(
        name.strip() for name in args.scenarios.split(",") if name.strip()
    )
    unknown = set(requested_scenarios) - set(SCENARIOS)
    if unknown:
        parser.error(f"unknown scenarios: {', '.join(sorted(unknown))}")

    subject = dict(SUBJECTS[args.subject])
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    mbx = args.mbx.resolve() if args.mbx else None
    plan = []
    for name in requested_scenarios:
        tools = tuple(
            tool for tool in requested_tools if tool in SCENARIOS[name]["tools"]  # type: ignore[operator]
        )
        if tools:
            plan.append((name, tools))
    progress = Progress(
        sum(
            len(tools) * (args.trials if SCENARIOS[name].get("repeatable") else 1)
            for name, tools in plan
        )
    )
    if plan:
        progress.preparing()

    # Checkouts, targets, and stores are large and disposable; only the reports
    # under --output survive the run.
    # ignore_cleanup_errors: the results are computed after this block, so a
    # failure to remove the scratch tree would throw away the whole run. kache
    # runs a daemon per cell that writes into its own runtime directory here,
    # and one still draining while rmtree walks makes the removal fail. The
    # tree is disposable; the measurements are not.
    with tempfile.TemporaryDirectory(
        prefix="mbx-real-world-", ignore_cleanup_errors=True
    ) as temporary:
        work = Path(temporary)
        cargo_home = work / "cargo-home"
        cargo_home.mkdir()

        mirror = work / "mirror.git"
        git("clone", "--bare", str(subject["url"]), mirror)
        subject["mirror"] = mirror

        # Fetch the registry once, outside every timed cell, so no build is
        # timed while it downloads crates.
        seed = work / "fetch"
        clone(subject, str(subject["child"]), seed)
        subprocess.run(
            ["cargo", "fetch", "--locked"],
            cwd=seed,
            check=True,
            env={
                **os.environ,
                "CARGO_HOME": str(cargo_home),
                "RUSTUP_TOOLCHAIN": str(subject["toolchain"]),
            },
        )

        runner = Runner(output, cargo_home, mbx)
        scenarios = []
        for name, tools in plan:
            scenarios.append(
                run_scenario(name, tools, subject, runner, work, progress, args.trials)
            )

    failures = validate(scenarios)
    result: dict[str, object] = {
        # 2 added per-trial timings and named the published timing a median.
        "schema": 2,
        "subject": args.subject,
        "revision": subject["child"],
        "toolchain": subject["toolchain"],
        "platform": platform.platform(),
        "runner": os.environ.get("RUNNER_NAME") or os.environ.get("HOSTNAME") or "local",
        "workflow_run": os.environ.get("GITHUB_RUN_ID"),
        "commit": os.environ.get("GITHUB_SHA"),
        "versions": {
            # Bare version, no leading program name: CI compares this against
            # `cargo metadata` to decide whether the published numbers are
            # older than the mbx on main.
            "mbx": (
                subprocess.check_output([str(mbx), "--version"], text=True).strip().split()[-1]
                if mbx
                else None
            ),
            "cargo": tool_version("cargo", str(subject["toolchain"])),
            "rustc": tool_version("rustc", str(subject["toolchain"])),
            "kache": tool_version("kache"),
        },
        "passed": not failures,
        "failures": failures,
        "scenarios": scenarios,
    }

    (output / "real-world.json").write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    markdown = summarize(result)
    (output / "real-world.md").write_text(markdown, encoding="utf-8")
    print(markdown)

    if args.write_results:
        args.write_results.parent.mkdir(parents=True, exist_ok=True)
        args.write_results.write_text(
            json.dumps(publishable(result), indent=2) + "\n", encoding="utf-8"
        )

    for failure in failures:
        print(f"benchmark failed: {failure}", file=sys.stderr)
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
