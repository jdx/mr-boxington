#!/usr/bin/env python3
"""Linux lifecycle tests; requires an explicitly delegated cgroup v2 root.

Run inside a disposable memory-limited container, never against the host root:
    python3 scripts/test-scheduler-cgroups.py --mbx target/debug/mbx --root /sys/fs/cgroup/mbx-tests
"""
import argparse
import fcntl
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import time
import uuid


def wait_for(predicate, timeout=8):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(0.05)
    raise AssertionError("condition did not become true")


def load(path):
    try:
        return json.loads(path.read_text())
    except (OSError, ValueError):
        return None


def frozen(path):
    try:
        return "frozen 1" in (path / "cgroup.events").read_text()
    except FileNotFoundError:
        return False


def lifecycle(mbx, root, failure):
    with tempfile.TemporaryDirectory() as temporary:
        state = Path(temporary)
        group = root / ("test-" + uuid.uuid4().hex)
        group.mkdir()
        worker = subprocess.Popen([str(mbx), "__mbx-control", "supervisor", str(state), str(group)])
        children = []
        leases = []
        try:
            wait_for(lambda: load(state / "current.json"))
            generation = load(state / "current.json")["generation"]
            registry = state / generation
            actions = group / generation
            for _ in range(2):
                identity = uuid.uuid4().hex[:24]
                lease = (registry / (identity + ".lease")).open("w")
                fcntl.flock(lease, fcntl.LOCK_EX)
                leases.append(lease)
                action = actions / identity
                action.mkdir()
                child = subprocess.Popen([
                    "python3", "-u", "-c",
                    "import pathlib,subprocess,time; "
                    f"pathlib.Path({str(action / 'cgroup.procs')!r}).write_text('0'); "
                    "subprocess.Popen(['sleep','120']); print('ready',flush=True); time.sleep(120)"
                ], stdout=subprocess.PIPE, text=True)
                children.append((child, action))
                assert child.stdout.readline().strip() == "ready"
                (registry / (identity + ".json")).write_text(str(int(time.time() * 1000)))
                wait_for(lambda: len((action / "cgroup.procs").read_text().splitlines()) == 2)
            target = children[-1][1]
            (target / "cgroup.freeze").write_text("1")
            wait_for(lambda: frozen(target))
            if failure == "exit":
                worker.kill()
                worker.wait()
            elif failure == "hang":
                worker.send_signal(signal.SIGSTOP)
            elif failure == "cancel":
                leases[-1].close()
            else:
                raise AssertionError(failure)
            wait_for(lambda: not frozen(target))
            if failure == "hang":
                assert (registry / "disabled").exists()
                worker.send_signal(signal.SIGCONT)
                worker.wait(timeout=8)
            if failure == "cancel":
                wait_for(lambda: not target.exists())
            print(f"PASS supervisor {failure}: compiler tree thawed", flush=True)
        finally:
            if worker.poll() is None:
                worker.send_signal(signal.SIGCONT)
                worker.terminate()
                worker.wait(timeout=8)
            for child, action in children:
                if action.exists():
                    (action / "cgroup.freeze").write_text("0")
                    (action / "cgroup.kill").write_text("1")
                child.wait(timeout=8)
            for lease in leases:
                lease.close()
            wait_for(lambda: all(not action.exists() or not (action / "cgroup.procs").read_text().strip() for _, action in children))
            for path in sorted(group.rglob("*"), reverse=True):
                if path.is_dir():
                    path.rmdir()
            group.rmdir()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mbx", type=Path, required=True)
    parser.add_argument("--root", type=Path, required=True)
    args = parser.parse_args()
    for failure in ("exit", "hang", "cancel"):
        lifecycle(args.mbx.resolve(), args.root.resolve(), failure)
