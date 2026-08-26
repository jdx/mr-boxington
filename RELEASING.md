# Releasing

Releases are driven by [release-plz](https://release-plz.dev). Nobody tags by
hand.

## The flow

1. Commits land on `main`. `release-plz-pr` opens (or updates) a release PR
   that bumps versions and writes `CHANGELOG.md`.
2. Merging that PR runs `release-plz-release`, which tags, publishes the crates
   to crates.io, and creates the GitHub release **as a draft**.
3. `release.yml` builds a binary per target and attaches the archives plus
   `SHA256SUMS` to that draft.
4. `publish-release` undrafts it. A release is therefore never visible without
   its assets.

`release_always = false`, so a push to `main` that merely carries a version
bump does not release — only merging a release PR does.

## Tags and asset names

The `mbx` crate is tagged `v{version}`; the three library crates are published to
crates.io but get no GitHub release of their own. Asset names carry no version
(`mbx-x86_64-unknown-linux-musl.tar.gz`), which is what keeps
`releases/latest/download/…` a stable URL.

## Setup this depends on

These settings live outside the repository, and releases fail without them:

- **`RELEASE_PLZ_TOKEN`** — a PAT with `contents: write` and
  `pull-requests: write`. The default `GITHUB_TOKEN` cannot be used: pushes made
  with it do not trigger workflows, so the release PR would never run CI.
- **`CERTIFICATES_P12`** and **`CERTIFICATES_P12_PASS`** — the base64-encoded
  Developer ID Application certificate and its password used by the other
  jdx.dev CLI release workflows. The macOS jobs import the certificate and sign
  `mbx` as `Developer ID Application: Jeffrey Dickey (4993Y37DX6)` before
  creating the release archives.
- **A crates.io Trusted Publisher for each of `mbx`, `mbx-cache-core`,
  `mbx-cache-protocol`, and `mbx-cache-rustc`**, naming this repository and the
  workflow file `release-plz.yml`. Publishing is OIDC-only; no registry token
  exists anywhere. **Renaming `release-plz.yml` breaks publishing** until the
  trusted publishers are updated to match. A crate that does not exist on
  crates.io yet cannot be created this way — publish its first version by hand
  with a `publish-new` token, then configure its trusted publisher.

## When a release goes wrong

If the tag was created but the binaries failed to build, fix the problem and
run the `release` workflow manually with that tag. It rebuilds, re-attaches
with `--clobber`, and undrafts the release itself — dispatching by hand is the
one path where nothing else would. If the tag exists but its release does not,
because release-plz failed after tagging or the draft was deleted, that run
recreates it rather than failing.
