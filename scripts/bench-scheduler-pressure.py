#!/usr/bin/env python3
"""Compare cold concurrent Rust builds under an externally constrained cgroup.

Run in a disposable Linux container. --scope contains this benchmark and the
--root delegated cgroup; set memory.high/max before launching this script.
No limits are changed here. Results are sampled, not kernel peak-RSS claims.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time


def full_stalls(scope):
    for line in (scope / "memory.pressure").read_text().splitlines():
        if line.startswith("full "):
            return int(next(field[6:] for field in line.split() if field.startswith("total=")))
    return 0


def run(args, work, mode):
    cache = work / (mode + "-cache")
    env = os.environ.copy()
    env.update(MBX_CACHE_DIR=str(cache), MBX_GC_AUTO="0",
               MBX_SCHEDULER_CPUS="4", MBX_SCHEDULER_PRESSURE="0" if mode == "baseline" else "1",
               MBX_SCHEDULER_SUSPEND="1" if mode == "suspension" else "0",
               MBX_SCHEDULER_CGROUP_ROOT=str(args.root), CARGO_INCREMENTAL="0")
    env.pop("MBX_DISABLE", None)
    env.pop("MBX_REMOTE_URL", None)
    env.pop("MBX_REMOTE_MODE", None)
    children, logs, targets = [], [], []
    start_stalls = full_stalls(args.scope)
    started = time.monotonic()
    for index in range(4):
        project = work / (mode + str(index))
        (project / "src").mkdir(parents=True)
        (project / "Cargo.toml").write_text(f'[package]\nname="pressure{index}"\nversion="0.1.0"\nedition="2024"\n')
        source = "\n".join(f"#[inline(never)] fn f{i}(x:u64)->u64 {{ x.wrapping_add({i}) }}" for i in range(args.functions))
        source += '\nfn main(){ let mut total = 0u64;\n'
        source += "\n".join(f"total = total.wrapping_add(f{i}(std::hint::black_box(1)));" for i in range(args.functions))
        source += '\nprintln!("{total}");}\n'
        (project / "src/main.rs").write_text(source)
        log = (work / (mode + str(index) + ".log")).open("w")
        logs.append(log)
        target = project / "target"
        targets.append(target / "debug" / f"pressure{index}")
        children.append(subprocess.Popen([str(args.mbx), "build", "--offline", "--manifest-path", str(project / "Cargo.toml"), "--target-dir", str(target)], env=env, stdout=log, stderr=log))
    peak = 0
    while any(child.poll() is None for child in children):
        peak = max(peak, int((args.scope / "memory.current").read_text()))
        if time.monotonic() - started > 240:
            for child in children:
                child.terminate()
            raise RuntimeError("benchmark timed out; inspect compiler logs")
        time.sleep(0.05)
    codes = [child.wait() for child in children]
    elapsed = time.monotonic() - started
    for log in logs:
        log.close()
    expected = str(args.functions * (args.functions + 1) // 2)
    correct = all(code == 0 for code in codes)
    if correct:
        correct = all(subprocess.check_output([str(target)], text=True).strip() == expected for target in targets)
    stats = [json.loads(path.read_text()) for path in cache.glob("scheduler/supervision-*/*/*.stats")]
    result = dict(mode=mode, seconds=round(elapsed, 3), sampled_peak_bytes=peak,
                  full_stall_us=full_stalls(args.scope)-start_stalls, exit_codes=codes,
                  outputs_correct=correct, suspended_ms=sum(s["suspended_ms"] for s in stats),
                  freeze_count=sum(s["freeze_count"] for s in stats), supervised_actions=len(stats))
    print(json.dumps(result), flush=True)
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mbx", type=Path, required=True)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--scope", type=Path, required=True)
    parser.add_argument("--functions", type=int, default=6000)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--mode", choices=("baseline", "admission", "suspension"))
    args = parser.parse_args()
    args.mbx = args.mbx.resolve()
    args.work.mkdir(parents=True, exist_ok=True)
    results = [run(args, args.work, mode) for mode in ([args.mode] if args.mode else ("baseline", "admission", "suspension"))]
    (args.work / "results.json").write_text(json.dumps(results, indent=2) + "\n")
