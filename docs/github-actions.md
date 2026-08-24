# GitHub Actions

The simplest GitHub-hosted setup stores mbx's local cache in GitHub Actions
cache. A cache written by `main` can warm pull requests, including pull requests
from forks, without giving external contributors access to a private cache host.

## Build the cache on main

```yaml
name: rust-cache

on:
  push:
    branches: [main]
  workflow_dispatch:

permissions:
  contents: read

jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
        with:
          persist-credentials: false
      - uses: actions/cache@v6
        with:
          path: |
            ~/.cargo/registry
            ~/.cargo/git
            ~/.cargo/.global-cache
            ~/.cache/mbx
          key: ${{ runner.os }}-${{ runner.arch }}-mbx-${{ github.sha }}
          restore-keys: |
            ${{ runner.os }}-${{ runner.arch }}-mbx-
      - uses: jdx/mise-action@v4
        with:
          cache: false
          install_args: github:jdx/mr-boxington
      - run: mbx build build --workspace --all-features
      - run: mbx gc --max-size 3GB
        if: always()
```

Each `main` build restores the preceding cache, adds the actions needed by the
new commit, trims it to a repository-friendly budget, and saves an immutable
entry for its SHA.

## Restore it in pull requests

Use the restore-only action so pull requests never create a cache entry:

```yaml
- uses: actions/cache/restore@v6
  with:
    path: |
      ~/.cargo/registry
      ~/.cargo/git
      ~/.cargo/.global-cache
      ~/.cache/mbx
    key: ${{ runner.os }}-${{ runner.arch }}-mbx-${{ github.event.pull_request.base.sha }}
    restore-keys: |
      ${{ runner.os }}-${{ runner.arch }}-mbx-
- uses: jdx/mise-action@v4
  with:
    cache: false
    install_args: github:jdx/mr-boxington
- run: mbx build test --workspace
```

The operating system, architecture, and an explicit cache generation belong in
the key. Change the prefix when an mbx upgrade or cache-format change should
start fresh.

::: tip Pin actions in production
The examples use major tags for readability. Pin third-party actions to full
commit SHAs in a real workflow.
:::

## Self-hosted remote cache

For trusted runners and teams, mbx can talk to a compatible remote server such
as [`jdx/mbx-cache`](https://github.com/jdx/mbx-cache). Configure the URL,
namespace, and OIDC audience:

```yaml
permissions:
  contents: read
  id-token: write

env:
  MBX_REMOTE_URL: https://cache.example.com
  MBX_REMOTE_NAMESPACE: acme/backend
  MBX_REMOTE_OIDC_AUDIENCE: mbx-cache
```

Only a push to a protected branch may write. Pull requests degrade to read-only,
and tag or release builds do not use the remote cache at all. If fork authors
must not reach the host, use GitHub Actions cache for those jobs instead.
