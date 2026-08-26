# GitHub Actions

[`jdx/mr-boxington-action`](https://github.com/jdx/mr-boxington-action)
installs mbx and connects it to either GitHub Actions cache or an mbx-compatible
server.

## GitHub Actions cache

The default backend restores mbx's local store on every run and saves an entry
only after a successful push to the repository's default branch. Pull requests,
including pull requests from forks, are restore-only.

```yaml
permissions:
  contents: read

steps:
  - uses: actions/checkout@v7
  - uses: jdx/mr-boxington-action@v1
  - run: mbx test --workspace
```

Before saving, the action prunes the store to 3 GB. Set `max-size` to change the
budget, or change `cache-generation` when an upgrade or policy change should
start fresh:

```yaml
- uses: jdx/mr-boxington-action@v1
  with:
    version: 0.3.0
    cache-generation: v2
    max-size: 5GB
```

The operating system and architecture are included in generated keys. Advanced
workflows can provide complete `cache-key` and `restore-keys` inputs.

## Manual GitHub cache setup

The equivalent pieces can be assembled directly when Cargo download caches or
custom save policies need to share the same entry:

```yaml
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
    install_args: mr-boxington
- run: mbx test --workspace
- run: mbx gc --max-size 3GB
  if: always()
```

Use `actions/cache/restore` instead of `actions/cache` in pull requests so they
cannot create entries.

::: tip Pin actions in production
The examples use major tags for readability. Pin third-party actions to full
commit SHAs in a real workflow.
:::

## Cache server

For trusted runners and teams, mbx can talk to a compatible remote server such
as [`jdx/mbx-cache`](https://github.com/jdx/mbx-cache). The action exports the
remote configuration for subsequent steps:

```yaml
permissions:
  contents: read
  id-token: write

steps:
  - uses: actions/checkout@v7
  - uses: jdx/mr-boxington-action@v1
    with:
      backend: server
      server-url: https://cache.example.com
      namespace: acme/backend
      oidc-audience: mbx-cache
  - run: mbx build --workspace --all-features
```

Only a push to a protected branch may write. Pull requests degrade to read-only,
and tag or release builds do not use the remote cache at all. If fork authors
must not reach the host, use the GitHub backend for those jobs instead. A bearer
token can be supplied with the action's `token` input when OIDC is unavailable.

For a repository that combines both backends — the server for trusted runs, the
GitHub cache for fork pull requests — see
[CI with fork pull requests](/cookbook/fork-prs).
