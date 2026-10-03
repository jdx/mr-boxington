---
description: Route trusted CI builds to a private cache server and fork pull requests to GitHub Actions cache.
---
# CI with fork pull requests

Open-source repositories take pull requests from forks, and GitHub withholds
secrets and OIDC tokens from fork-triggered runs. A fork's job therefore cannot
[authenticate](/remote-cache#authenticate) to a cache server. mbx's own
[read and write policy](/remote-cache#read-and-write-policy) already keeps every
pull request read-only, so the remaining question is which backend each run
should use.

If every job can use [GitHub Actions cache](/github-action#github-actions-cache),
the GitHub Action's default setup already supports fork pull requests. Use this
recipe when trusted runs need a private cache server: it sends trusted runs to
the server and fork pull requests to the `github` backend.

mbx writes to the remote only from pushes to a protected branch, so protect
`main` before expecting remote writes. Configure read and write grants on the
server as well; client-side policy does not authorize access. See
[Authorization](/cache-server#authorization).

```yaml
name: ci

on:
  push:
    branches: [main]
  pull_request:

permissions:
  contents: read
  id-token: write # GitHub withholds OIDC from fork runs on its own

env:
  # a fork PR cannot mint an OIDC token, so it uses the github backend
  MBX_BACKEND: >-
    ${{ github.event_name == 'pull_request'
        && github.event.pull_request.head.repo.full_name != github.repository
        && 'github' || 'remote' }}

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
      # optional: a push to main also saves a pruned Cargo target directory and
      # registry to GitHub Actions cache, so fork PRs restore warm without ever
      # reaching the server
      - name: Mirror the target directory for fork pull requests
        if: github.event_name == 'push'
        uses: jdx/mr-boxington-action@v1
        with:
          backend: github
      - uses: jdx/mr-boxington-action@v1
        with:
          backend: ${{ env.MBX_BACKEND }}
          remote-url: https://cache.example.com
          namespace: acme/backend
          oidc-audience: https://cache.example.com
      - run: mbx test --workspace
```

## How it works

- The backend expression treats pushes and same-repository pull requests as
  trusted, because both come from people with push access. It treats
  everything else as a fork. Fork runs get the `github` backend, which restores
  from [GitHub Actions cache](/github-action#github-actions-cache) without
  credentials and never saves from a fork pull request.
- `id-token: write` is safe to declare at the workflow level, because GitHub
  refuses to issue OIDC tokens to fork-triggered runs regardless of the
  declared permission. For the same reason, the fork path must not pick the
  `remote` backend: it would fail asking for a token it can never have.
- The mirror step keeps fork pull requests warm. Their runs can restore only
  from GitHub Actions cache, and nothing would populate it if every trusted
  build used the server alone. On pushes to `main`, the mirror runs the
  `github` backend alongside the server and saves a pruned Cargo target
  directory and registry after the build. Fork pull requests restore that
  entry. Action steps clean up in reverse order, so the mirror's save runs
  last. Without a `save-on-*` input, the `github` backend saves only after a
  push to the default branch, so the `if:` event guard is all the step needs.
- To make the target directory transportable, the mirror step sets
  `MBX_TARGET_VIEWS=0` for the rest of the job. Trusted pushes therefore build
  in the workspace's own `target` directory instead of a
  [managed target directory](/managed-targets).
- Keep both action steps on the default `target` payload
  (`github-cache-mode: target`). The `objects` payload is saved under a
  different key, so if the mirror step saved it, fork runs would not restore it.

## Separate trust levels {#hardening}

The single-workflow recipe trusts GitHub to withhold fork credentials. To make
the trust boundary structural, split the workflow in two. A router workflow
runs either a `trusted` or an `untrusted` job, never both, and each job calls a
shared `workflow_call` implementation with its own permissions and inputs.

[tak's `ci.yml`](https://github.com/jdx/tak/blob/main/.github/workflows/ci.yml)
is a living example of the router and its `final` job, and tak's
[`ci-impl.yml`](https://github.com/jdx/tak/blob/main/.github/workflows/ci-impl.yml)
picks a runner pool for each route. Because tak does not use mbx's remote
cache, both of its routes declare the same permissions; add `id-token: write`
and the `remote` backend to your trusted route yourself.

This structure lets you enforce the following boundaries:

- The untrusted route never declares `id-token: write`, and its first step can
  assert that `ACTIONS_ID_TOKEN_REQUEST_URL` is absent.
- Trust can be narrower than “same repository”: tak trusts only one account,
  so with a cache server behind the trusted route, a compromised collaborator
  token still could not reach it.
- Each trust level can use its own runner pool, keeping fork code off
  self-hosted runners.
- A `final` job asserts that exactly one route ran and succeeded, giving branch
  protection a single required check.

::: warning Pin actions to commit SHAs
Whichever shape you use, pin third-party actions to full commit SHAs in a real
workflow. A mutable tag such as `@v1` can be retargeted at any time.
:::
