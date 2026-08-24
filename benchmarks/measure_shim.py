#!/usr/bin/env python3
"""Measure the warm mbx rustc-shim overhead against a trivial compiler."""

from __future__ import annotations

import argparse
import json
import os
import platform
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path


def percentile(values: list[int], percent: float) -> int:
    ordered = sorted(values)
    return ordered[min(round((len(ordered) - 1) * percent), len(ordered) - 1)]


def elapsed_ns(command: list[str]) -> int:
    started = time.perf_counter_ns()
    subprocess.run(command, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    return time.perf_counter_ns() - started


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--mbx", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--samples", type=int, default=400)
    parser.add_argument("--warmups", type=int, default=30)
    parser.add_argument("--budget-ms", type=float, default=2.0)
    args = parser.parse_args()

    if args.samples < 20 or args.warmups < 1 or args.budget_ms <= 0:
        parser.error("samples >= 20, warmups >= 1, and budget-ms > 0 are required")
    baseline = shutil.which("true")
    if baseline is None:
        raise SystemExit("the benchmark requires a POSIX `true` executable")

    args.output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="mbx-shim-benchmark-") as temporary:
        shim = Path(temporary) / "mbx-rustc"
        shutil.copy2(args.mbx, shim)
        direct = [baseline, "-vV"]
        wrapped = [str(shim), baseline, "-vV"]

        for index in range(args.warmups):
            elapsed_ns(direct if index % 2 == 0 else wrapped)
            elapsed_ns(wrapped if index % 2 == 0 else direct)

        direct_samples: list[int] = []
        wrapped_samples: list[int] = []
        paired_overhead: list[int] = []
        for index in range(args.samples):
            if index % 2 == 0:
                direct_ns = elapsed_ns(direct)
                wrapped_ns = elapsed_ns(wrapped)
            else:
                wrapped_ns = elapsed_ns(wrapped)
                direct_ns = elapsed_ns(direct)
            direct_samples.append(direct_ns)
            wrapped_samples.append(wrapped_ns)
            paired_overhead.append(wrapped_ns - direct_ns)

    median_overhead_ns = int(statistics.median(paired_overhead))
    budget_ns = int(args.budget_ms * 1_000_000)
    passed = median_overhead_ns <= budget_ns
    result = {
        "version": 1,
        "platform": platform.platform(),
        "python": platform.python_version(),
        "samples": args.samples,
        "warmups": args.warmups,
        "budget_ns": budget_ns,
        "passed": passed,
        "direct": {
            "median_ns": int(statistics.median(direct_samples)),
            "p95_ns": percentile(direct_samples, 0.95),
        },
        "wrapped": {
            "median_ns": int(statistics.median(wrapped_samples)),
            "p95_ns": percentile(wrapped_samples, 0.95),
        },
        "overhead": {
            "median_ns": median_overhead_ns,
            "p95_ns": percentile(paired_overhead, 0.95),
        },
        "raw": {
            "direct_ns": direct_samples,
            "wrapped_ns": wrapped_samples,
            "paired_overhead_ns": paired_overhead,
        },
    }
    (args.output / "shim-overhead.json").write_text(
        json.dumps(result, indent=2) + "\n", encoding="utf-8"
    )
    markdown = (
        "## Warm rustc-shim overhead\n\n"
        "| Samples | Direct median | Wrapped median | Median overhead | p95 overhead | Budget | Result |\n"
        "| ---: | ---: | ---: | ---: | ---: | ---: | :--- |\n"
        f"| {args.samples} | {result['direct']['median_ns'] / 1e6:.3f} ms "
        f"| {result['wrapped']['median_ns'] / 1e6:.3f} ms "
        f"| {median_overhead_ns / 1e6:.3f} ms "
        f"| {result['overhead']['p95_ns'] / 1e6:.3f} ms "
        f"| {args.budget_ms:.3f} ms | {'pass' if passed else 'fail'} |\n\n"
        "The hard gate uses the median of paired, alternating direct/wrapped invocations. "
        "The p95 is retained for diagnosis but is not gated because shared CI runners can have isolated scheduling spikes.\n"
    )
    (args.output / "shim-overhead.md").write_text(markdown, encoding="utf-8")
    print(markdown)
    if not passed:
        print(
            f"warm shim overhead {median_overhead_ns / 1e6:.3f} ms exceeds "
            f"the {args.budget_ms:.3f} ms budget",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
