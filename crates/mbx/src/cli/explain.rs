use super::with_toolchain;
use crate::config::{CliSettings, Config};
use eyre::Result;
use std::process::ExitCode;

#[derive(usage::Args)]
#[usage(unknown_flags = "value")]
pub(super) struct ExplainArgs {
    /// Cargo subcommand to run under diagnostics.
    #[usage(value_name = "CARGO_COMMAND")]
    cargo_command: String,
    /// Arguments to pass to the Cargo subcommand.
    #[usage(double_dash = "preserve", value_name = "CARGO_ARGS")]
    cargo_args: Vec<String>,
}

impl ExplainArgs {
    pub(super) fn arguments(self) -> Vec<String> {
        std::iter::once(self.cargo_command)
            .chain(self.cargo_args)
            .collect()
    }
}

pub(super) fn run(
    config: &Config,
    settings: &CliSettings,
    args: ExplainArgs,
    toolchain: Option<&str>,
) -> Result<ExitCode> {
    let arguments = args.arguments();
    crate::explain::run_with_settings(config, settings, &with_toolchain(toolchain, arguments))
}
