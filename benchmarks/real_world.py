#!/usr/bin/env python3
"""Build a pinned real-world project through the situations CI actually hits.

`measure_builds.py` measures this workspace against itself, which answers
"did mbx regress" but not "what does mbx do to somebody else's build". This
driver clones a pinned third-party subject and runs the same Cargo invocation
under raw cargo, mbx, and kache across scenarios drawn from the live hk and
mise cache workflows: a cold store, a warm store with a fresh target, the
next commit on the branch, and a second checkout at a different path.

Every timing is wall clock around one build, the way CI experiences it. The
registry is fetched once up front and shared, so no cell is timed while it
downloads crates. Toolchains are pinned per subject: a runner-image Rust bump
changes every invocation digest at once, which reads as a cache that stopped
working. See `benchmarks/README.md`.
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
    },
}

TOOLS = ("cargo", "mbx", "kache")

# Which tools each scenario asks for. cargo appears only where a no-cache
# baseline is meaningful: "warm" and "worktree" describe a cache being reused,
# and cargo would just repeat its cold number.
SCENARIOS: dict[str, dict[str, object]] = {
    "cold": {
        "tools": ("cargo", "mbx", "kache"),
        "description": "empty store, fresh target -- a first build on a new machine",
    },
    "warm": {
        "tools": ("mbx", "kache"),
        "description": "warm store, fresh target -- CI restoring a cache for the same commit",
    },
    "commit": {
        "tools": ("cargo", "mbx", "kache"),
        "description": "store warmed at the parent commit, build the child -- push-to-push CI",
    },
    "worktree": {
        "tools": ("mbx", "kache"),
        "description": "warm store, second checkout at a different path",
    },
    "toolchain": {
        "tools": ("mbx",),
        "description": "store warmed on the pinned toolchain, rebuild on another",
        "timed": False,
    },
}


# What fraction of a manifest's predictions may still be looked up after the
# compiler changes. Anything above this means rustc work survived a change that
# invalidates every invocation digest.
TOOLCHAIN_SURVIVOR_LIMIT = 0.05


class Skipped(Exception):
    """A cell could not run for a reason that is not a benchmark failure."""


def tool_version(command: str) -> str | None:
    executable = shutil.which(command)
    if executable is None:
        return None
    for flag in ("--version", "-V"):
        try:
            return subprocess.check_output([executable, flag], text=True).strip().splitlines()[0]
        except (subprocess.CalledProcessError, OSError):
            continue
    return None


def resolved_rustc(toolchain: str) -> str:
    """What `rustc -V` reports under one rustup toolchain name."""
    return subprocess.check_output(
        ["rustc", "-V"],
        text=True,
        env={**os.environ, "RUSTUP_TOOLCHAIN": toolchain},
    ).strip()


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

    def base_environment(self, subject: dict[str, object], target: Path) -> dict[str, str]:
        environment = os.environ.copy()
        environment.update(
            {
                "CARGO_HOME": str(self.cargo_home),
                "CARGO_TARGET_DIR": str(target),
                "CARGO_INCREMENTAL": "0",
                "CARGO_TERM_COLOR": "never",
                "RUSTUP_TOOLCHAIN": str(subject["toolchain"]),
            }
        )
        # A wrapper inherited from the caller's shell would silently cache the
        # supposedly uncached baseline.
        for variable in ("RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"):
            environment.pop(variable, None)
        return environment

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
    ) -> dict[str, object]:
        """Run one build and return its timing plus whatever the tool reported."""
        shutil.rmtree(target, ignore_errors=True)
        environment = self.base_environment(subject, target)
        if toolchain is not None:
            environment["RUSTUP_TOOLCHAIN"] = toolchain
        args = list(subject["args"])  # type: ignore[arg-type]
        extra: dict[str, object] = {}

        if tool == "cargo":
            command = ["cargo", *args]
        elif tool == "mbx":
            if self.mbx is None:
                raise Skipped("no mbx binary was given (--mbx)")
            report = self.output / f"{cell}-stats.json"
            environment.update(
                {
                    "MBX_CACHE_DIR": str(store),
                    "MBX_STATS_REPORT": str(report),
                }
            )
            command = [str(self.mbx), *args]
        elif tool == "kache":
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
            command = ["cargo", *args]
        else:
            raise ValueError(f"unknown tool {tool}")

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

        if tool == "mbx":
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

        return {"tool": tool, "wall_duration_ns": duration_ns, **extra}


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


def run_scenario(
    scenario: str,
    tools: tuple[str, ...],
    subject: dict[str, object],
    runner: Runner,
    work: Path,
) -> dict[str, object]:
    """Run every tool through one scenario, each in its own store and target."""
    results: list[dict[str, object]] = []
    notes: list[str] = []
    for tool in tools:
        cell = f"{scenario}-{tool}"
        store = work / f"store-{cell}"
        target = work / f"target-{cell}"
        try:
            if scenario == "cold":
                checkout = work / f"checkout-{cell}"
                clone(subject, str(subject["child"]), checkout)
                results.append(
                    runner.run(
                        tool=tool,
                        cell=cell,
                        subject=subject,
                        checkout=checkout,
                        target=target,
                        store=store,
                    )
                )
            elif scenario == "warm":
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
                results.append(
                    runner.run(
                        tool=tool,
                        cell=cell,
                        subject=subject,
                        checkout=checkout,
                        target=target,
                        store=store,
                    )
                )
            elif scenario == "commit":
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
                    # cargo has no store to seed and its target is wiped before
                    # every run, so seeding it would only measure a cold build
                    # twice. The baseline for this scenario is a cold build.
                    clone(subject, str(subject["child"]), checkout)
                results.append(
                    runner.run(
                        tool=tool,
                        cell=cell,
                        subject=subject,
                        checkout=checkout,
                        target=target,
                        store=store,
                    )
                )
            elif scenario == "worktree":
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
                # A different path, which is the whole point: absolute paths
                # must not have entered the keys.
                other = work / f"checkout-{cell}-elsewhere"
                clone(subject, str(subject["child"]), other)
                results.append(
                    runner.run(
                        tool=tool,
                        cell=cell,
                        subject=subject,
                        checkout=other,
                        target=work / f"target-{cell}-elsewhere",
                        store=store,
                    )
                )
            elif scenario == "toolchain":
                alternate = os.environ.get("MBX_BENCH_ALTERNATE_TOOLCHAIN")
                if not alternate:
                    raise Skipped("set MBX_BENCH_ALTERNATE_TOOLCHAIN to a second installed Rust")
                if resolved_rustc(alternate) == resolved_rustc(str(subject["toolchain"])):
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
                results.append(
                    runner.run(
                        tool=tool,
                        cell=cell,
                        subject=subject,
                        checkout=checkout,
                        target=target,
                        store=store,
                        toolchain=alternate,
                    )
                )
            else:
                raise ValueError(f"unknown scenario {scenario}")
        except Skipped as skip:
            notes.append(f"{tool}: {skip}")

    return {
        "scenario": scenario,
        "description": SCENARIOS[scenario]["description"],
        "timed": SCENARIOS[scenario].get("timed", True),
        "results": results,
        "skipped": notes,
    }


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

    cold = by_name.get("cold")
    warm = by_name.get("warm")
    if cold and warm:
        cold_by_tool = {cell["tool"]: cell for cell in cold["results"]}  # type: ignore[index]
        for cell in warm["results"]:  # type: ignore[index]
            baseline = cold_by_tool.get(cell["tool"])
            if baseline and cell["wall_duration_ns"] >= baseline["wall_duration_ns"]:
                failures.append(
                    f"warm: {cell['tool']} was no faster than its own cold build"
                )

    toolchain = by_name.get("toolchain")
    if toolchain:
        for cell in toolchain["results"]:  # type: ignore[index]
            stats = cell.get("stats")
            if not isinstance(stats, dict):
                continue
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
            "| Tool | Wall time | Hits | Restored files |"
            if scenario["timed"]
            else "| Tool | Ran for | Hits | Restored files |",
            "| :--- | ---: | ---: | ---: |",
        ]
        for cell in scenario["results"]:
            stats = cell.get("stats")
            hit = str(hits(cell)) if isinstance(stats, dict) else "-"
            files = str(restored_files(cell)) if isinstance(stats, dict) else "-"
            lines.append(
                f"| {cell['tool']} | {cell['wall_duration_ns'] / 1e9:.1f} s | {hit} | {files} |"
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
    parser.add_argument("--scenarios", default="cold,warm,commit")
    parser.add_argument("--mbx", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument(
        "--write-results",
        type=Path,
        help="rewrite this checked-in results file with the run (the docs site reads it)",
    )
    args = parser.parse_args()

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

    # Checkouts, targets, and stores are large and disposable; only the reports
    # under --output survive the run.
    with tempfile.TemporaryDirectory(prefix="mbx-real-world-") as temporary:
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
        for name in requested_scenarios:
            tools = tuple(
                tool for tool in requested_tools if tool in SCENARIOS[name]["tools"]  # type: ignore[operator]
            )
            if not tools:
                continue
            print(f"running scenario {name} ({', '.join(tools)})", file=sys.stderr)
            scenarios.append(run_scenario(name, tools, subject, runner, work))

    failures = validate(scenarios)
    result: dict[str, object] = {
        "schema": 1,
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
            "cargo": tool_version("cargo"),
            "rustc": tool_version("rustc"),
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
