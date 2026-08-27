# Cache results

The summary separates work mbx handled from work it could not safely cache.

## Hit

mbx derived an action key, found its result, and restored the outputs. A hit can
come from the local store or a configured remote.

## Miss

mbx derived a key and looked it up, but no result existed. The compilation ran
and its successful result was stored.

## Could not look up

mbx did not yet have usable dep-info or a prediction from which to derive the
key. This is common on a genuinely cold build. The action is still stored after
compilation, so calling it a miss would overstate the number of failed lookups.

## Bypass

mbx recognized that it could not model the action exactly and ran the real
compiler without caching it. Reasons are grouped in the summary; set
`MBX_BYPASS_LOG` to a file path for the full per-action record.

Run a Cargo command through `mbx explain` to collect those records temporarily,
group identical causes, and print guidance for every bypass category:

```sh
mbx explain build --workspace
```

The command preserves Cargo's exit status after printing the explanation.

## Remote failure

A remote cache request failed and the build carried on without it: unreachable
host, refused credentials, or a response this client would not accept. mbx never
fails a build over a remote cache, so these only ever cost hit rate — but a
remote that is failing every request reports the same hits, misses, and bytes as
one that was merely empty, which is why the summary counts them:

```text
mbx[cache]: the remote cache failed 4 of its requests; this build ran without what it could not reach, and the warnings above say why
```

The individual warnings, printed as the build runs, say what failed. The count
also appears as `remote_failures` in the JSON statistics report, so CI can alert
on a cache that has quietly stopped serving.

## Watching a build instead

Everything above describes the summary printed after a build. To see the same
outcomes as they are decided -- one row per compilation, with the crate it
belongs to -- run [`mbx tui`](/tui) in another terminal. It reads every build on
the machine, including ones already running.

## Reading the hit rate

A build can report a high hit rate among attempted lookups while spending most
of its time on actions that were not looked up or were bypassed. Read all three
summary lines together, and compare wall-clock time when evaluating the cache.

Native link steps always run, so an otherwise warm native binary build still
has work to do. Binaries, tests, and `cdylib`s for supported self-contained
WebAssembly targets are the exception and may be restored as hits.

## Troubleshooting a low hit rate

Run the build through `mbx explain` first — it collects the per-action
records, groups identical causes, and prints guidance for each category:

```sh
mbx explain build --workspace
```

The usual causes, roughly in the order they show up:

- **The store is cold.** A first build has no dep-info to derive keys from, so
  "could not look up" dominates and everything is stored rather than restored.
  The second build is the honest measurement.
- **Incremental builds are enabled.** With `MBX_INCREMENTAL=1`, workspace
  members compile incrementally, those compilations bypass the cache, and the
  changed artifacts make crates above them miss too. See
  [limits](/limits#incremental-compilations-are-not-cached).
- **Link steps always run.** Native binaries, tests, and dylibs re-link even
  when every compilation hit, so a warm build of a large binary still takes
  time. Self-contained WebAssembly targets are the exception.
- **The inputs actually differ.** A different toolchain, feature set, profile,
  or `RUSTFLAGS` between two checkouts is a different key, and the summary
  reports it as an ordinary miss. `cargo tree` and comparing the two commands
  usually finds it.
- **Build-script output paths.** A crate that embeds its `OUT_DIR` produces
  checkout-specific inputs for its dependents. `MBX_SHARE_OUT_DIR=1` remaps
  it; see [limits](/limits#out_dir-sharing-is-opt-in).
- **A build chose its own C compiler.** Setting `CC` or `CXX` leaves that
  build's C and C++ compilations uncached, and bypass kinds beginning `cc-`
  report anything the C adapter declined to model. See
  [limits](/limits#c-and-c-caching-covers-build-script-compiles-only).
- **CI restored nothing.** On GitHub Actions, check that the cache step
  actually restored an entry — a changed `cache-generation` or a fresh
  repository starts empty by design. With a remote cache configured, check the
  [remote failure](#remote-failure) count too: a remote that is failing every
  request reports the same zeros as one that is merely empty.

## Compiler time

The session summary reports real compiler time by outcome and an estimate of
the compiler time avoided by cache hits. The estimate comes from the duration
recorded with the successful compilation that populated the action prediction;
older predictions without a timing hint contribute zero rather than being
guessed. The five crates with the largest cumulative uncached compiler time are
listed so optimization work can target wall-clock cost instead of action count.

The version 2 JSON statistics report exposes the same data in
`estimated_compiler_duration_avoided_ns`, `compiler`, and
`slow_compilations`.
