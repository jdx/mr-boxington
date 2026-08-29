# GitHub Action

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

## Parallel Cargo steps

Independent lint or test configurations do not have to queue behind each
other. GitHub Actions can start them together, while mbx gives every compiler
they launch one machine-wide CPU and memory budget:

```yaml
steps:
  - uses: actions/checkout@v7
  - uses: jdx/mr-boxington-action@v1
  - parallel:
      - name: Clippy with default features
        env:
          CARGO_TARGET_DIR: ${{ runner.temp }}/clippy-default
        run: mbx clippy --workspace -- -D warnings
      - name: Clippy with all features and targets
        env:
          CARGO_TARGET_DIR: ${{ runner.temp }}/clippy-all
        run: mbx clippy --workspace --all-features --all-targets -- -D warnings
```

Each command needs a separate `CARGO_TARGET_DIR`; otherwise Cargo's target
directory lock serializes the supposedly parallel steps. The mbx store and
scheduler remain shared. When both configurations reach the same cold
compilation, one does the work and the other restores its result; unrelated
compilations run within the shared permit pool instead of each Cargo process
trying to fill the runner independently.

In two cold, order-reversed A/B trials on an xlarge CI runner, parallel Clippy
finished **up to 44.9% sooner** than the same two commands run sequentially.
The permanent
[contention benchmark](/benchmarks#contention) reproduces that workload and
checks both wall time and peak compiler count. Tuning and failure behavior are
covered under
[machine-wide compile scheduling](/configuration#machine-wide-compile-scheduling).

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
as the self-hostable [cache server](/cache-server). The action exports the
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

Only a push to a protected branch may write. Pull requests and tag or release
builds degrade to read-only. If fork authors must not reach the host, use the
GitHub backend for those jobs instead. A bearer token can be supplied with the
action's `token` input when OIDC is unavailable.

## S3-compatible bucket

A bucket needs nothing running.
[`aws-actions/configure-aws-credentials`](https://github.com/aws-actions/configure-aws-credentials)
exchanges the runner's OIDC token for a role and exports the credentials mbx
reads, so no long-lived secret is stored:

```yaml
permissions:
  contents: read
  id-token: write

env:
  MBX_REMOTE_URL: s3://acme-build-cache
  MBX_REMOTE_NAMESPACE: acme/backend

steps:
  - uses: actions/checkout@v7
  - uses: aws-actions/configure-aws-credentials@v5
    with:
      role-to-assume: arn:aws:iam::111122223333:role/mbx-cache
      aws-region: us-west-2
  - uses: jdx/mise-action@v4
    with:
      cache: false
      install_args: mr-boxington
  - run: mbx build --workspace --all-features
```

mbx is installed rather than set up through
[`jdx/mr-boxington-action`](https://github.com/jdx/mr-boxington-action) here,
because the action's GitHub Actions cache backend would store the same actions
a second time. Use one or the other.

mbx still refuses to publish from a pull request. Because a bucket has no
server to authorize anything, make IAM agree: scope the role's trust policy to
the branches allowed to assume it, and give pull request jobs a role that can
only read. See [remote cache](/remote-cache#who-may-publish).

::: warning Do not use remote caches for production releases
A production release may still use `mbx` and its local cache, but should not use
a remote cache so a cache-poisoning attack cannot influence published artifacts.
Release jobs should also avoid restoring or saving the mbx store through
`actions/cache`.
:::

For a repository that combines both backends — the server for trusted runs, the
GitHub cache for fork pull requests — see
[CI with fork pull requests](/cookbook/fork-prs).
