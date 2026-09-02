# CI with fork pull requests

Open-source repositories take pull requests from forks, and forks change what
a CI run may hold: GitHub withholds secrets and OIDC tokens from
fork-triggered runs, so a fork's job cannot authenticate to a
[cache server](/remote-cache). mbx's own [write policy](/remote-cache#read-and-write-policy)
already keeps every pull request read-only. The remaining question is which
backend each run should use.

The recipe: trusted runs talk to the cache server, and fork pull requests fall
back to GitHub Actions cache, which needs no credentials.

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
  # A fork PR cannot mint an OIDC token, so it uses the GitHub backend.
  MBX_BACKEND: >-
    ${{ github.event_name == 'pull_request'
        && github.event.pull_request.head.repo.full_name != github.repository
        && 'github' || 'server' }}

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
      # Optional: a push to main also saves the local store into GitHub
      # Actions cache, so fork PRs restore warm despite never reaching the
      # server. The github backend only saves from the default branch, so
      # the event guard is all this step needs.
      - name: Mirror the store for fork pull requests
        if: github.event_name == 'push'
        uses: jdx/mr-boxington-action@v1
        with:
          backend: github
      - uses: jdx/mr-boxington-action@v1
        with:
          backend: ${{ env.MBX_BACKEND }}
          server-url: https://cache.example.com
          namespace: acme/backend
          oidc-audience: mbx-cache
      - run: mbx test --workspace
```

## How it works

- The backend expression treats pushes and same-repository pull requests as
  trusted, since both come from people with push access, and everything else
  as a fork. Fork runs get the [GitHub Actions cache backend](/github-action),
  which restores without credentials and never lets a pull request save.
- `id-token: write` is safe to declare at the workflow level. GitHub refuses
  to issue OIDC tokens to fork-triggered runs regardless of the declared
  permission. That is also why the fork path must not pick the server
  backend: it would fail asking for a token it can never have.
- The mirror step keeps fork PRs warm. Their runs can restore only from GitHub
  Actions cache, and nothing would populate it if every trusted build used
  the server alone. Running the github backend alongside the server on `main`
  pushes saves the store after the build (action steps clean up in reverse
  order, so the mirror's save runs last), and fork PRs restore that entry.

## Hardening

The single-workflow recipe above trusts the platform to withhold fork
credentials. To make the trust boundary structural, split it in two: a router
workflow whose `trusted` and `untrusted` jobs are mutually exclusive, each
calling a shared `workflow_call` implementation with different permissions and
inputs.
[tak's ci.yml](https://github.com/jdx/tak/blob/main/.github/workflows/ci.yml)
is a living example. What the split buys:

- An untrusted run never declares `id-token: write` at all, and its first step
  can assert `ACTIONS_ID_TOKEN_REQUEST_URL` is absent.
- Trust can be narrower than "same repository": tak only trusts one account,
  so a compromised collaborator token still cannot reach the server.
- Each trust level can use its own runner pool, keeping fork code off
  self-hosted runners.
- A `final` job asserts that exactly one route ran and succeeded, giving
  branch protection a single required check.

Whichever shape you use, pin third-party actions to full commit SHAs in a real
workflow; a mutable tag can be retargeted at any time.
