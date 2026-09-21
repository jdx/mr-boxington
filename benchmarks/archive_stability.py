#!/usr/bin/env python3
"""Find native archives whose bytes move between otherwise identical builds.

A `.a` produced by a build script is an input to everything downstream of it.
When the archiver stamps a timestamp into the header, rebuilding an unchanged
dependency republishes the same content under a new digest, and every cached
action below it misses. This reads an existing mbx cache and reports where that
happened, separating the two causes that look identical from the outside:

  timestamp   every member payload is byte-identical and only the archive
              headers differ. `ZERO_AR_DATE` fixes this, and mbx sets it for
              build scripts (see the `ar_determinism` setting).

  content     member payloads themselves differ between builds. A different
              problem with a different cause; `ZERO_AR_DATE` does nothing for
              it, and it needs diagnosing on its own.

Archives are compared only against other copies of the same crate, profile and
build hash, so a difference is never just two different configurations.

Usage:
    python3 benchmarks/archive_stability.py [--targets DIR] [--json OUT]
"""

from __future__ import annotations

import argparse
import collections
import hashlib
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path


def default_targets() -> Path:
    """The managed target root, which defaults to <cache_dir>/targets."""
    if root := os.environ.get("MBX_TARGET_ROOT"):
        return Path(root)
    if cache := os.environ.get("MBX_CACHE_DIR"):
        return Path(cache) / "targets"
    if sys.platform == "darwin":
        return Path.home() / "Library/Caches/mbx/targets"
    base = os.environ.get("XDG_CACHE_HOME", str(Path.home() / ".cache"))
    return Path(base) / "mbx/targets"


def digest(path: Path) -> str:
    hasher = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            hasher.update(chunk)
    return hasher.hexdigest()


def members(path: Path) -> dict[str, str] | None:
    """Digest every archive member payload, or None if it cannot be read."""
    with tempfile.TemporaryDirectory() as scratch:
        extracted = subprocess.run(
            ["ar", "x", str(path.resolve())],
            cwd=scratch,
            capture_output=True,
        )
        if extracted.returncode != 0:
            return None
        out: dict[str, str] = {}
        for name in sorted(os.listdir(scratch)):
            member = Path(scratch, name)
            if member.is_file():
                out[name] = digest(member)
        return out


def identity(path: Path, root: Path) -> tuple[str, str] | None:
    """Group key: the profile and the path below `build/`.

    That suffix is `<crate>-<build hash>/out/.../name.a`, so two files sharing
    it were produced by the same crate at the same Cargo fingerprint. Copies
    under different target directories are then directly comparable.
    """
    parts = path.relative_to(root).parts
    if "build" not in parts:
        return None
    index = parts.index("build")
    if index == 0:
        return None
    return parts[index - 1], "/".join(parts[index + 1 :])


def classify(paths: list[Path]) -> dict:
    by_digest: dict[str, list[Path]] = collections.defaultdict(list)
    for path in paths:
        by_digest[digest(path)].append(path)
    if len(by_digest) < 2:
        return {"stable": True, "copies": len(paths)}

    representatives = [group[0] for group in by_digest.values()]
    sizes = sorted({path.stat().st_size for path in paths})
    first, second = members(representatives[0]), members(representatives[1])
    if first is None or second is None:
        cause, differing = "unknown", None
    elif set(first) != set(second):
        cause, differing = "content", None
    else:
        differing = sorted(name for name in first if first[name] != second[name])
        cause = "content" if differing else "timestamp"
    return {
        "stable": False,
        "copies": len(paths),
        "distinct_digests": len(by_digest),
        "sizes": sizes,
        "cause": cause,
        "members_differing": len(differing) if differing is not None else None,
        "example_members": (differing or [])[:5],
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--targets", type=Path, default=default_targets())
    parser.add_argument("--json", type=Path)
    arguments = parser.parse_args()

    root: Path = arguments.targets
    if not root.is_dir():
        print(f"no target directories to inspect at {root}", file=sys.stderr)
        return 2

    groups: dict[tuple[str, str], list[Path]] = collections.defaultdict(list)
    for path in root.rglob("*.a"):
        if path.is_file() and (key := identity(path, root)):
            groups[key].append(path)

    comparable = {key: paths for key, paths in groups.items() if len(paths) > 1}
    findings = {key: classify(paths) for key, paths in sorted(comparable.items())}
    unstable = {key: r for key, r in findings.items() if not r["stable"]}

    print(f"target root: {root}")
    print(f"archives comparable across target directories: {len(comparable)}")
    print(f"unstable: {len(unstable)}")
    for (profile, suffix), result in unstable.items():
        print(f"\n  {profile}/{suffix}")
        print(
            f"    {result['distinct_digests']} distinct digests from "
            f"{result['copies']} copies; cause: {result['cause']}"
        )
        if result["cause"] == "timestamp":
            print("    every member payload identical -> ZERO_AR_DATE addresses this")
        elif result["members_differing"]:
            print(
                f"    {result['members_differing']} member payloads differ "
                f"-> not a timestamp; e.g. {', '.join(result['example_members'])}"
            )
        if len(result["sizes"]) > 1:
            print(f"    sizes vary: {result['sizes']}")

    if arguments.json:
        arguments.json.write_text(
            json.dumps(
                {f"{profile}/{suffix}": r for (profile, suffix), r in findings.items()},
                indent=2,
            ),
            encoding="utf-8",
        )
        print(f"\nwrote {arguments.json}")

    # Reporting a survey is the whole job; an unstable archive is a finding to
    # act on, not a failure of this script.
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
