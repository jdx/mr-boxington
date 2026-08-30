use std::process::ExitCode;

fn main() -> ExitCode {
    // Cargo invokes the rustc shim thousands of times per build. Dispatch on
    // argv0 before any runtime, logging, or configuration setup.
    if mbx::session::is_rustc_shim() {
        return mbx::session::run_rustc_shim();
    }
    if mbx::session::is_rustdoc_shim() {
        return mbx::session::run_rustdoc_shim();
    }
    if let Some(language) = mbx::session::is_cc_shim() {
        return mbx::session::run_cc_shim(language);
    }

    // Top-level help and version terminate during argument parsing. Avoid
    // constructing the logger for those read-only paths: it cannot emit
    // anything before the parser exits, so the setup is unnecessary work.
    if !matches!(
        std::env::args_os().nth(1).as_deref(),
        Some(arg) if arg == "--help" || arg == "-h" || arg == "--version" || arg == "-V"
    ) {
        env_logger::Builder::from_env(env_logger::Env::default().filter_or("MBX_LOG", "info"))
            .format_target(false)
            .format_timestamp(None)
            .init();
    }

    match mbx::cli::run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("mbx[error]: {error:#}");
            ExitCode::FAILURE
        }
    }
}
