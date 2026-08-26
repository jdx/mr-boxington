# Stability

What an mbx upgrade can and cannot break.

## The store is disposable

The local store is a cache of work mbx can redo. Correctness never depends on
its format: an entry a new version cannot read is a miss, and the compilation
runs again. The worst case for any upgrade is a colder build, not a wrong one,
and deleting the cache directory is always safe.

Managed target directories live under a versioned root (`targets/v1/…`), and
the `target` symlink in each checkout keeps working across upgrades.

## JSON output is versioned

`mbx doctor --json`, `mbx cache stats --json`, `mbx gc --json`, and
`MBX_STATS_REPORT` emit versioned documents; fields are added compatibly and a
shape change bumps the version. Scripts should read the version field and
parse JSON rather than the human-readable `mbx[...]` stderr lines, which are
written for people and may be reworded at any time.

## Configuration

Unknown TOML keys and invalid values are errors, so an upgrade that renames or
retires a setting fails loudly instead of silently ignoring what you wrote.

## Wire protocols and crates

The shim/agent protocol is internal — both halves ship in one binary and
require exact version equality, so there is nothing to keep compatible across
machines. The remote cache protocol is versioned under `/v1/` with explicit
evolution rules, and published Rust crates follow semantic versioning enforced
by `cargo-semver-checks` in CI. See
[protocol compatibility](/protocol-compatibility) for the details.
