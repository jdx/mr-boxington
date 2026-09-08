---
description: Make Cargo and rust-analyzer use mbx, choose a setup scope, and verify shell activation.
---
# Set up Cargo and your editor

## Native mise integration

With mise 2026.9.2 or newer:

```sh
mise use --global --tool-option mr_boxington=true rust mr-boxington
```

This enables ordinary Cargo commands through mise without running `mbx setup`.
Use `mise exec -- cargo build`, `mise run` tasks, or a shell with mise
activation or shims on `PATH`. Rust and mbx remain independently versioned.
Drop `--global` for a project-scoped configuration.

## Standalone setup

After [installing mbx](/installation), run this to install a stable Cargo shim
and configure rust-analyzer:

```sh
mbx setup
mbx setup --status
```

Setup offers explicit mise wrapper and rust-analyzer integration. It is useful
for older mise versions or applications that need an absolute Cargo shim path.
To try mbx without changing your setup, run `mbx build` directly.

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

The explicit wrapper written by `mbx setup` requires mise 2026.8.16 or newer.

## Share setup with a project

With mise 2026.9.2 or newer, commit the tool option in the project's `mise.toml`:

```toml
[tools]
rust = { version = "stable", mr_boxington = true }
mr-boxington = "latest"
```

Keep the project's existing Rust version and other options when adding
`mr_boxington = true`. Both tools must be configured. Run `mise install`, then
use `mise exec -- cargo build`, project tasks with `mise run`, or plain `cargo`
with mise activation or shims on `PATH`.

Set `mr_boxington = false` on the project's Rust entry to disable native
wrapping there. An explicit `[wrappers.cargo]` takes precedence over the Rust
option, including `false`. When migrating from `mbx setup`, remove the old mbx
`[wrappers.cargo]` entry and its `postinstall` hook from the applicable config
so the Rust option controls wrapping. Run `mise reshim` after migrating.

## Verify plain Cargo

Open a new shell after setup and verify that Cargo resolves to mise's command
wrapper, a mise shim, or the stable mbx shim, depending on how you activated it.
On Unix:

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
Calling `~/.cargo/bin/cargo` directly bypasses mise's integration.

### Desktop applications and non-interactive commands

For coding agents, SSH commands, and other non-interactive tools, use
`mise exec -- cargo build` or put mise's shims directory on their `PATH`.
When using shims, `command -v cargo` resolves to the mise shim rather than the
command-wrapper directory; both honor the Rust option.

Desktop applications such as Codex may inherit a different `PATH` from an
interactive terminal. Check `command -v cargo` from a command run by the
application itself. Configure its command environment to include mise's shims,
or use `mise exec -- cargo build` for build commands. Restart the application
after changing its inherited environment.

If you prefer mbx's standalone launcher, run `mbx setup` and prepend the
directory it prints to the environment those processes use. For zsh,
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

The native mise option does not configure editor checks. Run `mbx setup` to
configure rust-analyzer's background check in the matching global
or project scope. The editor invokes the stable Cargo shim by its absolute path,
so its build shares mbx's cache and machine-wide compiler pool even when the
editor did not inherit mise's `PATH`. Its outputs go to
`target/rust-analyzer`, separate from terminal builds so the two Cargo processes
do not contend on the target-directory lock. When `target` is managed, the
editor directory lives inside that view and is collected with it, while the
shared store warms both builds. Existing rust-analyzer check settings are left
unchanged.

## Remove automatic wrapping

For native mise integration, remove `mr_boxington` or set it to `false` in each
Rust tool entry where you enabled it, then run `mise reshim`. Keep the Rust
version and any unrelated tool options. Do this before removing mr-boxington
from `[tools]`.

If you ran standalone setup as well:

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
