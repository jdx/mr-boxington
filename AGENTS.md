# Repository Instructions

## Conventional Commits

Pull request titles must use
`<type>[optional scope][optional !]: <description>`; intermediate commit
subjects should use the same format. Start descriptions with a lowercase
character and keep them concise and imperative. Use `!` for a breaking change and explain it with a
`BREAKING CHANGE:` footer.

Allowed types are `bench`, `chore`, `ci`, `docs`, `feat`, `fix`, `perf`,
`refactor`, `revert`, `security`, `style`, and `test`.

CI validates the pull request title and re-runs when it is edited. Intermediate
commit subjects are not checked because pull requests are squash-merged. CI
mechanically checks the allowed type, syntax, and lowercase-leading description;
imperative mood and breaking-change details remain review rules.

## Versions

Never bump a crate version in an ordinary pull request. Release-plz computes
versions from the commits touching each crate and writes the bumps in its
release pull request. This includes breaking API changes: describe the break in
the commit and pull request, then let release-plz choose the version.

## Changelogs

- Do not edit `CHANGELOG.md` files in ordinary pull requests. The release-plz
  workflow generates changelog entries in release pull requests.
- Only edit a changelog when the user explicitly requests it or when working on
  the release process itself.

## Generated Documentation

`docs/cli/` is generated from the usage declarations in
`crates/mbx/src/config.rs` and `crates/mbx/src/cli.rs`. Run
`mise run render:docs` after changing a setting and commit the result. Do not
hand-edit generated CLI documentation.

## Before Pushing

`mise run ci` is the main gate. `mise run format` fixes formatting problems.
The Bats suites require the git submodules under `test/`; the wasm end-to-end
test requires `wasm32-unknown-unknown` for the repository's pinned toolchain.
