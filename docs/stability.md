---
description: Understand compatibility guarantees for upgrades, CLI JSON output, local storage, and Rust APIs.
---
# Stability

The CLI, machine-readable reports, local cache, and embedding APIs have
different compatibility rules. Use this page when planning an upgrade or
building tooling around mbx.

## The store is disposable

The local store is a cache of work mbx can redo. Correctness never depends on
its format: an entry a new version cannot read is a miss, and the compilation
runs again. The worst case for any upgrade is a colder build, not a wrong one,
and cached compilations can be regenerated. Stop builds before manually
deleting the whole root: it also contains managed targets and private state.
Prefer [`mbx gc`](/managed-targets#inspect-and-clean-up) for routine cleanup.

Managed target directories live under a versioned root (`targets/v1/…`), and
the `target` symlink in each checkout keeps working across upgrades.

The store is also not fsynced. Cached objects and action results are published
by atomic rename, so a reader sees a whole file or none, but a crash or power
cut before the bytes reach disk can leave a file whose content does not match
its name. Every object is checked against its content hash the first time a
build reads it, and one that fails is a miss: the compilation runs again and
its result is republished over the bad bytes. Records that only shortcut work,
such as the input list a finished compilation leaves for the next build of the
same invocation, are written the same way and ignored when they fail to parse.
`mbx cache verify` reads the whole store back and reports any object left in
that state.

## JSON output is versioned

`mbx doctor --json`, `mbx stats --json`, `mbx cache stats --json`, `mbx gc --json`, and
`MBX_STATS_REPORT` emit versioned documents; fields are added compatibly and a
shape change bumps the version. Scripts should read the version field and
parse the JSON. The `mbx[...]` stderr lines are written for people and may be
reworded at any time.

### Session event streams are not a public API {#session-event-streams-are-not}

The per-compilation streams under `sessions/v1/` that back [`mbx tui`](/tui) are
an implementation detail of that command. Their records carry a version field,
but the format may change in any release. Use mbx commands to read them; scripts
should consume `MBX_STATS_REPORT` for build statistics.

## Configuration

Unknown TOML keys and invalid values are errors, so an upgrade that renames or
retires a setting fails instead of ignoring what you wrote.

## Wire protocols and crates

The local shim/agent protocol requires exact protocol and application-version
equality. Embedders can connect to it, but must upgrade their client alongside
mbx when that exact-match version changes. The remote cache protocol is
versioned under `/v1/` with explicit evolution rules. Published Rust subcrates
remain on `0.x`, so their APIs may change in a minor release. See
[protocol compatibility](/protocol-compatibility) for the details.

`mbx-cache-cargo`, `mbx-cache-store`, `mbx-cache-core`, `mbx-cache-rustc`, and
`mbx-cache-cc` are supported building blocks for coordinated embedding.
They stay on `0.x`: their public APIs can change in a new minor
release, and embedders should pin a compatible minor. Store and wire formats
remain independently versioned and treat unfamiliar records as cache misses.
