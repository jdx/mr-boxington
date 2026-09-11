---
description: Diagnose setup, low cache reuse, remote failures, and unexpected build behavior.
---
# Troubleshooting

Start with the symptom, then inspect a specific build before clearing any
cached work.

| Symptom | First check |
| --- | --- |
| Plain Cargo does not use mbx | [Check Cargo's path](/setup#verify-plain-cargo); for standalone setup, also run `mbx setup --status` |
| A build restores little or nothing | `mbx explain --last`, then [read the results](/cache-results#troubleshooting-a-low-hit-rate) |
| Cargo waits for a target lock | Give simultaneous builds [separate targets](/scheduling) |
| Remote requests fail | `mbx doctor`, then [check authentication](/remote-cache#authenticate) |
| Build storage is larger than expected | `mbx cache stats` and `mbx gc --dry-run`; review [budgets](/managed-targets#budgets-scale-with-the-disk) |
| Breakpoints point at an old checkout | Use the [debugger recipe](/cookbook/local-development#debug-a-binary-restored-from-another-checkout) |
| Restores are slow in CI | `mbx doctor`, then [align the cache and target filesystems](#restores-are-slow-in-containerized-ci) |

## Diagnose the installation

```sh
mbx doctor
```

Example output (versions, paths, and budgets depend on your machine):

```text
  ok  cargo        cargo 1.98.0 (797e8a9bc 2026-08-05)
  ok  rustc        rustc 1.98.0 (88d9e12ae 2026-08-18)
  ok  cache        /home/you/.cache/mbx is writable
  ok  session      Unix-domain listeners are available
  ok  config       50.0 GiB budget, automatic gc enabled, managed targets enabled at /home/you/.cache/mbx/targets
  ok  reflink      supported by the cache filesystem
  ok  setup        mise Cargo wrapper is active and the fallback shim is current at /home/you/.local/share/mbx/bin/cargo
  ok  remote       not configured; using the local cache

0 failures, 0 warnings
```

Doctor's setup check currently recognizes explicit mise wrappers and the
standalone shim installed by `mbx setup`, but not the native Rust tool option.
Its setup warning alone does not mean native wrapping is inactive;
[check Cargo's path](/setup#verify-plain-cargo).

For standalone setup, doctor checks that mise's Cargo wrapper is active, or
that the installed fallback
shim matches the running mbx and is the first `cargo` on `PATH`. It also checks
the Cargo and rustc executables, cache write access, the local build-session
listener, filesystem reflink support, effective remote policy, and remote
protocol connectivity. Warnings describe setup problems, optional features, or
fallbacks; failures make the command exit
unsuccessfully.

Build sessions normally communicate over a Unix-domain socket. When a sandbox
blocks Unix socket listeners but permits filesystem FIFOs, mbx automatically
uses FIFO transport and keeps caching enabled. If neither transport is
available, mbx warns and runs Cargo without caching instead of preventing the
build from starting.

## Restores are slow in containerized CI

[Copy-on-write output restoration](/how-it-works#copy-on-write-output-restoration)
needs the cache directory and the target directory on the same filesystem.
Many containerized CI runners (GitLab's Docker and Kubernetes executors,
Kubernetes-based GitHub Actions runners, and similar setups) mount the job
workspace on an overlay filesystem while the default cache directory resolves
to a different mount, such as an emptyDir volume or the image's own layer.
Reflinking then fails with "different filesystems" on every restore, and mbx
falls back to a full byte copy for every cached output:

```text
warn  reflink      /root/.cache/mbx -> /builds/acme/backend/target: different filesystems (CrossesDevices); restores to this location may require copying. Containerized CI runners often mount an overlay filesystem for the workspace while the cache directory defaults elsewhere; point MBX_CACHE_DIR at a path on the same filesystem as the target directory (for example, inside the job workspace) to restore cloning.
```

Point `MBX_CACHE_DIR` at a path on the same filesystem as the target directory,
typically somewhere under the job's own workspace:

```yaml
# GitLab CI
variables:
  MBX_CACHE_DIR: $CI_PROJECT_DIR/.mbx-cache
```

Re-run `mbx doctor` after the change; the `reflink` check should report
`cloning is supported` for that layout. Persist `MBX_CACHE_DIR` across jobs
with your CI's own cache mechanism (for example, GitLab's `cache:` key) the
same way you would persist `~/.cache/mbx` otherwise, or the cache starts cold
on every job.

### The warning says "cloning unsupported" instead

A different `mbx doctor` warning names the failure "cloning unsupported"
rather than "different filesystems":

```text
warn  reflink      /root/.cache/mbx -> /builds/acme/backend/target: cloning unsupported (Operation not supported (os error 95)); restores to this location may require copying. The filesystem driver here has no clone operation at all, regardless of which directory is used...
```

`MBX_CACHE_DIR` does not fix this one. "Different filesystems" means the two
paths are on separate mounts, which choosing a co-located directory solves.
"Cloning unsupported" means the filesystem driver itself has no clone
operation, so every directory on that filesystem is equally unable to
reflink. This is common when a container's root filesystem uses a storage
driver such as Docker's `overlay2` without a reflink-capable backing
filesystem; reflinks need the backing filesystem itself to support them, such
as Btrfs or XFS formatted with `reflink=1`. Ask whoever manages the runner or
container host to check the storage driver and backing filesystem. Until
that changes, restores on that host copy bytes instead of cloning them, and
`MBX_CACHE_DIR` placement has no effect either way.

## Inspect a build

```sh
mbx explain --last                 # read the last recorded session
mbx explain build --workspace      # run a build with diagnostics
```

The first command does not run Cargo. The second preserves Cargo's exit status.
If Cargo considers every output fresh, there may be no compiler invocations to
explain. See [Measure cache reuse](/cache-results#measure-cache-reuse) for a
controlled comparison with fresh targets.

## Bypass mbx for one command

In a POSIX shell:

```sh
MBX_DISABLE=1 cargo build
```

In PowerShell:

```powershell
$env:MBX_DISABLE = "1"
try { cargo build } finally { Remove-Item Env:MBX_DISABLE }
```

This keeps Cargo's existing outputs. To investigate an artifact that may have
been restored earlier, use a separate target directory as shown in the
[debugger recipe](/cookbook/local-development#debug-a-binary-restored-from-another-checkout).

## Reporting a problem

Three things describe almost any mbx problem:

```sh
mbx doctor --json
MBX_LOG=debug mbx build
MBX_BYPASS_LOG=bypass.log mbx build
```

The doctor report describes the environment; `MBX_LOG` records build diagnostics
and `MBX_BYPASS_LOG` records per-compilation bypass reasons.

All three describe your machine: `mbx doctor --json` reports absolute cache
paths and the URL and namespace of any configured remote, and the logs name
the crates you build. Credentials are never printed. Remove anything else you
would rather not publish before posting.

`MBX_LOG` takes an [env_logger](https://docs.rs/env_logger) filter, so
`debug`, `trace`, or a per-module filter such as `mbx=trace` all work; it
defaults to `info`. It covers the `mbx` process that drives the build. The
rustc shim runs without a logger, so per-compilation detail comes from
`MBX_BYPASS_LOG` instead, or from `mbx explain` for a grouped summary. See
[Cache results](/cache-results).

Report a problem in
[Q&A discussions](https://github.com/jdx/mr-boxington/discussions/categories/q-a),
and propose a change in
[Ideas](https://github.com/jdx/mr-boxington/discussions/categories/ideas).
A suspected vulnerability goes through
[private advisory reporting](https://github.com/jdx/mr-boxington/security/advisories/new);
see [SECURITY.md](https://github.com/jdx/mr-boxington/blob/main/SECURITY.md).
