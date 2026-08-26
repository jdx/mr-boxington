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
- **CI restored nothing.** On GitHub Actions, check that the cache step
  actually restored an entry — a changed `cache-generation` or a fresh
  repository starts empty by design.

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
