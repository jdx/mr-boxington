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

## Why hit rate is not the whole story

A build can report a high hit rate among attempted lookups while spending most
of its time on actions that were not looked up or were bypassed. Read all three
summary lines together, and compare wall-clock time when evaluating the cache.

Native link steps always run, so an otherwise warm native binary build still
has work to do. Default-linked `wasm32-unknown-unknown` binaries and tests are
the exception and may be restored as hits.
