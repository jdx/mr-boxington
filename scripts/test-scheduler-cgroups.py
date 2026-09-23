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


def watchdogs(worker):
    # Only signal directly observed children of this test's worker.
    return [int(pid) for pid in Path(f"/proc/{worker.pid}/task/{worker.pid}/children").read_text().split()]


def running(pid):
    try:
        return Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[0] != "Z"
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
        successor = None
        try:
            wait_for(lambda: load(state / "current.json"))
            generation = load(state / "current.json")["generation"]
            assert f"controller-{generation}" in Path(f"/proc/{worker.pid}/cgroup").read_text()
            registry = state / generation
            actions = group / generation
            launch = None
            if failure == "registration":
                launch = (state / "launch.lock").open("w")
                fcntl.flock(launch, fcntl.LOCK_EX)
                time.sleep(5.5)
                assert worker.poll() is None, "idle exit raced a registration"
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
            if launch:
                launch.close()
            target = children[-1][1]
            (target / "cgroup.freeze").write_text("1")
            wait_for(lambda: frozen(target))
            if failure == "exit":
                worker.kill()
                worker.wait()
            elif failure == "hang":
                worker.send_signal(signal.SIGSTOP)
            elif failure in ("cancel", "registration"):
                leases[-1].close()
            elif failure == "watchdog":
                [hung] = watchdogs(worker)
                os.kill(hung, signal.SIGSTOP)
            elif failure == "replaced":
                [orphaned] = watchdogs(worker)
                worker.kill()
                worker.wait()
            else:
                raise AssertionError(failure)
            wait_for(lambda: not frozen(target))
            if failure == "hang":
                assert (registry / "disabled").exists()
                worker.send_signal(signal.SIGCONT)
                # Disabled supervision still cleans up if an owner disappears.
                leases[-1].close()
                wait_for(lambda: not target.exists())
            if failure in ("cancel", "registration"):
                wait_for(lambda: not target.exists())
            if failure == "watchdog":
                wait_for(lambda: (registry / "disabled").exists())
                # A replacement watchdog cleans up even if the supervisor stalls.
                wait_for(lambda: watchdogs(worker) not in ([], [hung]))
                worker.send_signal(signal.SIGSTOP)
                leases[-1].close()
                wait_for(lambda: not target.exists())
                worker.send_signal(signal.SIGCONT)
                for lease in leases:
                    lease.close()
                wait_for(lambda: all(not action.exists() for _, action in children))
                worker.wait(timeout=10)
            if failure == "replaced":
                successor = subprocess.Popen([str(mbx), "__mbx-control", "supervisor", str(state), str(group)])
                wait_for(lambda: (load(state / "current.json") or {}).get("generation") != generation)
                # Keep the successor from idling out so it holds the election
                # lock; the old watchdog must exit once its orphans are gone.
                launch = (state / "launch.lock").open("w")
                fcntl.flock(launch, fcntl.LOCK_EX)
                for lease in leases:
                    lease.close()
                wait_for(lambda: not running(orphaned))
                assert successor.poll() is None
                launch.close()
                successor.wait(timeout=10)
                # The successor's idle shutdown removes the drained generation.
                assert not registry.exists() and not actions.exists()
                assert not (group / f"controller-{generation}").exists()
            print(f"PASS supervisor {failure}: compiler tree thawed", flush=True)
        finally:
            if worker.poll() is None:
                worker.send_signal(signal.SIGCONT)
                worker.terminate()
                worker.wait(timeout=8)
            if successor and successor.poll() is None:
                successor.terminate()
                successor.wait(timeout=8)
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


def handoff(mbx, root):
    with tempfile.TemporaryDirectory() as temporary:
        state = Path(temporary)
        group = root / ("test-" + uuid.uuid4().hex)
        group.mkdir()
        workers = [subprocess.Popen([str(mbx), "__mbx-control", "supervisor", str(state), str(group)])]
        try:
            wait_for(lambda: load(state / "current.json"))
            generation = load(state / "current.json")["generation"]
            # Launch a successor as soon as the idle supervisor stops accepting
            # registrations, while it still holds the election lock.
            deadline = time.monotonic() + 10
            while not (state / generation / "disabled").exists():
                assert time.monotonic() < deadline, "no idle shutdown"
                time.sleep(0.005)
            assert workers[0].poll() is None, "missed the handoff window"
            workers.append(subprocess.Popen([str(mbx), "__mbx-control", "supervisor", str(state), str(group)]))
            wait_for(lambda: (load(state / "current.json") or {}).get("generation") != generation)
            assert workers[1].poll() is None
            print("PASS supervisor handoff: successor elected during idle shutdown", flush=True)
        finally:
            for worker in workers:
                if worker.poll() is None:
                    worker.terminate()
                    worker.wait(timeout=8)
            time.sleep(0.5)
            for path in sorted(group.rglob("*"), reverse=True):
                if path.is_dir():
                    path.rmdir()
            group.rmdir()


