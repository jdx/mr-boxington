# CLI reference

## Cargo subcommands

Run any Cargo subcommand with the build cache enabled.

```text
mbx <CARGO_SUBCOMMAND> [ARGS]...
```

Examples:

```sh
mbx build --release
mbx test --workspace
mbx clippy --all-targets -- -D warnings
```

The Cargo subcommand and all of its arguments are passed through unchanged.
Cargo aliases and installed subcommands work too. `cache` and `gc` are reserved
for mbx's own store-management commands.

## `mbx cache dir`

Print the action-store directory.

```sh
mbx cache dir
```

## `mbx cache stats`

Summarize cached objects, action results, and their sizes.

```sh
mbx cache stats
```

## `mbx gc`

Collect stale managed targets and evict cached objects until the store fits its
budget.

```text
mbx gc [--max-size <SIZE>]
```

Without `--max-size`, the command uses `gc.max_size` / `MBX_GC_MAX_SIZE`.

```sh
mbx gc
mbx gc --max-size 3GB
mbx gc --max-size 20GiB
```

Eviction is safe: a missing cached object is rebuilt when it is needed again.
