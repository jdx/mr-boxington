---
description: Make Cargo and rust-analyzer use mbx, choose a setup scope, and verify shell activation.
---
# Cargo and editor setup {#set-up-cargo-and-your-editor}

To make plain `cargo` commands use mbx, enable
[native mise integration](#native-mise-integration) with mise 2026.9.2 or
newer, or run [standalone setup](#standalone-setup). Only `mbx setup`
configures [rust-analyzer](#rust-analyzer).

## Native mise integration

With mise 2026.9.2 or newer, turn on the Rust tool's `mr_boxington` option:

```sh
mise use --global --tool-option mr_boxington=true rust mr-boxington
```

Ordinary Cargo commands then run through mbx without `mbx setup`. Use
`mise exec -- cargo build`, `mise run` tasks, or a shell with mise activation
or shims on `PATH`. Rust and mbx remain independently versioned. Drop
`--global` for a project-scoped configuration.

## Share setup with a project

With mise 2026.9.2 or newer, commit the tool option in the project's
`mise.toml`:

```toml
[tools]
rust = { version = "stable", mr_boxington = true }
mr-boxington = "latest"
```

Keep the project's existing Rust version and other options when adding
`mr_boxington = true`. The `[tools]` table needs both `rust` and
`mr-boxington`. Run `mise install`, then use `mise exec -- cargo build`,
project tasks with `mise run`, or plain `cargo` with mise activation or shims
on `PATH`.

Set `mr_boxington = false` on the project's Rust entry to disable native
wrapping there. An explicit `[wrappers.cargo]` takes precedence over the
`mr_boxington` option, including `false`.

To migrate from `mbx setup`, remove the `[wrappers.cargo]` entry it added and
the `postinstall` hook that runs `mbx setup` from the mise config that holds
them, so the `mr_boxington` option controls wrapping. Then run `mise reshim`.

## Standalone setup

After [installing mbx](/installation), run `mbx setup` to configure
rust-analyzer and install the Cargo shim: a `cargo` command at a stable path
that runs mbx. `mbx setup --status` checks the result:

```sh
mbx setup
mbx setup --status
```

`mbx setup` can also add mise's Cargo wrapper to a mise config; see
[Choose a scope](#choose-a-scope). Use standalone setup with older mise
versions, without mise, or for applications that need an absolute path to the
Cargo shim. To try mbx without changing your setup, run `mbx build` directly.

## Choose a scope

| Command | Scope |
| --- | --- |
| `mbx setup` | Prompt for the recommended configuration |
| `mbx setup --yes` | Accept the recommendation |
| `mbx setup --global` | Global mise configuration |
| `mbx setup --local` | Current project's mise configuration |

`mbx setup` can add mise's Cargo wrapper, a `[wrappers.cargo]` entry that
requires mise 2026.8.16 or newer, to one mise config:

- When `MISE_CONFIG_FILE` is set, as it is during `mise use --postinstall`,
  `mbx setup --yes` uses that config. `mbx setup --status` and `--uninstall`
  use it too unless you pass `--global` or `--local`.
- `--global` and `--local` select their config even when mise is not activated
  in the current shell.
- Otherwise, `mbx setup` integrates with mise only when mise is activated in
  the current shell. It recommends the config that defines `mr-boxington`, then
  the nearest project config, and finally the global config. In an interactive
  terminal, `mbx setup` prompts before using that recommendation. Without one,
  as when a script or coding agent runs it, `mbx setup` uses no mise config.
  `--yes` accepts the recommendation without prompting.

After adding or removing the wrapper, `mbx setup` runs `mise reshim`. When it
uses no mise config, it prints the exact shell-specific `PATH` change the first
time it creates the shim. It never edits a shell startup file.

## Verify plain Cargo

Open a new shell after setup and verify that Cargo resolves to mise's Cargo
wrapper, a mise shim, or the Cargo shim, depending on how you enabled wrapping.
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

mise activation supplies the command-wrapper path to shells where mise is
active. The wrapper invokes `mbx` with Cargo shim mode enabled, then mise
removes its dispatch directories before mbx delegates to the configured or
system Cargo. Calling `~/.cargo/bin/cargo` directly skips mise, so Cargo runs
without mbx.

### Desktop applications and non-interactive commands

For coding agents, SSH commands, and other non-interactive tools, use
`mise exec -- cargo build` or put mise's shims directory on their `PATH`.
When using shims, `command -v cargo` resolves to the mise shim rather than the
command-wrapper directory; both honor the `mr_boxington` option.

Desktop applications such as Codex may inherit a different `PATH` from an
interactive terminal. Check `command -v cargo` from a command run by the
application itself. Configure its command environment to include mise's shims,
or use `mise exec -- cargo build` for build commands. Restart the application
after changing its inherited environment.

To use the Cargo shim instead, run `mbx setup` and prepend the shim's directory
to the `PATH` those processes use. With native mise integration, choose
**Create the shim without activating it** if `mbx setup` prompts, and do not
pass `--yes`, `--global`, or `--local`; see [rust-analyzer](#rust-analyzer).
When `mbx setup` activates a mise config, it names that directory on every run.
Without a mise config, only the first run prints a ready-made `PATH` line.
Other runs print only the shim file, as in
`refreshed the Cargo shim at …/mbx/bin/cargo`; prepend that file's parent
directory, not the file.

For zsh, add the `export PATH` line to `~/.zshenv`, which applies to
interactive and non-interactive shells. For example, with the default data
directory on Linux:

```sh
export PATH="$HOME/.local/share/mbx/bin:$PATH"
```

On macOS, the default is different:

```sh
export PATH="$HOME/Library/Application Support/mbx/bin:$PATH"
```

If your data directory is customized, use the shim directory that `mbx setup`
reports instead.

Start a new process and run `command -v cargo` again. Once it resolves to the
shim, ordinary `cargo build`, `cargo test`, and `cargo check` commands use mbx
automatically. Prefixing a Cargo command with `mbx`, as in `mbx build`, still
works.

Outside mise activation, the Cargo shim finds mbx this way:

- On Unix, it runs the mbx that `mbx setup` recorded while that file exists,
  then resolves the active `mbx` from `PATH` after an upgrade removes it.
- On Windows, `cargo.exe` resolves the active mbx from `PATH`, with the
  recorded mbx as a fallback.

On Unix, once mise prunes the mbx version that `mbx setup` recorded, as it does
after `mise upgrade`, the Cargo shim works only where `mbx` is on `PATH`.
`mbx doctor` and `mbx setup --status` then report the shim as outdated. Run
`mbx setup` again after such an upgrade so editors and other processes without
mise's `PATH` keep working.

## rust-analyzer

Run `mbx setup` to send rust-analyzer's background check through mbx; the
native mise option does not configure editor checks. `mbx setup` sets
`check.overrideCommand` in rust-analyzer's user configuration file and prints
the file's path, whichever mise scope activation uses. If that file already has
check settings, `mbx setup` leaves them unchanged.

For the TOML configuration that `mbx setup` uses, the override belongs in that
user file. A `rust-analyzer.toml` beside `Cargo.toml` does not apply the
override to the editor's Cargo process.

With native mise integration, run `mbx setup` for rust-analyzer only:

- If `mbx setup` asks `Where should plain cargo commands use mbx?`, choose
  **Create the shim without activating it**, and skip any `PATH` change it
  prints.
- Do not pass `--yes`, `--global`, or `--local`. Selecting a mise scope adds
  mise's Cargo wrapper, which takes precedence over the `mr_boxington` option.
- Check the override with `MISE_SHELL` unset, as in
  `env -u MISE_SHELL mbx setup --status`. In a shell with mise activation, a
  plain `mbx setup --status` reports
  `mbx setup is installed but is not active in the selected mise config` and
  stops before checking rust-analyzer.

The editor invokes the stable Cargo shim by its absolute path, so its build
shares mbx's cache and machine-wide compiler pool even when the editor did not
inherit mise's `PATH`. Its outputs go to `target/rust-analyzer`, separate from
terminal builds, so the two Cargo processes do not contend on the
target-directory lock. When `target` is a
[managed target directory](/managed-targets), `target/rust-analyzer` lives
inside it and is collected with it, while the shared store warms both builds.

After `mbx setup` writes the override, rust-analyzer shows this message when it
loads the user file:

```text
invalid config value:
check/overrideCommand: unexpected field
;
```

The check command still applies. Because rust-analyzer validates its user file
against global and local settings only, it flags `check.overrideCommand`, a
workspace setting, before reading it. `mbx setup --status` confirms that the
override is the one `mbx setup` wrote. The upstream discussion of the
validation is in
[rust-lang/rust-analyzer#23381](https://github.com/rust-lang/rust-analyzer/pull/23381).

::: info Upgrading from 1.12.0 or earlier
Releases through 1.12.0 wrote the override beside `Cargo.toml` when mbx was
activated in a project mise scope, where rust-analyzer never read it. Run
`mbx setup` in that project to remove the setting; it reports the file it
cleaned. `mbx setup --uninstall` there also removes the setting.
:::

## Remove automatic wrapping

For native mise integration, remove `mr_boxington` or set it to `false` in each
Rust tool entry where you enabled it, then run `mise reshim`. Keep the Rust
version and any unrelated tool options. Do this before removing `mr-boxington`
from `[tools]`.

If you also ran `mbx setup`:

```sh
mbx setup --uninstall
```

Use `--global` or `--local` to select a mise scope explicitly. Repeat for each
scope you enabled, and remove any mbx `PATH` entry you added manually. Open a
new shell and check Cargo's path again.

One user-level rust-analyzer override serves every scope. Uninstalling a
project scope leaves the override in place for the others. Uninstalling the
global scope, or running `mbx setup --uninstall` without an active mise shell,
removes it.

`mbx setup --uninstall` never removes the Cargo shim or the mbx executable, and
it does not clear the cache. The rust-analyzer override calls the shim by its
absolute path, so delete the shim's directory only after you uninstall every
scope, remove the override, and remove any mbx `PATH` entry.
`mbx setup --global --uninstall` removes the override even if you enabled only
project scopes. The shim's directory is the parent of the path that
`mbx setup --uninstall` prints.

## Shell completions

Generate a completion script for `bash`, `zsh`, `fish`, `powershell` (or
`pwsh`), `nu` (or `nushell`), or `elvish`:

```sh
mbx completion zsh > _mbx
```

Install the generated file in your shell's completion directory. The script
is self-contained. See the [completion reference](/cli/completion).

For watch loops, debugger paths, and laptop budgets, continue to
[Local development](/cookbook/local-development).
