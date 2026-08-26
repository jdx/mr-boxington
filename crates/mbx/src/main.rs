use std::process::ExitCode;

fn main() -> ExitCode {
    // Cargo invokes the rustc shim thousands of times per build. Dispatch on
    // argv0 before any runtime, logging, or configuration setup.
    if mbx::session::is_rustc_shim() {
        return mbx::session::run_rustc_shim();
    }

    env_logger::Builder::from_env(env_logger::Env::default().filter_or("MBX_LOG", "info"))
        .format_target(false)
        .format_timestamp(None)
        .init();

    match mbx::cli::run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("mbx[error]: {error:#}");
            ExitCode::FAILURE
        }
    }
}
