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
4. While the binaries build, Communiqué rewrites the draft's raw changelog into
   release notes informed by the commits, pull requests, and diffs. If that
   optional enhancement fails, the original release-plz notes remain in place.
5. `publish-release` waits for both paths, then undrafts it. A release is
   therefore never visible without its assets, and no asset upload runs after
   immutable releases lock the tag and asset set.

`release_always = false`, so a push to `main` that merely carries a version
bump does not release — only merging a release PR does.

## Versions

The crates do not share a version. `mbx` and `mbx-cache-protocol` each have an
independent one, because each carries a stability promise the other should not
be able to break: the CLI's surface is its commands and JSON output, and the
protocol crate's is the remote cache wire contract that `jdx/mbx-cache` depends
on. `mbx-cache-core` and `mbx-cache-rustc` are internals; they share the
`mbx-internals` version group, move together, and stay on `0.x` so semver
itself says a minor bump may break them.

release-plz computes each line from the commits that touched it and updates the
path dependencies' version requirements, so a release can move one crate and
leave the rest alone. When `mbx` reaches `1.0`, the internal crates stay on
`0.x`: the CLI depending on a `0.x` library is normal, and the library target
inside `mbx` is `#[doc(hidden)]` so none of those types are part of the
promise. See [protocol compatibility](docs/protocol-compatibility.md) for the
promise each version line makes.

## Tags and asset names

The `mbx` crate is tagged `v{version}`; the three library crates are published to
crates.io but get no GitHub release of their own. Asset names carry no version
(`mbx-x86_64-unknown-linux-musl.tar.gz`), which is what keeps
`releases/latest/download/…` a stable URL.

## Setup this depends on

These settings live outside the repository and are required for secure,
successful releases:

- **Immutable releases enabled for the repository** — draft assets remain
  replaceable while the matrix is being assembled, then GitHub locks the tag
  and every asset when the draft is published. This also gives consumers a
  release attestation they can verify independently of `SHA256SUMS`.
- **`RELEASE_PLZ_TOKEN`** — a PAT with `contents: write` and
  `pull-requests: write`. The default `GITHUB_TOKEN` cannot be used: pushes made
  with it do not trigger workflows, so the release PR would never run CI.
- **`ANTHROPIC_API_KEY`** — optional, used by Communiqué to replace the draft's
  release-plz changelog with editorialized release notes. Without it (or when
  generation fails), publishing continues with the original notes. Communiqué
  is locked in `mise.lock`; update that pin deliberately with `mise lock`.
- **`CERTIFICATES_P12`** and **`CERTIFICATES_P12_PASS`** — the base64-encoded
  Developer ID Application certificate and its password used by the other
  jdx.dev CLI release workflows. The macOS jobs import the certificate and sign
  `mbx` as `Developer ID Application: Jeffrey Dickey (4993Y37DX6)` before
  creating the release archives.
- **`APPLE_API_KEY_P8`, `APPLE_API_KEY_ID`, and `APPLE_API_ISSUER_ID`** — an App
  Store Connect **team** API key with the Developer role, base64-encoded, plus
  its key and issuer identifiers. It has to be a team key rather than an
  individual one: `notarytool` takes an issuer only for team keys and rejects it
  for individual keys. The macOS job submits the signed binary to Apple's
  notary service with them. These three are the one optional entry in this list:
  without them the release still ships, the binary is still signed, and the
  build annotates a warning instead of failing. What it loses is Gatekeeper
  approval for anyone who downloads the archive through a browser — see
  [Notarization](#notarization).
- **A crates.io Trusted Publisher for each of `mbx`, `mbx-cache-core`,
  `mbx-cache-protocol`, and `mbx-cache-rustc`**, naming this repository and the
  workflow file `release-plz.yml`. Publishing is OIDC-only; no registry token
  exists anywhere. **Renaming `release-plz.yml` breaks publishing** until the
  trusted publishers are updated to match. A crate that does not exist on
  crates.io yet cannot be created this way — publish its first version by hand
  with a `publish-new` token, then configure its trusted publisher.

## Notarization

Signing proves who built the binary; notarization is what Gatekeeper asks for
when the archive carries the quarantine bit, which is to say when someone
downloaded it from the releases page in a browser rather than with `curl`. An
un-notarized binary is blocked there behind a "cannot be verified" dialog.
Getting past it takes a deliberate override — approving the binary under Privacy
& Security, or stripping `com.apple.quarantine` by hand — which is not something
to ask of someone installing a build tool.

The ticket lives on Apple's side and is never stapled into the archive.
`stapler` writes tickets into bundles, disk images, and installer packages, and
`mbx` is a bare Mach-O executable in a tarball — none of those. Gatekeeper
resolves the ticket online instead, which is the normal arrangement for a CLI
distributed this way. Two consequences worth knowing:

- The shipped archive is byte-identical to the signed binary. Nothing may
  re-sign, strip, or rewrite it after notarization, or the ticket no longer
  matches what a user runs.
- A machine with no network reaching Apple cannot confirm the ticket. That is
  the trade a non-bundle CLI makes; the alternative is shipping a `.pkg`.

The build's `spctl` output is informational. Tickets take a moment to propagate
after a submission is accepted, so a negative assessment in a release log is not
by itself evidence of a bad build — the accepted submission status is the gate,
and the job fails when it is anything else.

## When a release goes wrong

If the tag was created but the binaries failed to build, fix the problem and
run the `release` workflow manually with that tag while the release is still a
draft. It rebuilds, re-attaches with `--clobber`, and undrafts the release
itself — dispatching by hand is the one path where nothing else would. If the
tag exists but its release does not, because release-plz failed after tagging
or the draft was deleted, that run recreates it rather than failing. The
workflow refuses to replace assets after publication; release a new version if
a published artifact is wrong.
