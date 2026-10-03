---
description: Diagnose setup, low cache reuse, remote failures, and unexpected build behavior.
---
# Troubleshooting

Start with the symptom, then inspect a specific build before clearing any
cached work.

| Symptom | First check |
| --- | --- |
| Plain Cargo does not use mbx | [Check Cargo's path](/setup#verify-plain-cargo); for standalone setup, also run `mbx setup --status` |
| A build restores little or nothing | `mbx explain --last`, then [troubleshoot a low hit rate](/cache-results#troubleshooting-a-low-hit-rate) |
| A rebuild takes longer than expected | [`mbx analyze`](/analyze) ranks its uncached compiler time by cause |
| Cargo waits for a target lock | In a managed target directory, `check` and `clippy` already have a [check lane](/managed-targets#check-lanes); give other simultaneous builds separate targets as described in [Parallel builds](/scheduling) |
| Cargo metadata fails before a build | [Check workspace discovery](#workspace-discovery-fails) |
| Build storage is on NFS | Move outputs to [local build storage](/configuration#local-build-storage) |
| Remote requests fail | `mbx doctor`, then [check authentication](/remote-cache#authenticate) |
| Build storage is larger than expected | `mbx cache stats`, `mbx cache projects` (cache use per workspace), `mbx cache largest`, and `mbx gc --dry-run`; see [Budgets scale with the disk](/managed-targets#budgets-scale-with-the-disk) |
| Breakpoints point at an old checkout | See [Debug a binary restored from another checkout](/cookbook/local-development#debug-a-binary-restored-from-another-checkout) |
| A restored artifact looks wrong | [Verify restored outputs](#verify-mode) |

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
  ok  config       50.0 GiB budget, automatic gc enabled, managed targets enabled at /home/you/.cache/mbx/targets; `mbx settings ls` shows every setting
  ok  restore      /home/you/.cache/mbx -> /home/you/.cache/mbx/targets: cloning is supported
  ok  restore      /home/you/.cache/mbx -> /home/you/code/app/target: cloning is supported
  ok  archives     the archive tools already produce identical bytes for identical input
  ok  setup        mise Cargo wrapper is active and the fallback shim is current at /home/you/.local/share/mbx/bin/cargo
  ok  remote       not configured; using the local cache

0 failures, 0 warnings
```

`mbx doctor` checks:

- the Cargo and rustc executables
- cache write access
- on Unix, the local listener that compiler shims use to reach the cache agent
- whether restores can clone or hard link into Cargo's target directory and,
  with managed targets enabled, the managed target root
- whether the archive tools write timestamps into native archives
- the Cargo shim and mise's Cargo wrapper
- effective remote policy and remote protocol connectivity

Warnings describe setup problems, optional features, or fallbacks. Failures
make `mbx doctor` exit unsuccessfully.

For standalone setup, the `setup` check confirms that the Cargo shim is
installed and current, and that either mise's Cargo wrapper or that shim is
the first `cargo` on `PATH`. The check does not recognize
[native mise integration](/setup#native-mise-integration), so with native
integration a `setup` warning alone does not mean wrapping is inactive;
[check Cargo's path](/setup#verify-plain-cargo) instead.

On Unix, compiler shims normally reach the cache agent over a Unix-domain
socket. When a sandbox blocks Unix socket listeners but permits filesystem
FIFOs, mbx automatically uses FIFO transport and keeps caching enabled. If
neither transport is available, mbx warns and runs Cargo without caching
instead of preventing the build from starting.

## Workspace discovery fails

Cargo builds through mbx need a successful metadata probe to locate the
workspace and verify output storage. If the probe fails, mbx stops before
compilation. Run the probe directly with the same manifest and configuration
options:

```sh
cargo metadata --no-deps --format-version 1
```

Resolve the reported Cargo error, then retry the build. Help and cleanup
commands still pass through without this probe, as does any plain `cargo`
command run with `MBX_DISABLE` set; see
[Run Cargo without mbx](#bypass-mbx-for-one-command).

Outside a project, commands such as `cargo binstall` pass through to Cargo,
and `cargo build` in a directory with no manifest reports Cargo's own
missing-manifest error. An alias that names a local package still requires
verified build storage:

```toml
[alias]
i = ["install", "--path", "/path/to/project with spaces"]
```

When the probe fails for an alias, mbx reads the alias's arguments from Cargo
configuration, keeping each array element intact, and retries workspace
discovery for the package they name. This alias recovery supports:

- aliases from the configuration hierarchy and `CARGO_HOME`
- `CARGO_ALIAS_*` environment variables
- recursive aliases
- Cargo's built-in shorthand commands, such as `b` and `t`

It does not support:

- configuration `include` files
- aliases with `--config` overrides
- directory-changing options such as `-C`
- unstable Cargo options (`-Z`)

When mbx cannot read an alias in full and cannot verify its storage, it refuses
the alias rather than guessing. To diagnose it, spell out the build command
with its manifest or path instead of using the alias. Commands that are not
aliases keep passing through, so an `include` in your configuration does not
stop `cargo binstall` from working outside a project. mbx never reconstructs
alias arguments from `cargo --list`, which loses the boundaries of arguments
that contain whitespace.

## Inspect a build

```sh
mbx explain --last                 # explain the last recorded build
mbx explain build --workspace      # run a build with diagnostics
```

The first command does not run Cargo. The second preserves Cargo's exit status.
If Cargo considers every output fresh, there may be no compiler invocations to
explain. See [Measure cache reuse](/cache-results#measure-cache-reuse) for a
controlled comparison with fresh targets.

## Run Cargo without mbx {#bypass-mbx-for-one-command}

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
been restored earlier, use a separate target directory as shown in
[Debug a binary restored from another checkout](/cookbook/local-development#debug-a-binary-restored-from-another-checkout).

On a Unix filesystem that cannot clone, such as ext4, mbx restores outputs as
read-only hard links by default. mbx unlinks them before it runs a compiler,
but Cargo run without mbx does not, so a unit Cargo decides to rebuild can
fail with `output file ... is not writeable`. Remove the reported file, or run
`mbx clean` to discard a managed target. Setting
[`restore_hardlink = false`](/configuration#restore-hardlink) avoids the
problem for later restores. Windows is unaffected because mbx never hard links
a restored output there. See
[Output restoration](/how-it-works#output-restoration).

## Verify restored outputs {#verify-mode}

`MBX_VERIFY=1` compiles and consults the cache side by side and compares the
results. It is expensive; use it to investigate correctness, not for everyday
builds.

For routine checks, set `MBX_VERIFY_SAMPLE_RATE=5` (or `verify_sample_rate = 5`)
to verify approximately 5% of compilation identities. The range is 0 to 100,
and 0 disables sampling. Selection is stable across compiler shim processes and
build order, so rerunning the same invocation selects the same sample. This
samples units, not elapsed compiler time. `MBX_VERIFY=1` takes precedence and
verifies all eligible units. Selected units rehash inputs and disable learned
and eager incremental reuse, as full verification does.

The build summary reports what it found. Locally, the default one-line summary
lists the counts among its other outcomes, for example
`24 verified, 0 diverged`. In CI, they appear on the `mbx[cache]: object cache:`
line. With `summary = "full"` (`MBX_SUMMARY=full`; see
[Build summaries](/configuration#build-summaries)), they get a line of their
own:

```text
mbx[cache]: qualification: 24 verified, 0 diverged
```

The verified count includes divergent compilations. Each compilation
contributes at most one divergence, reporting its first mismatch. Warnings
identify the adapter, unit, and action. For stdout and stderr differences, they
also give the first differing byte offset, the line number, and bounded,
escaped excerpts of both results. mbx rewrites cached diagnostics into this
checkout's paths before comparing them.

Cargo must invoke the compiler for mbx to verify anything. Run in the checkout
that filled the cache, with a fresh target directory: an unchanged build in an
existing target can be a Cargo no-op. Keep the original target and the shared
store. `MBX_BYPASS_LOG` and `mbx explain` show what was left out.

For audits across worktrees, populate and verify using the same virtual source
root. For example, run this from each checkout's workspace root, first with
`MBX_VERIFY=0` to populate, then with `MBX_VERIFY=1` in the other checkout:

```sh
RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }--remap-path-prefix=$PWD=/workspace" \
  CARGO_TARGET_DIR="$PWD/target-audit" MBX_VERIFY=1 mbx build --all-targets --locked
```

Use a fresh `target-audit` directory each time and the same toolchain, profile,
and other compiler flags. The source side of `--remap-path-prefix` is keyed
portably; its virtual destination must agree between checkouts. If you use
`CARGO_ENCODED_RUSTFLAGS`, add the remap there instead: Cargo gives it
precedence over `RUSTFLAGS`.

Remapping reduces differences in embedded source paths, but it does not
guarantee byte-identical outputs, especially for native links or paths outside
the mapped root. See
[Restored artifacts are equivalent, not always identical](/limits#restored-artifacts-are-equivalent-not-always-identical).
Investigate remaining divergences rather than treating every cross-worktree
mismatch as harmless. Report unexplained differences with the unit and action
the warning identifies. See [Report a problem](#reporting-a-problem).

Use verification to check a caching feature against your own workload, including
[native link caching](/limits#native-linking-is-cached-only-where-the-linker-can-be-described).

## Report a problem {#reporting-a-problem}

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
the crates you build. mbx never prints credentials. Remove anything else you
would rather not publish before posting.

`MBX_LOG` takes an [env_logger](https://docs.rs/env_logger) filter, so `debug`,
`trace`, or a per-module filter such as `mbx=trace` all work; it defaults to
`info,portable_pty=off`. It filters logs in the `mbx` process that drives the
build, including routine compiler shim diagnostics forwarded to the build
session. Use `MBX_BYPASS_LOG` for per-compilation bypass records, or
`mbx explain` for a grouped summary. See [Cache results](/cache-results).

Report a problem in
[Q&A discussions](https://github.com/jdx/mr-boxington/discussions/categories/q-a),
and propose a change in
[Ideas](https://github.com/jdx/mr-boxington/discussions/categories/ideas).
Report a suspected vulnerability through
[private advisory reporting](https://github.com/jdx/mr-boxington/security/advisories/new)
instead. See
[SECURITY.md](https://github.com/jdx/mr-boxington/blob/main/SECURITY.md).
