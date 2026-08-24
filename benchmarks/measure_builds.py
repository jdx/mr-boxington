#!/usr/bin/env python3
"""Measure cold/cache-warm builds and optionally run the verify canary."""

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


def version(command: str) -> str:
    return subprocess.check_output([command, "-V"], text=True).strip()


def run_phase(
    phase: str,
    mbx: Path,
    workspace: Path,
    output: Path,
    target: Path,
    store: Path,
    verify: bool,
) -> dict[str, object]:
    # The target has a fixed name so compiler paths stay byte-for-byte stable.
    # It is underneath our output directory and is recreated for every phase.
    if target.parent != store.parent:
        raise RuntimeError("refusing to remove a target outside the benchmark work directory")
    shutil.rmtree(target, ignore_errors=True)
    report = output / f"{phase}-stats.json"
    stdout = output / f"{phase}.stdout.log"
    stderr = output / f"{phase}.stderr.log"
    environment = os.environ.copy()
    environment.update(
        {
            "MBX_CACHE_DIR": str(store),
            "MBX_STATS_REPORT": str(report),
            "MBX_VERIFY": "1" if verify else "0",
            "CARGO_TARGET_DIR": str(target),
            "CARGO_INCREMENTAL": "0",
        }
    )
    started = time.perf_counter_ns()
    completed = subprocess.run(
        [str(mbx), "check", "--workspace", "--all-targets", "--locked"],
        cwd=workspace,
        env=environment,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    duration_ns = time.perf_counter_ns() - started
    stdout.write_text(completed.stdout, encoding="utf-8")
    stderr.write_text(completed.stderr, encoding="utf-8")
    if completed.returncode != 0:
        raise RuntimeError(f"{phase} build failed; see {stderr}")
    stats = json.loads(report.read_text(encoding="utf-8"))
    return {"phase": phase, "wall_duration_ns": duration_ns, "stats": stats}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--mbx", type=Path, required=True)
    parser.add_argument("--workspace", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--verify", action="store_true")
    args = parser.parse_args()

    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    workspace = args.workspace.resolve()
    # Keep the large Cargo target and cache trees out of the retained artifact.
    # TemporaryDirectory also guarantees every invocation begins cold.
    with tempfile.TemporaryDirectory(prefix="mbx-build-measurement-") as temporary:
        work = Path(temporary)
        target = work / "build-target"
        store = work / "cache"
        phases = [
            run_phase("cold", args.mbx.resolve(), workspace, output, target, store, False),
            run_phase("warm", args.mbx.resolve(), workspace, output, target, store, False),
        ]
        if args.verify:
            phases.append(
                run_phase("verify", args.mbx.resolve(), workspace, output, target, store, True)
            )

    warm_stats = phases[1]["stats"]
    assert isinstance(warm_stats, dict)
    failures: list[str] = []
    if warm_stats["hits"] <= 0 or warm_stats["restored_output_files"] <= 0:
        failures.append("warm build did not restore any cached compiler outputs")
    if args.verify:
        verify_stats = phases[2]["stats"]
        assert isinstance(verify_stats, dict)
        if verify_stats["verifications"] <= 0:
            failures.append("verify build did not qualify any cache hits")
        if verify_stats["divergences"] != 0:
            failures.append(f"verify build found {verify_stats['divergences']} divergences")

    result = {
        "version": 1,
        "platform": platform.platform(),
        "rustc": version("rustc"),
        "cargo": version("cargo"),
        "commit": os.environ.get("GITHUB_SHA"),
        "passed": not failures,
        "failures": failures,
        "phases": phases,
    }
    (output / "build-measurements.json").write_text(
        json.dumps(result, indent=2) + "\n", encoding="utf-8"
    )

    rows = []
    for phase in phases:
        stats = phase["stats"]
        assert isinstance(stats, dict)
        rows.append(
            f"| {phase['phase']} | {phase['wall_duration_ns'] / 1e9:.2f} s "
            f"| {stats['hits']} | {stats['misses']} | {stats['unconsulted']} "
            f"| {stats['restored_output_files']} | {stats['restored_output_bytes']} "
            f"| {stats['verifications']} | {stats['divergences']} |"
        )
    markdown = (
        "## End-to-end workspace measurements\n\n"
        "`cargo check --workspace --all-targets --locked` runs against a fresh target in each phase.\n\n"
        "| Phase | Wall time | Hits | Misses | Unconsulted | Restored files | Restored bytes | Verified | Diverged |\n"
        "| :--- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |\n"
        + "\n".join(rows)
        + "\n"
    )
    (output / "build-measurements.md").write_text(markdown, encoding="utf-8")
    print(markdown)
    for failure in failures:
        print(f"measurement failed: {failure}", file=sys.stderr)
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
