# Watching builds

`mbx tui` shows what every build on this machine is doing to the cache, as it
happens.

```sh
mbx tui
```

The end-of-build summary tells you what a build did once it is over, and
[cache results](/cache-results) explains how to read it. It cannot tell you what
a build is doing right now, and it cannot tell you which crate any of its
numbers belonged to. That is what this is for: one row per compilation, named,
with the outcome mbx chose for it.

Because mbx has no daemon, `mbx tui` is not talking to anything. Each build
appends its decisions to a stream under the cache, and the dashboard reads those
streams. It therefore shows builds in other terminals, builds that started
before you opened it, and any number of builds at once.

## Screens

**Live** lists the builds it knows about and, for the selected one, the
compilations as they are decided. Outcomes are colored: green for a hit, red for
a miss, grey for a compilation no lookup was possible for, yellow for one mbx
deliberately bypassed, cyan for a shadow verification.

The hit rate shown is over *attempted lookups*, the same as the summary's. A
cold build that looked nothing up shows `-` rather than `0%`, because those are
different facts.

**Sessions** lists finished builds with the totals each one ended with — the same
numbers `MBX_STATS_REPORT` would have written.

**Store** shows what the store holds and what mbx has saved on this machine
since it started counting.

A build's state is one of:

| State | Meaning |
| --- | --- |
| `live` | a build is running and still appending |
| `finished` | the build ended and recorded its totals |
| `abandoned` | the build died before it could record them |

`abandoned` is not an error mbx reports; it is what a stream looks like when the
process writing it is gone. A build killed mid-compile shows up this way rather
than appearing to run forever.

## Keys

| Key | Action |
| --- | --- |
| `q`, `Esc`, `Ctrl-C` | quit |
| `Tab` | next screen |
| `1`, `2`, `3` | jump to a screen |
| `j`, `k`, `↓`, `↑` | move |
| `p` | pause and resume reading |

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
directly to its stream — no buffering, so the dashboard is live — which costs
one small append against a compilation measured in milliseconds. Turn it off
with `events = false`, or `MBX_EVENTS=0`, and a build will record nothing and
leave nothing behind.

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
adding rows after 16 MiB — its totals are still recorded. A stream a build is
still writing is never collected. `mbx gc --dry-run` reports what it would drop
alongside everything else.

Streams are history, not cache content. Nothing keys on them, they are never
weighed against the store's size budget, and deleting them costs a row in a list.

::: warning Not a stable format
The event files are an implementation detail of `mbx tui` and may change in any
release. Scripts should read `MBX_STATS_REPORT`, which is
[versioned](/stability#json-output-is-versioned).
:::
