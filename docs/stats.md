---
description: Read lifetime compiler savings, pruning totals, and estimated workspace sharing with mbx stats.
---
# Savings and statistics

`mbx stats` shows what sharing builds has saved on this machine and the date
counting began. It prints durations such as `50m 59s` or `2d 4h`.

```sh
mbx stats
mbx stats --json
```

The report combines lifetime savings with a snapshot of the store, its recorded
workspaces, and the [managed target directories](/managed-targets). The
**Store** screen in [`mbx tui`](/tui#screens) shows the same savings and sharing
figures. [`mbx cache stats`](/cli/cache/stats) is the smaller report on the
action store, managed targets, and learned incremental state currently held.

## What the numbers mean

| Figure | Meaning |
| --- | --- |
| Compiler time avoided | Estimated compiler work skipped by cache hits, accumulated across builds. |
| Cache hits | Compilations served from cache since counting began. |
| Pruned by mbx | Cumulative bytes collected from managed targets, learned incremental state, [generated source trees](/limits#out-dir-sharing), and the store, by both automatic sweeps and explicit `mbx gc` runs. The `targets / cache` split counts everything but the store as `targets`. |
| Automatically pruned | Bytes collected by automatic sweeps, counted from a separate start date. |
| Requested removals | Bytes removed by confirmed migrations or explicit workspace removals, kept separate from pruning. |
| Copying avoided | Cumulative output bytes materialized by reflink or hard link rather than copied. |
| With separate caches | Sum of the logical cache bytes each live recorded workspace can reach, counting shared content once for each workspace. |
| Duplication avoided | A conservative lower-bound estimate: the separate-cache sum minus the entire shared store, floored at zero. |

Read these figures with their limits in mind:

- **Compiler time avoided** is not elapsed wall-clock time saved, because
  compilations run in parallel.
- **Pruned by mbx**, **Automatically pruned**, and **Requested removals** are
  logical file sizes, not physical space reclaimed.
- **Copying avoided** is not the disk space saved now: files may later change
  or be deleted.
- **Duplication avoided** understates sharing. The store also holds objects no
  live workspace claims, and mbx subtracts those too.

The sharing estimate counts logical cache bytes, not physical filesystem
blocks, and leaves out target directories. It covers independent workspaces as
well as worktrees. mbx records each checkout it builds, and a recorded
workspace counts as live while its checkout is still on disk and mbx has built
it in the last 30 days. To attribute bytes to workspaces, mbx walks these
checkout records and the cache entries each workspace can reach. The TUI
repeats that walk every 30 seconds while the Store screen is open. On an
active machine, builds can change the store while mbx is reading it.

mbx keeps the lifetime totals in a savings ledger. Dates use UTC. Until the
ledger records anything, the report says `no savings recorded yet`; reading a
report does not start a new ledger. **Automatically pruned** starts counting at
the first ledger update from an mbx version that tracks automatic sweeps
separately, and shows `not tracked yet` until then. Older versions did not
distinguish automatic from explicit collection, so mbx does not guess or
backfill that history.

## JSON report

The [versioned JSON report](/stability#json-output-is-versioned) includes
`version`, `store`, `savings`, `cache`, and `sharing`. Sizes are integer
bytes, durations are nanoseconds, and times are Unix timestamps in seconds; an
unknown start time is `null`. Its field names identify estimates and
cumulative counters. JSON never contains quips.

## A little personality

With the default [`savings = "quips"`](/configuration#the-savings-line), the
text report and the TUI add a line grounded in the recorded figures, such as
“321 compilations served reheated. rustc can finish its coffee.” Set
`savings = "plain"` or `savings = "off"` to omit the quip. `mbx stats` and the
Store screen still show every figure with either setting.
