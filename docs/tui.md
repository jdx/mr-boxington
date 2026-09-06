# Watching builds

`mbx tui` shows what every build on this machine is doing to the cache, as it
happens.

```sh
mbx tui
```

[![Live dashboard showing build activity, cache traffic, capacity, and compilation savings](/screenshots/tui-live.png)](/screenshots/tui-live.png)

*Live, with example build data. Open any screenshot at full size to read the details.*

The end-of-build summary tells you what a build did once it is over, and
[cache results](/cache-results) explains how to read it. It cannot tell you what
a build is doing right now, or which crate any of its numbers belonged to.
`mbx tui` shows one row per compilation, named, with the outcome mbx chose for
it.

mbx has no daemon, so `mbx tui` is not talking to anything. Each build appends
its decisions to a stream under the cache, and the dashboard reads those
streams. It shows builds in other terminals, builds that started before you
opened it, and any number of builds at once.

## Screens

**Live** lists the builds it knows about and, for the selected one, the
compilations as they are decided. Outcomes are colored: green for a hit, red for
a miss, grey for a compilation no lookup was possible for, yellow for one mbx
bypassed, cyan for a shadow verification that matched, and magenta for one that
diverged.

The selected build stays visible as you move through the list. The activity
panel follows the newest compilations by default. Use `Page Up` and `Page Down`
to read older activity, and `End` to return to the latest. New events keep
arriving while you read history, without shifting the rows you are inspecting.
Narrow terminals show fewer build columns; the selected build's counts also
appear above its activity.

On wider terminals, Live adds a sidebar with lookup traffic, the selected
build's cache-hit gauge, and its biggest compilation savings. The hit and miss
graphs share a scale and cover the last five minutes observed by the TUI;
idle intervals stay empty. The header groups compiler time saved, store
capacity, and evictions into separate metric panels.

The hit rate shown is over *attempted lookups*, the same as the summary's. A
cold build that looked nothing up shows `-`, not `0%`, because those are
different facts.

**Sessions** lists finished builds with the totals each one ended with, the
same numbers `MBX_STATS_REPORT` would have written.

**Store** shows what the store holds and what mbx has saved on this machine
since it started counting, with a date and human-readable compiler time.
It also shows pruning totals, reflinked bytes, and estimated workspace sharing;
[`mbx stats`](/stats) explains the figures and prints them without opening the TUI.
Use `↑`/`↓` or `Page Up`/`Page Down` to scroll the Store screen.
On larger terminals, inventory, lifetime savings, and workspace sharing each
get a panel. Reports that need more room use the scrollable layout.

[![Store dashboard showing lifetime savings, automatic pruning, and estimated workspace sharing](/screenshots/tui-store.png)](/screenshots/tui-store.png)

*Store, with example build data. Sharing and compiler savings are estimates.*

**Insights** explains the build selected in Live. It shows:

- An outcome chart over all recorded actions, alongside the lookup-only hit
  rate. Bypassed and unconsulted work are visible without lowering that rate.
- A strip of recent outcomes, ordered oldest to newest. Each letter represents
  one action, not a fixed interval of time. On wider terminals a duration graph
  below the strip shows which of those actions took the longest.
- Bypass reasons ranked by frequency, with each reason's share of bypasses.
- The slowest recorded actions, with their outcomes. A hit's duration measures
  restoring outputs; other durations measure compiler work.
- The biggest cache wins, ranked by estimated compiler time avoided, plus
  total estimated compiler time saved and bytes restored for the build.

[![Insights dashboard showing outcome bars, action durations, bypass reasons, and the biggest cache wins](/screenshots/tui-insights.png)](/screenshots/tui-insights.png)

*Insights, with example build data. Action durations measure recorded work, not elapsed build time.*

Use `j`/`k` or `↑`/`↓` to inspect another build without leaving Insights,
and `Page Up`/`Page Down` to scroll. Rankings cover the most recent 2,000 retained
actions. Charts count recorded actions even after they leave that window; if
the stream hits its recording cap, Insights marks its data as incomplete.
Compiler time saved is cumulative work avoided, not elapsed wall-clock time:
compilations can run in parallel.

With enough room, Insights arranges its charts and rankings in a dashboard.
`Page Down` opens the full detail view; `Page Up` returns to the dashboard.

A build's state is one of:

