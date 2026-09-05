# GitHub Action

[`jdx/mr-boxington-action`](https://github.com/jdx/mr-boxington-action)
installs mbx and connects it to either GitHub Actions cache or an mbx-compatible
server. The examples below show the inputs that matter for each backend; the
action's repository documents the complete list.

## GitHub Actions cache

The default backend restores Cargo's pruned target directory and its registry
from the previous compatible cache entry, the same shape of entry
[Swatinem/rust-cache](https://github.com/Swatinem/rust-cache) restores, so a
job that changes a few files recompiles only those crates. Paired measurements
on GitHub-hosted Linux and macOS runners put restore-plus-build within noise
of rust-cache, ahead on build and test jobs, with a cache entry of the same
size; in the one Windows warm benchmark so far, restore was slower. A new
immutable entry is saved after a push to the repository's default branch, and
after a trusted `workflow_dispatch` run when the action's
`save-on-workflow-dispatch` input is enabled. Pull requests, including pull
requests from forks, are restore-only.

```yaml
permissions:
  contents: read

steps:
  - uses: actions/checkout@v7
  - uses: jdx/mr-boxington-action@v1
  - run: mbx test --workspace
```

The earlier payload, mbx's own object store exported as the closure of every
`mbx` command the job completed, remains available as
`github-cache-mode: objects`. It suits builds that must share one entry across
differing target directories or checkout layouts; it omits the Cargo registry,
which Cargo then downloads again during the build, and measured about ten
seconds slower per job. The action assigns `MBX_CACHE_EXPORT_GROUP` for that
mode itself. Change `cache-generation` when a cache format or policy change
should start fresh:

```yaml
- uses: jdx/mr-boxington-action@v1
  with:
    version: 0.3.0
    cache-generation: v2
```

Generated keys cover the operating system, the architecture, and the identity
of the `rustc` on `PATH`, because a store built by one compiler matches nothing
under another. Install the toolchain before the action, and name it in the
action's `toolchain` input when the build selects its own, as `mbx +1.91 check`
does. Advanced workflows can provide complete `cache-key` and `restore-keys`
inputs.

## Parallel Cargo steps

Run multiple Cargo builds at the same time by starting independent lint or
test configurations together. mbx gives every compiler they launch one
machine-wide CPU and memory budget:

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
directory lock serializes the parallel steps. The mbx store and scheduler stay
shared, so the steps run side by side on one CPU and memory budget instead of
each Cargo process filling the runner on its own.

We saw mise's own lint job finish up to 45% sooner this way. Tuning and
failure behavior are covered under
[machine-wide compile scheduling](/configuration#machine-wide-compile-scheduling).

### Docker builds

Mount the mbx store and Cargo registry into the container at stable locations.
The registry may either be mounted directly at `$CARGO_HOME/registry` or
mounted elsewhere and symlinked from there:

```sh
docker run --rm \
  --env CARGO_HOME=/tmp/cargo-home \
  --env HOME=/tmp/build-home \
  --mount "type=bind,source=$HOME/.cargo/registry,target=/tmp/host-cargo-registry" \
  --mount "type=bind,source=$HOME/.cache/mbx,target=/tmp/build-home/.cache/mbx" \
  builder \
  sh -c 'mkdir -p "$CARGO_HOME" && ln -s /tmp/host-cargo-registry "$CARGO_HOME/registry" && mbx build'
```

mbx maps the registry separately from the rest of `CARGO_HOME`, so cached
compiler inputs remain portable when that child symlink resolves outside the
Cargo home directory.

### Closure bundles for action transports

An action can transport only the cache entries produced or used by its builds,
instead of archiving the whole local store. Every completed `mbx` command
writes an immutable receipt into the group named by `MBX_CACHE_EXPORT_GROUP`,
including commands running in parallel or in different checkouts.
`jdx/mr-boxington-action` sets that group itself; another transport assigns a
unique opaque value for the job before any build steps run.

A restore phase imports a previously cached bundle:

```console
mbx cache import "$RUNNER_TEMP/mbx-cache.tar"
```

The bundle also carries Cargo's scheduler state for each recorded workspace.
When import runs from a matching checkout whose target directory is absent or
empty, mbx restores the fingerprints, dep-info, build-script state, and target
layout alongside the action closure. Large compiler outputs remain CAS objects
and are reflinked or copied into that layout rather than stored twice. Import
leaves a non-empty target directory untouched.

A post phase exports the deduplicated closure of every receipt in this job:

```console
mbx cache export --group "$MBX_CACHE_EXPORT_GROUP" "$RUNNER_TEMP/mbx-cache.tar"
```

The group should include the run attempt, job, and matrix identity, or be a
fresh random value generated for the run. It identifies builds within one job.
It is not the GitHub Actions cache key. A post step should skip saving when no
completed build was recorded or when the workflow's trust policy forbids a
cache write.

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

This example installs mbx with `jdx/mise-action` instead of
[`jdx/mr-boxington-action`](https://github.com/jdx/mr-boxington-action),
because the action's GitHub Actions cache backend would store the same actions
a second time. Use one or the other.

mbx still refuses to publish from a pull request. A bucket has no server to
authorize anything, so make IAM agree: scope the role's trust policy to the
branches allowed to assume it, and give pull request jobs a role that can only
read. See [remote cache](/remote-cache#who-may-publish).

::: warning Do not use remote caches for production releases
A production release may still use `mbx` and its local cache, but should not use
a remote cache so a cache-poisoning attack cannot influence published artifacts.
Release jobs should also avoid restoring or saving the mbx store through
`actions/cache`.
:::

For a repository that combines both backends, the server for trusted runs and
the GitHub cache for fork pull requests, see
[CI with fork pull requests](/cookbook/fork-prs).
