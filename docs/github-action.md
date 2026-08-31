# GitHub Action

[`jdx/mr-boxington-action`](https://github.com/jdx/mr-boxington-action)
installs mbx and connects it to either GitHub Actions cache or an mbx-compatible
server. The examples below show the inputs that matter for each backend; the
action's repository documents the complete list.

::: warning Current performance
mbx does not currently outperform
[Swatinem/rust-cache](https://github.com/Swatinem/rust-cache) in our
GitHub-hosted runner benchmarks consistently. A
[warm-cache run in jdx/hk](https://github.com/jdx/hk/actions/runs/33395159164)
finished the measured Cargo build with rust-cache in 16.6 seconds on Linux,
17.6 seconds on macOS, and 121 seconds on Windows. The server-backed mbx jobs
took 211, 212, and 320 seconds respectively.

The GitHub-cache backend narrowed the gap but did not reverse it. In a
[separately seeded warm run](https://github.com/jdx/hk/actions/runs/33439934246),
the measured Cargo builds took 15.54 versus 22.24 seconds on Linux, 24.64
versus 33.01 seconds on macOS, and 55.65 versus 123 seconds on Windows for
rust-cache and mbx respectively. Including checkout, cache restore, and action
setup, the corresponding full jobs took 24 versus 49 seconds, 45 versus 60
seconds, and 93 versus 183 seconds.

The result depends on the workload. In a
[warm `jdx/mise` run](https://github.com/jdx/mise/actions/runs/33440683366),
GitHub-backed mbx beat rust-cache on Linux: 38.79 versus 129 seconds for the
Cargo build and 97 versus 163 seconds for the full job. It lost on macOS
(307 versus 207 seconds for Cargo; 379 versus 276 seconds for the job) and
Windows (737 versus 305 seconds for Cargo; 1,070 versus 392 seconds for the
job). The Windows mbx job spent 199 seconds restoring and importing its cache
before Cargo started.

These results are not a claim about every environment: they measure fresh
hosted runners, with rust-cache restoring a `target/` archive and mbx restoring
fine-grained actions through either backend. They do show that you should not
replace rust-cache with mbx solely for CI speed today. Benchmark the complete
job, including setup and transfer time, before migrating.

The tradeoff can still favor mbx when the cache also serves local worktrees,
when several builds benefit from shared scheduling and in-flight
deduplication, when fine-grained reuse across changed builds matters more than
restoring one target archive, or when detailed diagnostics and a controlled
self-hosted remote are requirements. A nearby remote can also change the
transfer tradeoff.
:::

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
directory lock serializes the supposedly parallel steps. The mbx store and
scheduler stay shared, so the steps run side by side on one CPU and memory
budget rather than each Cargo process trying to fill the runner on its own.

We saw mise's own lint job finish up to 45% sooner this way. Tuning and
failure behavior are covered under
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

### Closure bundles for action transports

An action can transport only the cache entries produced or used by its builds,
instead of archiving the whole local store. Before any build steps, assign a
unique opaque value to `MBX_CACHE_EXPORT_GROUP` for that job. Every completed
`mbx` command writes an immutable receipt into the group, including commands
running in parallel or in different checkouts.

The action's restore phase imports a previously cached bundle:

```console
mbx cache import "$RUNNER_TEMP/mbx-cache.tar"
```

Its post phase exports the deduplicated closure of every receipt in this job:

```console
mbx cache export --group "$MBX_CACHE_EXPORT_GROUP" "$RUNNER_TEMP/mbx-cache.tar"
```

The group should include the run attempt, job, and matrix identity, or be a
fresh random value generated by the action. It identifies builds within one
job; it is not the GitHub Actions cache key. A post step should skip saving when
no completed build was recorded or when the workflow's trust policy forbids a
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
