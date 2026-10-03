---
description: Add mbx to GitHub Actions, choose a cache backend, run parallel builds, and handle release jobs.
---
# GitHub Action

[`jdx/mr-boxington-action`](https://github.com/jdx/mr-boxington-action)
installs or reuses mbx and configures caching for the job. Its `backend` input
chooses where the cache lives:

- `github`, the default, uses GitHub Actions cache. Start here.
- `remote` points mbx at a [cache server](#cache-server) or an
  [S3-compatible bucket](#s3-compatible-bucket). Use it when you need object
  sharing across runners.
- `local` keeps mbx's cache directory on the runner and never restores or
  saves it, which suits persistent runners.

When the `version` input is omitted, the action uses `mbx` from `PATH` and
installs the latest release only when none is found. The
[action repository](https://github.com/jdx/mr-boxington-action) owns the
complete input reference.

## Start with a complete workflow

Save this as `.github/workflows/ci.yml`. It uses the runner's Rust toolchain;
add your toolchain installation step before the cache action if you pin one.

```yaml
name: ci

on:
  push:
    branches: [main]
  pull_request:

permissions:
  contents: read

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
      - uses: jdx/mr-boxington-action@v1
      - run: mbx test --workspace
```

The push to `main` fills the cache for later runs. Pull requests restore it
without saving new entries. Most examples on this page are snippets for an
existing job's `steps` list.

## Read the CI summary

In GitHub Actions, mbx prints the explanatory `ci` style of its build summary
and skips the first-build notice about local cache management. The summary
covers only mbx's store. It does not count artifacts Cargo reused directly from
the restored target directory, or the action's own cache restore and save; the
action reports its restore and save results separately. Set `MBX_SUMMARY` to
`short`, `full`, or `off` to override the default. See
[Build summaries](/configuration#build-summaries).

## GitHub Actions cache

The default `github` backend keeps its entries in GitHub Actions cache. Its
default `target` payload restores a pruned Cargo target directory and Cargo's
registry and git downloads from a compatible entry, so Cargo can reuse its own
fresh outputs.

```yaml
permissions:
  contents: read

steps:
  - uses: actions/checkout@v7
  - uses: jdx/mr-boxington-action@v1
  - run: mbx test --workspace
```

The action saves a new immutable entry after a successful run for a push to
the default branch. Other runs are restore-only unless one of these inputs opts
them in:

- `save-on-pull-request` saves same-repository pull requests into that pull
  request's own cache scope.
- `save-on-protected-branch` saves pushes to other protected branches.
- `save-on-workflow-dispatch` saves `workflow_dispatch` runs. Enable it only in
  trusted workflows.

Fork pull requests never save.

The `target` payload carries the `target/` directory of the Cargo workspace
named by the `working-directory` input. The input is resolved against the job's
working directory, which is normally the checkout root. Set `working-directory`
when the workspace is below the checkout root; otherwise its outputs are not
saved. A target directory moved with `CARGO_TARGET_DIR` or Cargo's
`build.target-dir` is not carried either.

```yaml
- uses: jdx/mr-boxington-action@v1
  with:
    working-directory: rust
- run: mbx test --workspace
  working-directory: rust
```

With the `target` payload, the action also changes three mbx settings for the
rest of the job:

- It turns off [managed target directories](/managed-targets), so Cargo builds
  in the workspace's own `target/`.
- It turns off
  [native link caching](/limits#native-linking-is-cached-only-where-the-linker-can-be-described),
  whatever the `cache-links` input says.
- It clears `MBX_REMOTE_URL`, which disables any remote cache configured in an
  earlier step or in mbx's config file.

Set `github-cache-mode: objects` when several target directories or checkout
layouts need to share one entry. The `objects` payload carries the cache
entries the job's builds used or produced, and leaves out the Cargo registry.
Cargo may download crates again unless you cache those downloads separately.

```yaml
- uses: jdx/mr-boxington-action@v1
  with:
    github-cache-mode: objects
```

On GitHub-hosted runners, the `objects` payload also sets `MBX_GC_AUTO=0` for
the rest of the job, unless `MBX_GC_AUTO` is already set, so the restored
entries stay available. `mbx gc` still collects when you run it. To keep
automatic collection, set `MBX_GC_AUTO=1` in the job's `env`.

Change `cache-generation` to start a new series of entries, for example after
a format or policy change:

```yaml
- uses: jdx/mr-boxington-action@v1
  with:
    cache-generation: v2
```

### Match the toolchain

Generated keys cover the operating system, the architecture, and the identity
of the `rustc` on `PATH`, because a store built by one compiler matches nothing
under another. Install the toolchain before the action, and name it in the
action's `toolchain` input when the build selects its own, as `mbx +1.91 check`
does. Advanced workflows can provide complete `cache-key` and `restore-keys`
inputs. The `toolchain` input scopes the key; it does not install or select
the toolchain for the build.

## Cache server

For trusted runners and teams, the `remote` backend points mbx at a compatible
cache server, such as the self-hostable one described in
[Cache server](/cache-server). The action exports the remote configuration for
subsequent steps:

```yaml
permissions:
  contents: read
  id-token: write

steps:
  - uses: actions/checkout@v7
  - uses: jdx/mr-boxington-action@v1
    with:
      backend: remote
      remote-url: https://cache.example.com
      namespace: acme/backend
      oidc-audience: https://cache.example.com
  - run: mbx build --workspace --all-features
```

mbx writes to the server only from a push to a protected branch. Pull
requests, tag pushes, `release` events, and scheduled or manually dispatched
runs degrade to read-only. See
[Read and write policy](/remote-cache#read-and-write-policy).

If fork authors must not reach the host, use the `github` backend for those
jobs instead. When OIDC is unavailable, pass a bearer token in the action's
`token` input.

To combine both backends, with the server for trusted runs and GitHub Actions
cache for fork pull requests, see
[CI with fork pull requests](/cookbook/fork-prs).

## S3-compatible bucket

A bucket needs nothing running.
[`aws-actions/configure-aws-credentials`](https://github.com/aws-actions/configure-aws-credentials)
exchanges the runner's OIDC token for a role and exports the credentials mbx
reads, so no long-lived secret is stored:

```yaml
permissions:
  contents: read
  id-token: write

steps:
  - uses: actions/checkout@v7
  - uses: aws-actions/configure-aws-credentials@v6
    with:
      role-to-assume: arn:aws:iam::111122223333:role/mbx-cache
      aws-region: us-west-2
  - uses: jdx/mr-boxington-action@v1
    with:
      backend: remote
      remote-url: s3://acme-build-cache
      namespace: acme/backend
  - run: mbx build --workspace --all-features
```

The `remote` backend keeps every `MBX_REMOTE_*` value an earlier step exported
unless one of its inputs replaces it. A step that already points mbx at a
bucket therefore needs only `backend: remote`. The action fails the step when
mbx finds no remote configured anywhere, which catches a configuring step that
runs too late.

mbx still refuses to publish from a pull request. A bucket has no server to
authorize anything, so make IAM agree: scope the role's trust policy to the
branches allowed to assume it, and give pull request jobs a role that can only
read. See [Who may publish](/remote-cache#who-may-publish).

## Parallel Cargo steps

GitHub's [`parallel` step group](https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax#jobsjob_idstepsparallel)
starts independent lint or test configurations at the same time. mbx gives
every compiler they launch one machine-wide CPU and memory budget:

```yaml
steps:
  - uses: actions/checkout@v7
  - uses: jdx/mr-boxington-action@v1
    with:
      github-cache-mode: objects
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

The separate target directories are also why the example sets
`github-cache-mode: objects`. The default `target` payload carries only the
workspace's own `target/` directory and Cargo's downloads, not the
`clippy-default` and `clippy-all` directories these steps build in. With no
workspace `target/` to save, it would restore and save nothing for them. The
`objects` payload carries the cache entries every step used or produced,
whichever target directory each one built in.

Measure the complete job on your workload. See [Parallel builds](/scheduling)
for tuning and [Six parallel jobs](/benchmarks#six-parallel-jobs) for a
measured batch.

## Docker builds

Mount mbx's cache directory and the Cargo registry into the container at stable
locations. Mount the registry either directly at `$CARGO_HOME/registry` or
elsewhere with a symlink from there:

```sh
docker run --rm \
  --env CARGO_HOME=/tmp/cargo-home \
  --env HOME=/tmp/build-home \
  --env MBX_SHIMS_DIR=/tmp/mbx-shims \
  --mount "type=bind,source=$HOME/.cargo/registry,target=/tmp/host-cargo-registry" \
  --mount "type=bind,source=$HOME/.cache/mbx,target=/tmp/build-home/.cache/mbx" \
  --mount "type=volume,source=mbx-builder-shims,target=/tmp/mbx-shims" \
  builder \
  sh -c 'mkdir -p "$CARGO_HOME" && ln -s /tmp/host-cargo-registry "$CARGO_HOME/registry" && mbx build'
```

mbx maps the registry separately from the rest of `CARGO_HOME`, so cached
compiler inputs stay portable when the registry symlink resolves outside the
Cargo home directory.

The container's mbx is a separate installation from the one on the runner, so
the example gives it a compiler shim directory of its own in a named volume,
outside the shared cache directory. The volume persists between container
runs, which the shim directory must do because CMake and other build systems
can record absolute compiler paths. See
[Containers sharing a cache](/configuration#containers-sharing-a-cache).

### cargo-chef

`cargo chef cook` creates its skeleton workspace itself, so in a fresh build
stage it starts in a directory with no `Cargo.toml`. The Cargo shim finds no
manifest and hands the command to real Cargo without a build session (the
cache agent and shims mbx starts for one command). The build that cargo-chef
then starts through `$CARGO` never reaches mbx, so the cook compiles every
unit on every run. mbx prints `no Cargo manifest in scope` when this happens.

Write the skeleton first, so the cook starts with a manifest in scope:

```sh
cargo chef cook --no-build --recipe-path recipe.json
cargo chef cook --recipe-path recipe.json
```

The second command runs inside a build session and caches the dependency
compilations like any other build.

## Production releases

::: warning Keep releases off shared caches
A production release may still use mbx and its local cache. It should not use a
remote cache, so that a cache-poisoning attack cannot influence published
artifacts. Release jobs should also avoid restoring or saving compiler outputs
through `actions/cache` or the `github` backend. mbx refuses to write from tag
pushes and `release` events, but it still reads from a configured remote. A
release job triggered by a push to a protected branch keeps its configured
mode. Remove the remote configuration and cache restore steps explicitly. To
install mbx in a release job, use the action's `backend: local`, which clears
`MBX_REMOTE_URL` for later steps and never restores or saves a cache.
:::

## Manual GitHub Actions cache setup {#manual-github-cache-setup}

To control the entry yourself, for example to keep Cargo's download caches in
the same entry or to apply a custom save policy, assemble the pieces directly:

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
- run: mbx gc --max-size 3GiB
  if: always()
```

`--max-size` caps only the action store. A CI checkout with no `target/`
builds in a [managed target](/managed-targets) under the cache directory's
`targets` directory, so this entry also saves Cargo's outputs. Collection never
removes the most recently used managed target to meet a size budget. To keep
Cargo's outputs out of the entry, set `MBX_TARGET_VIEWS=0` in the job's `env`.

mbx's default cache directory depends on the platform, so `~/.cache/mbx`
applies only to Linux runners:

- Linux: `~/.cache/mbx`, or `$XDG_CACHE_HOME/mbx` when that variable is set
- macOS: `~/Library/Caches/mbx`
- Windows: `%LOCALAPPDATA%\mbx`

To use one path on every runner, set `MBX_CACHE_DIR` to an absolute path in the
job's `env` and list that directory in `path` instead.

In pull requests, use `actions/cache/restore` instead of `actions/cache` so the
run restores the entry without also saving one. Storage permissions and
workflow trust still determine what code in the job can access.

::: tip Pin actions in production
The examples use major tags for readability. Pin third-party actions to full
commit SHAs in a real workflow.
:::

## Export and import closure bundles {#closure-bundles-for-action-transports}

A CI cache step can save only the cache entries a job's builds used or
produced, called their closure, instead of archiving the whole store. The
`objects` payload works this way, and these commands let another cache step do
the same.

Every completed `mbx` command writes an immutable receipt, a record of what it
used and produced, into the group named by `MBX_CACHE_EXPORT_GROUP`. That
includes commands running in parallel or in different checkouts.
`jdx/mr-boxington-action` sets the group itself only for the `github` backend
with `github-cache-mode: objects`. The default `target` payload and the `local`
and `remote` backends set no group. Any other cache step must assign a unique
opaque value for the job before any build steps run. Include the run attempt,
job, and matrix identity in it, or generate a fresh random value for the run.
The group identifies the builds of one job, not the GitHub Actions cache key.

A restore phase imports a previously cached bundle:

```sh
mbx cache import "$RUNNER_TEMP/mbx-cache.tar"
```

Import accepts a tar archive or a directory. It consumes a directory: the
objects move into the store and the directory is removed.

The bundle also carries Cargo's scheduler state for each recorded workspace.
When import runs from a matching checkout whose target directory is absent or
empty, mbx restores the fingerprints,
[dep-info](/how-it-works#prediction-and-dep-info), build-script state, and
target layout alongside the closure. Large compiler outputs stay in the store
and are reflinked or copied into that layout rather than stored twice. Import
leaves a non-empty target directory untouched.

A post phase exports the deduplicated closure of every receipt in this job:

```sh
mbx cache export --group "$MBX_CACHE_EXPORT_GROUP" "$RUNNER_TEMP/mbx-cache.tar"
```

Without `--group`, export covers only this checkout's last build. Add
`--format directory` to write a directory instead of a tar when the cache step
archives a directory itself, as `actions/cache` does; a tar inside that archive
means every byte is written twice on restore. `jdx/mr-boxington-action` uses
the directory form with mbx 1.12 or newer.

A post step should skip saving when no completed build was recorded, which
export reports as `no completed mbx builds are recorded for export group`, or
when the workflow's trust policy forbids a cache write.
