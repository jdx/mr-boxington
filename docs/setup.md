---
description: Make Cargo and rust-analyzer use mbx, choose a setup scope, and verify shell activation.
---
# Set up Cargo and your editor

Run this after [installing mbx](/installation) to make ordinary `cargo`
commands use the cache:

```sh
mbx setup
mbx setup --status
```

Setup installs a stable Cargo shim and offers mise and rust-analyzer
integration. To try mbx without changing your setup, run `mbx build` directly.

## Choose a scope

| Command | Scope |
| --- | --- |
| `mbx setup` | Prompt for the recommended configuration |
| `mbx setup --yes` | Accept the recommendation |
| `mbx setup --global` | Global mise configuration |
| `mbx setup --local` | Current project's mise configuration |

During `mise use --postinstall`, `mbx setup --yes` adds a `[wrappers.cargo]`
entry to the configuration named by `MISE_CONFIG_FILE`. Otherwise, setup only integrates with mise when
mise is activated in the current shell. It recommends the config that defines
`mr-boxington`, then the nearest project config, and finally the global config.
`mbx setup` prompts before using that recommendation; `--yes` accepts it.
Without an active mise shell, setup prints the exact shell-specific `PATH`
change and never edits a shell startup file. Setup runs `mise reshim` after
adding or removing the wrapper.

Automatic mise wrapping requires mise 2026.8.16 or newer.

## Share setup with a project

To keep the wrapper project-scoped and reviewable, commit it directly in the
project's `mise.toml` instead:

```toml
[tools]
mr-boxington = "1"

[wrappers.cargo]
command = "mbx"
env = { MBX_CARGO_SHIM_MODE = "1" }
```

Then run `mise install`. This installs mbx but does not activate the wrapper in
the parent shell. Run project tasks with `mise run`, or activate mise in the
shell before invoking plain `cargo` commands. This avoids changing the global
mise configuration, and every contributor who activates the project gets the
same Cargo wrapper.

## Verify plain Cargo

Open a new shell after setup and verify that Cargo resolves to mise's command
wrapper or the stable mbx shim, depending on how you activated it. On Unix:

```sh
command -v cargo
# ~/.local/share/mise/command-wrappers/bin/cargo on Linux
```

On Windows, use PowerShell or `where.exe`:

```powershell
(Get-Command cargo).Path
where.exe cargo
```

mise activation supplies the command-wrapper path to shells where mise is active.
The wrapper invokes `mbx` with Cargo shim mode enabled, then mise removes its
dispatch directories before mbx delegates to the configured or system Cargo.
SSH commands, coding agents, editors, and other non-interactive tools may use a
startup path that does not activate mise. In that case, prepend the directory
printed by `mbx setup` in a startup file those processes read. For zsh,
`~/.zshenv` applies to interactive and non-interactive shells. For example,
the default Linux location is:

```sh
export PATH="$HOME/.local/share/mbx/bin:$PATH"
```

On macOS, the default is different:

```sh
export PATH="$HOME/Library/Application Support/mbx/bin:$PATH"
```

Use the path setup prints if your data directory is customized.

Start a new process and run `command -v cargo` again. Once it resolves to the
shim, ordinary `cargo build`, `cargo test`, and `cargo check` commands use mbx
automatically. Prefixing a Cargo command with `mbx`, as in `mbx build`, still
works.

Outside mise activation, the stable `cargo` launcher uses the setup-time mbx
while it exists, then resolves the active `mbx` from `PATH` after an upgrade
removes that path. Windows `cargo.exe` resolves the active mbx from `PATH`, with
the setup-time path as a fallback. Upgrading a mise-managed mbx does not require
running setup again.

## rust-analyzer

Setup also configures rust-analyzer's background check in the matching global
or project scope. The editor invokes the stable Cargo shim by its absolute path,
so its build shares mbx's cache and machine-wide compiler pool even when the
editor did not inherit mise's `PATH`. Its outputs go to
`target/rust-analyzer`, separate from terminal builds so the two Cargo processes
do not contend on the target-directory lock. When `target` is managed, the
editor directory lives inside that view and is collected with it, while the
shared store warms both builds. Existing rust-analyzer check settings are left
unchanged.

## Remove automatic wrapping

```sh
mbx setup --uninstall
```

Use `--global` or `--local` to select a mise scope explicitly. Repeat for each
scope you enabled, and remove any mbx `PATH` entry you added manually. Open a
new shell and check Cargo's path again. Uninstalling activation does not remove
the mbx executable or clear the cache.

## Shell completions

Generate a completion script for `bash`, `zsh`, `fish`, or `powershell`:

```sh
mbx completion zsh > _mbx
```

Install the generated file in your shell's completion directory. The script
is self-contained. See the [completion reference](/cli/completion).

For watch loops, debugger paths, and laptop budgets, continue to
[Local development](/cookbook/local-development).