def suspension(mbx, root):
    import threading
    with tempfile.TemporaryDirectory() as temporary:
        pool = Path(temporary)
        state = pool / "supervision-test"
        state.mkdir()
        group = root / ("test-" + uuid.uuid4().hex)
        group.mkdir()
        worker = subprocess.Popen([str(mbx), "__mbx-control", "supervisor", str(state), str(group)])
        children, leases = [], []
        stop = threading.Event()
        unhealthy = threading.Event()
        unhealthy.set()
        def readings():
            while not stop.is_set():
                with (pool / "pool.lock").open("w") as lock:
                    fcntl.flock(lock, fcntl.LOCK_EX)
                    now = int(time.time() * 1000)
                    value = {"version": 1, "sampled_ms": now,
                        "reading": {"total": 100, "available": 0 if unhealthy.is_set() else 50, "stalls": {}},
                        "bad_samples": 2, "healthy_since": None, "pressured": unhealthy.is_set(),
                        "valid": True, "recovered_ms": None, "last_admission_ms": None}
                    scratch = pool / "pressure.tmp"
                    scratch.write_text(json.dumps(value))
                    scratch.replace(pool / "pressure.json")
                stop.wait(0.05)
        sampler = threading.Thread(target=readings)
        sampler.start()
        try:
            wait_for(lambda: load(state / "current.json"))
            generation = load(state / "current.json")["generation"]
            assert f"controller-{generation}" in Path(f"/proc/{worker.pid}/cgroup").read_text()
            registry, actions = state / generation, group / generation
            for index in range(3):
                identity = uuid.uuid4().hex[:24]
                lease = (registry / (identity + ".lease")).open("w")
                fcntl.flock(lease, fcntl.LOCK_EX)
                leases.append(lease)
                action = actions / identity
                action.mkdir()
                child = subprocess.Popen(["sh", "-c", 'echo $$ > "$1/cgroup.procs"; exec sleep 120', "sh", str(action)])
                children.append((child, action))
                wait_for(lambda: (action / "cgroup.procs").read_text().strip())
                (registry / (identity + ".json")).write_text(str(int(time.time() * 1000) + index))
            wait_for(lambda: frozen(children[2][1]))
            assert not frozen(children[0][1])
            wait_for(lambda: frozen(children[1][1]))
            assert not frozen(children[0][1])
            assert any((pool / "suspended").iterdir())
            unhealthy.clear()
            wait_for(lambda: not frozen(children[1][1]))
            wait_for(lambda: not frozen(children[2][1]))
            events = [json.loads(line) for line in (registry / "events.jsonl").read_text().splitlines()]
            assert [event["event"] for event in events] == ["suspend", "suspend", "resume", "resume"], events
            assert events[1]["time_ms"] - events[0]["time_ms"] >= 1000
            assert events[3]["time_ms"] - events[2]["time_ms"] >= 1000
            print("PASS pressure trace: newest-first freeze and oldest-first resume", flush=True)
            # Stale registrar: even a live supervisor must fail open if it cannot sample.
            unhealthy.set()
            wait_for(lambda: frozen(children[2][1]))
            stop.set()
            sampler.join()
            with (pool / "pool.lock").open("w") as lock:
                fcntl.flock(lock, fcntl.LOCK_EX)
                wait_for(lambda: (registry / "disabled").exists())
                wait_for(lambda: not any(frozen(action) for _, action in children))
            for lease in leases:
                lease.close()
            wait_for(lambda: all(not action.exists() for _, action in children))
            worker.wait(timeout=8)
            print("PASS stale registrar: owned groups thawed and suspension disabled", flush=True)
        finally:
            stop.set()
            sampler.join()
            if worker.poll() is None:
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
    parser.add_argument("--suspension", action="store_true")
    args = parser.parse_args()
    for failure in ("exit", "hang", "cancel", "registration", "watchdog", "replaced"):
        lifecycle(args.mbx.resolve(), args.root.resolve(), failure)
    handoff(args.mbx.resolve(), args.root.resolve())
    if args.suspension:
        suspension(args.mbx.resolve(), args.root.resolve())
