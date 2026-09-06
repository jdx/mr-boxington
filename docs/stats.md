# Savings and statistics

`mbx stats` shows what sharing builds has saved on this machine, with the date
counting began and durations such as `50m 59s` or `2d 4h`.

```sh
mbx stats
mbx stats --json
```

The report combines lifetime savings with a snapshot of the cache and its
recorded workspaces. The **Store** screen in [`mbx tui`](/tui) shows the same
savings and sharing figures. `mbx cache stats` remains the smaller report about
what the store currently holds.

## What the numbers mean

| Figure | Meaning |
| --- | --- |
| Compiler time avoided | Estimated compiler work skipped by cache hits, accumulated across builds. Parallel compilation means this is not elapsed wall-clock time saved. |
| Cache hits | Compilations served from cache since counting began. |
| Pruned by mbx | Cumulative bytes collected from managed targets and the cache, including both automatic sweeps and explicit `mbx gc` runs. |
| Automatically pruned | Bytes collected by automatic sweeps, with a separate start date. Older versions did not distinguish automatic from explicit GC, so their history is not guessed or backfilled. |
| Requested removals | Bytes removed by confirmed migrations or explicit workspace removals, kept separate from pruning. |
| Copying avoided | Cumulative output bytes materialized by reflink rather than copying. This is not the amount of disk space currently saved: files may later change or be deleted. |
| With separate caches | Sum of logical cache bytes reachable by each live recorded workspace, counting shared content once for each workspace. |
| Duplication avoided | A conservative lower-bound estimate: the separate-cache sum minus the entire shared store, floored at zero. Unclaimed objects in the store make this underestimate sharing. |

The sharing estimate covers logical cache bytes, excludes target directories,
and includes independent workspaces as well as worktrees. It does not measure
physical filesystem blocks. On an active machine, builds can change the store
while the report is being read. Workspace attribution walks cache records and
targets; the TUI refreshes it every 30 seconds while the Store screen is open.

Dates use UTC. Missing savings history is shown as unrecorded; reading a report
does not start a new ledger. The automatic-pruning period begins with the first
ledger update from a version that tracks it separately.

## A little personality

With the default `savings = "quips"`, the text report and TUI add a line grounded
in the recorded figures, such as “321 compilations served reheated. rustc can
finish its coffee.” Set `savings = "plain"` or `savings = "off"` to omit the
quip. Explicitly requested statistics are still shown.

The [versioned JSON report](/stability#json-output-is-versioned) includes
`version`, `store`, `savings`, `cache`, and `sharing`. It uses integer bytes,
nanoseconds, and Unix timestamps; an unknown start time is `null`. Its field
names identify estimates and cumulative counters. JSON never contains quips.
