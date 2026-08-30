use super::request_agent;
use mbx_cache_core::{AgentRequest, AgentResponse};

/// Write to stderr without failing the build when the pipe is closed.
pub(crate) fn note(message: &str) {
    use std::io::Write as _;
    let _ = writeln!(std::io::stderr(), "{message}");
}

/// Whether this process's stderr belongs to the compiler it stands in for.
///
/// Set once at cc-shim entry. Build scripts read an intercepted compiler's
/// stderr as part of its answer -- cc-rs marks a probed flag unsupported the
/// moment anything lands there -- so one printed warning changes the flags of
/// every compilation the build script produces afterwards, and with them
/// every action key, orphaning the whole build's predictions.
static STDERR_RESERVED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Declare that this process replays compiler output on stderr and must not
/// mix its own diagnostics into it.
pub(crate) fn reserve_stderr_for_compiler() {
    STDERR_RESERVED.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Report a shim diagnostic without polluting a reserved stderr.
///
/// Delivered to the session's agent, which prints it from the process that
/// owns the build. When the agent cannot take it, the message falls back to
/// this process's stderr only where that stream is not the compiler's --
/// losing a diagnostic costs a little visibility, while poisoning a configure
/// probe costs the build its cache keys.
pub(crate) fn report_shim_warning(message: &str) {
    let mut message = message.replace(['\n', '\r'], "; ");
    // Stay under the agent's acceptance limit rather than losing the whole
    // diagnostic to it; the start of an error chain names the failure.
    if message.len() > 2048 {
        let end = (0..=2048).rfind(|&index| message.is_char_boundary(index));
        message.truncate(end.unwrap_or_default());
        message.push_str("...");
    }
    let delivered = matches!(
        request_agent(&[AgentRequest::RecordWarning {
            message: message.clone(),
        }])
        .map(|responses| responses.into_iter().next()),
        Ok(Some(AgentResponse::WarningRecorded))
    );
    if !delivered && !STDERR_RESERVED.load(std::sync::atomic::Ordering::Relaxed) {
        note(&format!("mbx[warning]: {message}"));
    }
}
