# Working in this repository

## Never bump a crate version in a pull request

Versions belong to release-plz alone. It computes each crate's next version
from the commits that touched it and writes the bump in the release PR, so a
version edited by hand in a feature PR either collides with that calculation or
is silently overwritten by it. See [RELEASING.md](RELEASING.md) for the flow.

This also holds for breaking API changes: describe an intended break in the
commit and pull request, then let release-plz choose the version in the release
pull request.

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
