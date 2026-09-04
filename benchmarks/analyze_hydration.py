#!/usr/bin/env python3
import hashlib
import fcntl
import json
import os
import shutil
import sys
import time
from collections import defaultdict
from pathlib import Path


def digest(path: Path) -> bytes:
    value = hashlib.sha256()
    with path.open("rb") as handle:
        while chunk := handle.read(1024 * 1024):
            value.update(chunk)
    return value.digest()


root = Path(sys.argv[2] if sys.argv[1] in ("snapshot", "hydrate") else sys.argv[1])
target = root / "target"
cas = root / "store/actions/cas/v1"
mbx = Path(sys.argv[3] if sys.argv[1] in ("snapshot", "hydrate") else sys.argv[2])


def clone(source: Path, destination: Path, ensure_parent: bool = True) -> None:
    if ensure_parent:
        destination.parent.mkdir(parents=True, exist_ok=True)
    with source.open("rb") as read, destination.open("wb") as write:
        try:
            fcntl.ioctl(write.fileno(), 0x40049409, read.fileno())
        except OSError:
            read.seek(0)
            shutil.copyfileobj(read, write, 1024 * 1024)


if sys.argv[1] == "hydrate":
    snapshot = root / "snapshot"
    manifest = json.loads((snapshot / "manifest.json").read_text())
    started = time.perf_counter()
    shutil.rmtree(target, ignore_errors=True)
    inline = snapshot / "inline"
    if inline.exists():
        os.replace(inline, target)
    else:
        target.mkdir()
    for entry in manifest["directories"]:
        (target / entry["path"]).mkdir(parents=True, exist_ok=True)

    directories_finished = time.perf_counter()
    regular = [
        entry
        for entry in manifest["files"]
        if "link" not in entry
        and entry["source"] != "inline"
        and not str(entry["source"]).startswith(str(inline) + os.sep)
    ]
    links = [entry for entry in manifest["files"] if "link" in entry]

    def restore_regular(entry: dict) -> None:
        destination = target / entry["path"]
        source = mbx if entry["source"] == "shim" else Path(entry["source"])
        clone(source, destination, ensure_parent=False)
        os.chmod(destination, entry["mode"])
        os.utime(destination, ns=(entry["mtime_ns"], entry["mtime_ns"]))

    for entry in regular:
        restore_regular(entry)

    regular_finished = time.perf_counter()
    for entry in links:
        # The first path for an inode is always a regular entry and already has
        # the inode's mode and timestamps. Reapplying them through every hard
        # link only adds metadata round trips.
        os.link(target / entry["link"], target / entry["path"])

    links_finished = time.perf_counter()
    for entry in reversed(manifest["directories"]):
        path = target / entry["path"]
        os.chmod(path, entry["mode"])
        os.utime(path, ns=(entry["mtime_ns"], entry["mtime_ns"]))
    finished = time.perf_counter()
    print(json.dumps({
        "hydrate_seconds": finished - started,
        "directory_create_seconds": directories_finished - started,
        "regular_files_seconds": regular_finished - directories_finished,
        "hardlinks_seconds": links_finished - regular_finished,
        "directory_metadata_seconds": finished - links_finished,
        "files": len(manifest["files"]),
        "hardlinks": len(links),
    }))
    raise SystemExit

cas_hashes = {digest(path) for path in cas.rglob("*") if path.is_file()}
mbx_hash = digest(mbx)

if sys.argv[1] == "snapshot":
    snapshot = root / "snapshot"
    shutil.rmtree(snapshot, ignore_errors=True)
    inline = snapshot / "inline"
    inline.mkdir(parents=True)
    directories = []
    for path in [target, *[path for path in target.rglob("*") if path.is_dir()]]:
        stat = path.stat()
        entry = {
            "path": str(path.relative_to(target)),
            "mode": stat.st_mode & 0o7777,
            "mtime_ns": stat.st_mtime_ns,
        }
        directories.append(entry)
        (inline / entry["path"]).mkdir(parents=True, exist_ok=True)
    cas_sources = {digest(path): path for path in cas.rglob("*") if path.is_file()}
    files = []
    inodes = {}
    for path in target.rglob("*"):
        if not path.is_file():
            continue
        stat = path.stat()
        relative = str(path.relative_to(target))
        entry = {"path": relative, "mode": stat.st_mode & 0o7777, "mtime_ns": stat.st_mtime_ns}
        inode = (stat.st_dev, stat.st_ino)
        if inode in inodes:
            entry["link"] = inodes[inode]
        else:
            inodes[inode] = relative
            hashed = digest(path)
            if hashed in cas_sources:
                entry["source"] = str(cas_sources[hashed])
            elif hashed == mbx_hash:
                entry["source"] = "shim"
            else:
                destination = inline / relative
                clone(path, destination)
                os.chmod(destination, entry["mode"])
                os.utime(destination, ns=(entry["mtime_ns"], entry["mtime_ns"]))
                entry["source"] = "inline"
        files.append(entry)
    for entry in reversed(directories):
        path = inline / entry["path"]
        os.chmod(path, entry["mode"])
        os.utime(path, ns=(entry["mtime_ns"], entry["mtime_ns"]))
    (snapshot / "manifest.json").write_text(json.dumps({"files": files, "directories": directories}))
    print(json.dumps({"files": len(files), "directories": len(directories)}))
    raise SystemExit
totals = defaultdict(lambda: [0, 0, 0])
missing = defaultdict(lambda: [0, 0, 0])

for path in target.rglob("*"):
    if not path.is_file():
        continue
    stat = path.stat()
    logical = stat.st_size
    allocated = stat.st_blocks * 512
    hashed = digest(path)
    relative = path.relative_to(target)
    parts = relative.parts
    if hashed in cas_hashes:
        category = "already in CAS"
    elif hashed == mbx_hash:
        category = "mbx build-script shim"
    elif ".fingerprint" in parts:
        category = "Cargo .fingerprint"
    elif len(parts) >= 3 and parts[1] == "build":
        category = "Cargo/build-script state"
    else:
        category = "other Cargo state"
    row = totals[category]
    row[0] += 1
    row[1] += logical
    row[2] += allocated
    if category not in ("already in CAS", "mbx build-script shim"):
        key = "/".join(parts[:3]) if len(parts) >= 3 else str(relative)
        row = missing[key]
        row[0] += 1
        row[1] += logical
        row[2] += allocated

for category, (count, logical, allocated) in sorted(totals.items()):
    print(f"{category}\t{count}\t{logical}\t{allocated}")
print("largest uncached groups")
for category, (count, logical, allocated) in sorted(
    missing.items(), key=lambda item: item[1][2], reverse=True
)[:30]:
    print(f"{category}\t{count}\t{logical}\t{allocated}")
