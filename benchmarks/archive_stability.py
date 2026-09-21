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

  build path  member payloads differ only in the absolute path they were
              built at, which objects carry in their debug information. Copies
              made in different checkouts differ by design, and mbx already
              keys such a build script to its path rather than sharing it.
              Nothing is unstable, so `ZERO_AR_DATE` is beside the point.

  content     member payloads differ for some other reason. A different
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
import re
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


# An absolute path as an object file carries it: the build directory recorded
# in debug information. Matched generously because the point is only to tell
# "these differ by where they were built" from "these differ in substance".
BUILD_PATH = re.compile(rb"/[A-Za-z0-9._+-]+(?:/[A-Za-z0-9._+-]+){2,}")

# The archive's own symbol index, not a compilation output. `ranlib` rebuilds
# it from the members, so it carries their offsets: change the length of a
# path inside one object and this file differs too, for no reason of its own.
# Judging substance by it would report every path difference as content.
INDEX_MEMBERS = ("__.SYMDEF", "/", "//", "/SYM64/")


def is_index(name: str) -> bool:
    return name in INDEX_MEMBERS or name.startswith("__.SYMDEF")


def members(path: Path) -> dict[str, tuple[str, str]] | None:
    """Digest every archive member payload, raw and with paths normalized.

    The second digest answers a question the first cannot: two objects built
    from the same source at different paths are not the same bytes, but they
    are the same compilation, and calling that "unstable" sends whoever reads
    this report looking for a bug that is not there.
    """
    with tempfile.TemporaryDirectory() as scratch:
        extracted = subprocess.run(
            ["ar", "x", str(path.resolve())],
            cwd=scratch,
            capture_output=True,
        )
        if extracted.returncode != 0:
            return None
        out: dict[str, tuple[str, str]] = {}
        for name in sorted(os.listdir(scratch)):
            member = Path(scratch, name)
            if member.is_file():
                raw = member.read_bytes()
                out[name] = (
                    hashlib.sha256(raw).hexdigest(),
                    hashlib.sha256(BUILD_PATH.sub(b"<path>", raw)).hexdigest(),
                )
        return out


def embedded_paths(path: Path) -> set[bytes]:
    """Absolute paths an archive's members carry, as debug information."""
    with tempfile.TemporaryDirectory() as scratch:
        extracted = subprocess.run(
            ["ar", "x", str(path.resolve())], cwd=scratch, capture_output=True
        )
        if extracted.returncode != 0:
            return set()
        found: set[bytes] = set()
        for name in os.listdir(scratch):
            member = Path(scratch, name)
            if member.is_file() and not is_index(name):
                found |= set(BUILD_PATH.findall(member.read_bytes()))
        return found


def build_roots(paths: list[Path]) -> list[str]:
    """One representative build directory per copy, where they disagree.

    An object records the directory it was compiled in. Two copies built in
    different checkouts therefore differ in bytes without anything being
    unstable, and the only way to say so is to show the directories.
    """
    per_copy = [embedded_paths(path) for path in paths]
    shared = set.intersection(*per_copy) if per_copy else set()
    roots = []
    for found in per_copy:
        unique = sorted(found - shared, key=len)
        roots.append(unique[0].decode("utf-8", "replace") if unique else "")
    return roots


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
    # Every variant, not just the first two: a group can hold several copies
    # that differ only by timestamp and one that genuinely differs, and calling
    # the whole group "timestamp" would point at ZERO_AR_DATE for a problem it
    # cannot fix. "timestamp" has to mean every copy agrees on every payload.
    baseline = members(representatives[0])
    differing: set[str] | None = set()
    cause = "timestamp"
    if baseline is None:
        cause, differing = "unknown", None
    else:
        for representative in representatives[1:]:
            other = members(representative)
            if other is None:
                cause, differing = "unknown", None
                break
            if set(other) != set(baseline):
                cause, differing = "content", None
                break
            changed = {name for name in baseline if baseline[name][0] != other[name][0]}
            if not changed:
                continue
            differing |= changed
            # Substance only where normalizing the build path still leaves a
            # difference. "content" has to outrank "build path" once any member
            # differs for a reason a path cannot explain.
            substantive = {
                name
                for name in changed
                if not is_index(name) and baseline[name][1] != other[name][1]
            }
            cause = "content" if substantive or cause == "content" else "build path"
    if differing is not None:
        differing = sorted(differing)
    roots = build_roots(representatives) if cause in ("content", "build path") else []
    distinct_roots = sorted({root for root in roots if root})
    # Objects carrying different build directories explain a byte difference
    # without anything being unstable, so say so rather than leaving "content"
    # to be read as a defect.
    if cause == "content" and len(distinct_roots) > 1:
        cause = "build path"
    return {
        "stable": False,
        "copies": len(paths),
        "distinct_digests": len(by_digest),
        "sizes": sizes,
        "cause": cause,
        "build_roots": distinct_roots,
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
        elif result["cause"] == "build path":
            print(
                f"    {result['members_differing']} members differ, and the copies "
                "record different build directories -> compiled at different paths, "
                "not unstable; ZERO_AR_DATE is beside the point"
            )
            for root in result["build_roots"][:4]:
                print(f"      built at: {root}")
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
