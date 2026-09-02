#!/usr/bin/env python3
"""Measure repeated source edits in a pinned mise checkout.

This is deliberately separate from real_world.py. That benchmark recreates
targets to model CI cache restores; this one preserves the target and mbx's
learned rustc incremental state to model a developer's edit/build loop.
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import shutil
import statistics
import subprocess
import tempfile
import time
from pathlib import Path


MISE_URL = "https://github.com/jdx/mise.git"
MISE_COMMIT = "8fe6385de7f73908ab5a6c9789f477a322eda3a5"
RUST_TOOLCHAIN = "1.98.0"
FIXTURE = Path("src/duration.rs")
BEFORE = "pub(crate) const HOURLY: Duration = Duration::from_secs(60 * 60);"
EDIT_PREFIX = "pub(crate) const HOURLY: Duration = Duration::from_secs(60 * 60 + "


def percentile(samples: list[int], fraction: float) -> int:
    """Return a nearest-rank percentile without interpolating measurements."""
    if not samples:
        raise ValueError("cannot summarize zero samples")
    ordered = sorted(samples)
    rank = max(1, int(len(ordered) * fraction + 0.999999999))
    return ordered[rank - 1]


def set_edit(checkout: Path, value: int) -> None:
    path = checkout / FIXTURE
    source = path.read_text(encoding="utf-8")
    candidates = [
        line
        for line in source.splitlines()
        if line == BEFORE or line.startswith(EDIT_PREFIX)
    ]
    if len(candidates) != 1:
        raise RuntimeError(f"edit fixture changed upstream: {path}")
    old = candidates[0]
    new = f"{EDIT_PREFIX}{value});"
    path.write_text(source.replace(old, new), encoding="utf-8")


def compiler_duration(stats: dict[str, object]) -> int:
    compiler = stats.get("compiler", {})
    if not isinstance(compiler, dict):
        raise RuntimeError("invalid mbx statistics: compiler is not an object")
    total = 0
    for outcome in compiler.values():
        if isinstance(outcome, dict):
            duration = outcome.get("duration_ns", 0)
            if isinstance(duration, int):
                total += duration
    return total


def clone_subject(destination: Path, local_checkout: Path | None) -> None:
    source = str(local_checkout) if local_checkout else MISE_URL
    command = ["git", "clone", "--quiet"]
    if local_checkout:
        command.append("--shared")
    command.extend([source, str(destination)])
    subprocess.run(command, check=True)
    subprocess.run(
        ["git", "checkout", "--quiet", "--detach", MISE_COMMIT],
        cwd=destination,
        check=True,
    )


def environment(tool: str, work: Path, report: Path) -> dict[str, str]:
    env = os.environ.copy()
    env.pop("RUSTC_WRAPPER", None)
    env.pop("RUSTC_WORKSPACE_WRAPPER", None)
    env.update(
        {
            "CARGO_TARGET_DIR": str(work / "target"),
            "RUSTUP_TOOLCHAIN": RUST_TOOLCHAIN,
            # Raw Cargo gets its normal incremental path. mbx starts from Cargo's
            # non-incremental shape so the measurement specifically exercises
            # mbx's per-checkout learned incremental policy.
            "CARGO_INCREMENTAL": "1" if tool == "cargo" else "0",
        }
    )
    if tool == "mbx":
        env["MBX_CACHE_DIR"] = str(work / "mbx-cache")
        env["MBX_STATS_REPORT"] = str(report)
    return env


def run_build(
    tool: str,
    executable: Path,
    checkout: Path,
    work: Path,
    output: Path,
    phase: str,
) -> dict[str, object]:
    report = output / f"{tool}-{phase}-stats.json"
    env = environment(tool, work, report)
    started = time.perf_counter_ns()
    completed = subprocess.run(
        [str(executable), "build", "--locked"],
        cwd=checkout,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    wall_duration_ns = time.perf_counter_ns() - started
    (output / f"{tool}-{phase}.stdout.log").write_text(
        completed.stdout, encoding="utf-8"
    )
    (output / f"{tool}-{phase}.stderr.log").write_text(
        completed.stderr, encoding="utf-8"
    )
    if completed.returncode != 0:
        raise RuntimeError(f"{tool} {phase} failed; see benchmark logs")

    result: dict[str, object] = {"wall_duration_ns": wall_duration_ns}
    if tool == "mbx":
        stats = json.loads(report.read_text(encoding="utf-8"))
        result["session_duration_ns"] = stats["session_duration_ns"]
        result["compiler_duration_ns"] = compiler_duration(stats)
        result["compiler"] = stats["compiler"]
        result["hits"] = stats["hits"]
        result["misses"] = stats["misses"]
    return result


def summarize(samples: list[dict[str, object]], key: str) -> dict[str, int]:
    values = [sample[key] for sample in samples if isinstance(sample.get(key), int)]
    assert all(isinstance(value, int) for value in values)
    integer_values = [int(value) for value in values]
    return {
        "min_ns": min(integer_values),
        "p50_ns": int(statistics.median(integer_values)),
        "p95_ns": percentile(integer_values, 0.95),
        "max_ns": max(integer_values),
    }


def measure_tool(
    tool: str,
    executable: Path,
    source_checkout: Path | None,
    root: Path,
    output: Path,
    iterations: int,
) -> dict[str, object]:
    checkout = root / f"mise-{tool}"
    work = root / f"work-{tool}"
    work.mkdir()
    clone_subject(checkout, source_checkout)

    print(f"{tool}: seeding target", flush=True)
    run_build(tool, executable, checkout, work, output, "seed")
    set_edit(checkout, 1)
    print(f"{tool}: warming first edited build", flush=True)
    run_build(tool, executable, checkout, work, output, "warmup")

    samples = []
    for index in range(1, iterations + 1):
        # Never repeat source content: an mbx artifact hit would skip rustc and
        # measure undo/redo caching rather than an ordinary new edit.
        set_edit(checkout, index + 1)
        phase = f"edit-{index:02d}"
        sample = run_build(tool, executable, checkout, work, output, phase)
        if tool == "mbx":
            compiler = sample.get("compiler")
            incremental = compiler.get("incremental") if isinstance(compiler, dict) else None
            if not isinstance(incremental, dict) or incremental.get("invocations") != 1:
                raise RuntimeError(
                    f"{phase} did not run exactly one incremental mise compilation"
                )
        samples.append(sample)
        print(f"{tool}: {phase} {sample['wall_duration_ns'] / 1e9:.3f}s", flush=True)

    summaries = {"wall": summarize(samples, "wall_duration_ns")}
    if tool == "mbx":
        summaries["session"] = summarize(samples, "session_duration_ns")
        summaries["compiler"] = summarize(samples, "compiler_duration_ns")
    return {"tool": tool, "summary": summaries, "samples": samples}


def markdown(result: dict[str, object]) -> str:
    lines = [
        "# mise edit-loop benchmark",
        "",
        f"Pinned mise commit: `{MISE_COMMIT}`",
        f"Rust toolchain: `{RUST_TOOLCHAIN}`",
        "",
        "| tool | wall p50 | wall p95 | compiler p50 | compiler p95 |",
        "| --- | ---: | ---: | ---: | ---: |",
    ]
    for entry in result["results"]:  # type: ignore[index]
        summary = entry["summary"]
        wall = summary["wall"]
        compiler = summary.get("compiler")
        compiler_p50 = f"{compiler['p50_ns'] / 1e9:.3f}s" if compiler else "—"
        compiler_p95 = f"{compiler['p95_ns'] / 1e9:.3f}s" if compiler else "—"
        lines.append(
            f"| {entry['tool']} | {wall['p50_ns'] / 1e9:.3f}s | "
            f"{wall['p95_ns'] / 1e9:.3f}s | {compiler_p50} | {compiler_p95} |"
        )
    return "\n".join(lines) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--mbx", type=Path, required=True)
    parser.add_argument("--checkout", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--iterations", type=int, default=30)
    parser.add_argument("--tools", default="cargo,mbx")
    args = parser.parse_args()
    if args.iterations < 1:
        parser.error("--iterations must be positive")

    tools = [tool.strip() for tool in args.tools.split(",") if tool.strip()]
    unknown = set(tools) - {"cargo", "mbx"}
    if unknown:
        parser.error(f"unknown tools: {', '.join(sorted(unknown))}")

    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    executables = {
        "cargo": Path(shutil.which("cargo") or "cargo"),
        "mbx": args.mbx.resolve(),
    }
    source_checkout = args.checkout.resolve() if args.checkout else None
    if source_checkout and not source_checkout.exists():
        parser.error(f"checkout does not exist: {source_checkout}")

    # Keep mise's multi-gigabyte targets on the caller-selected filesystem.
    with tempfile.TemporaryDirectory(
        prefix="edit-loop-", dir=output.parent
    ) as temporary:
        root = Path(temporary)
        results = [
            measure_tool(
                tool,
                executables[tool],
                source_checkout,
                root,
                output,
                args.iterations,
            )
            for tool in tools
        ]

    result: dict[str, object] = {
        "version": 1,
        "subject": {"name": "mise", "commit": MISE_COMMIT},
        "platform": platform.platform(),
        "rustc": subprocess.check_output(
            ["rustup", "run", RUST_TOOLCHAIN, "rustc", "-V"], text=True
        ).strip(),
        "iterations": args.iterations,
        "results": results,
    }
    (output / "edit-loop.json").write_text(
        json.dumps(result, indent=2) + "\n", encoding="utf-8"
    )
    (output / "edit-loop.md").write_text(markdown(result), encoding="utf-8")
    print(markdown(result), end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
