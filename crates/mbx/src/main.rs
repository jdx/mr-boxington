use std::process::ExitCode;

fn main() -> ExitCode {
    // Cargo invokes the rustc shim thousands of times per build. Dispatch on
    // argv0 before any runtime, logging, or configuration setup.
    if mbx::session::is_rustc_shim() {
        return mbx::session::run_rustc_shim();
    }

    eprintln!("mbx: the command line is not implemented yet; see PLAN.md");
    ExitCode::from(1)
}