| State | Meaning |
| --- | --- |
| `live` | a build is running and still appending |
| `finished` | the build ended and recorded its totals |
| `abandoned` | the build died before it could record them |

`abandoned` is not an error mbx reports; it is what a stream looks like when the
process writing it is gone. A build killed mid-compile shows up this way instead
of appearing to run forever.

## Store pressure

The header shows store bytes against the configured `gc.max_size` budget with
an inline usage bar. It turns yellow at 90% and red at or above 100%; a store
can temporarily exceed its budget between sweeps. This is the action-store
budget, not free disk space. A configured combined target/store budget can
reduce the actual allowance further. Automatic collection being off is shown
explicitly.

Evicted bytes are shown for the last five minutes of observation and for the
lifetime of the savings ledger. These count cache-store collection, including
explicit `mbx gc`, and exclude target-directory cleanup. The recent counter
starts when the TUI opens; it does not treat old ledger totals as new evictions.
Resuming after a pause starts a new observation period.

A **POSSIBLE CACHE THRASHING** banner appears on every screen when all of these
hold within the last five minutes:

- The eviction counter increased in at least three separate observations.
- Evicted bytes total at least 25% of the configured store budget.
- At least 20 new lookups were observed, with a miss rate of 50% or more.

The banner alternates red and yellow once per second while keeping its text
visible. The Store screen gives the evidence and suggests inspecting misses
and the cache budget. This is a warning heuristic, not proof that specific
misses were caused by eviction. A single cleanup, a cold build without
repeated eviction, and a reset ledger do not trigger it. It clears as the
observations leave the five-minute window. Counter observations are not an
exact count of GC runs; several runs may fall between reads.

## Keys

| Key | Action |
| --- | --- |
| `q`, `Esc`, `Ctrl-C` | quit |
| `Tab`, `→` | next screen |
| `Shift-Tab`, `←` | previous screen |
| `1`, `2`, `3`, `4` | jump to Live, Sessions, Store, Insights |
| `j`, `k`, `↓`, `↑` | select a build in Live or Insights; scroll Sessions or Store |
| `Page Up`, `Page Down` | browse activity history in Live; scroll Insights or Store |
| `End` | follow the latest activity in Live |
| `p` | pause and resume reading |

Click a tab to switch screens. Click a build in Live to select it, or a row in
Sessions to open that build in Live. The mouse wheel selects builds over the
Live build list, browses history over Activity, and scrolls the Sessions,
Store, and Insights reports. Scrolling up in Activity shows older events;
scrolling down returns toward the latest events.

## Without a terminal

`mbx tui --once` prints one plain-text snapshot and exits, for a pipe, a CI log,
or a quick look that does not take over the terminal.

```sh
mbx tui --once
```

```text
store: /home/you/.cache/mbx/actions
objects: 44 (8.6 MiB); action results: 7 (2.5 KiB)

command                             state         hit   miss  unconsulted  bypass
mbx check --workspace               live            0      0            4       3
mbx build                           finished        3      0            0       3
mbx build                           finished        0      0            3       3
```

## Recording

Recording is on by default. A build appends one short line per compilation
directly to its stream, with no buffering, so the dashboard is live. The cost
is one small append against a compilation measured in milliseconds. Turn it
off with `events = false`, or `MBX_EVENTS=0`, and a build records nothing and
leaves nothing behind.

Streams live beside the rest of the store's bookkeeping:

```text
<store>/sessions/v1/<session>.jsonl   one line per decision
<store>/sessions/v1/<session>.lock    held for as long as the build runs
```

The lock is how a stream says it is still being written. Whoever can take it is
looking at a build that has ended, however it ended, because the operating
system releases the lock with the process either way.

Collection bounds them without any configuration: a stream is dropped once it is
a week old or once it is not among the newest 256, and a single build stops
adding rows after 16 MiB, though its totals are still recorded. A stream a build
is still writing is never collected. `mbx gc --dry-run` reports what it would
drop alongside everything else.

Streams are history, not cache content. Nothing keys on them, they are never
weighed against the store's size budget, and deleting them costs a row in a list.

::: warning Not a stable format
The event files are an implementation detail of `mbx tui` and may change in any
release. Scripts should read `MBX_STATS_REPORT`, which is
[versioned](/stability#json-output-is-versioned).
:::
