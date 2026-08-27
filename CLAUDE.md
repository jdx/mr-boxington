# Working in this repository

## Never bump a crate version in a pull request

Versions belong to release-plz alone. It computes each crate's next version
from the commits that touched it and writes the bump in the release PR, so a
version edited by hand in a feature PR either collides with that calculation or
is silently overwritten by it. See [RELEASING.md](RELEASING.md) for the flow.

This holds even when `cargo semver-checks` fails in CI. That check compares the
branch against its base and reports what the change would require; it is
telling you the shape of the change, not asking for a bump. A failure means one
of two things:

- The break is unintended. Fix the API instead. Adding a field to a struct with
  all-public fields breaks exhaustive literals, so add a method or a
  constructor rather than a field. Additive methods, new types, and new enum
  variants on `#[non_exhaustive]` enums are all free.
- The break is intended and worth it. Say so in the pull request and leave the
  version alone. Whoever merges decides how it releases; release-plz will pick
  the right number from the commits.

Chasing the check with a bump is also self-defeating: `main` moves, the bump
collides on the next rebase, and it has to be raised again.

## Documentation is generated

`docs/cli/` is generated from the `usage` declarations in
`crates/mbx/src/config.rs` and `crates/mbx/src/cli.rs`. Run `mise run
render:docs` after changing a setting and commit the result; `mise run
check:docs` fails otherwise. Never hand-edit a file under `docs/cli/`, and
resolve a rebase conflict there by regenerating rather than by merging.

## Before pushing

`mise run ci` is the gate: lint, build, generated-docs check, and both test
suites. `mise run format` fixes what lint complains about.

The bats suites need the git submodules under `test/`, and the wasm end-to-end
test needs `wasm32-unknown-unknown` installed for the toolchain `mise` pins —
`rustup target add --toolchain <pinned> wasm32-unknown-unknown`, not just for
the rustup default.
