//! Hold machine-wide scheduler capacity while an external command runs.

use super::cargo::exit_code;
use crate::config::{Config, SchedulerPriority};
use bytesize::ByteSize;
use eyre::{Result, WrapErr};
use std::process::{Command, ExitCode};

#[derive(usage_rs::Args)]
pub(super) struct ReserveArgs {
    /// CPU cores to hold while the command runs.
    #[usage(long, value_name = "N")]
    cpus: Option<u64>,
    /// Memory to hold from the configured scheduler budget.
    #[usage(long, value_name = "SIZE")]
    memory: Option<ByteSize>,
    /// Whether this reservation yields capacity to normal-priority builds.
    #[usage(long, default = "normal")]
    priority: SchedulerPriority,
    /// Command and arguments to run while capacity is reserved.
    #[usage(value_name = "COMMAND", required = true, double_dash = "automatic")]
    pub(super) command: Vec<String>,
}

pub(super) fn run(config: &Config, args: ReserveArgs) -> Result<ExitCode> {
    if std::env::var_os("MBX_RESERVE_WORKER").is_some() {
        return run_direct(config, args);
    }

    let mut worker = Command::new(std::env::current_exe()?);
    worker.arg("reserve");
    if let Some(cpus) = args.cpus {
        worker.args(["--cpus", &cpus.to_string()]);
    }
    if let Some(memory) = args.memory {
        worker.args(["--memory", &format!("{}B", memory.as_u64())]);
    }
    worker.args(["--priority", args.priority.as_str(), "--"]);
    worker.args(&args.command);
    worker.env("MBX_RESERVE_WORKER", "1");
    Ok(exit_code(
        worker.status().wrap_err("failed to start reserve worker")?,
    ))
}

fn run_direct(config: &Config, args: ReserveArgs) -> Result<ExitCode> {
    let Some((program, arguments)) = args.command.split_first() else {
        eyre::bail!("reserve needs a command to run")
    };
    let Some(pool) = crate::scheduler::reservation_pool(config, args.priority) else {
        eyre::bail!("reserve needs scheduler.enabled")
    };
    let (permit, admission) =
        pool.reserve_observed(args.cpus, args.memory.map(|memory| memory.as_u64()))?;
    // A terminal's ^C or hangup, or a signal sent to the whole process group,
    // reaches this worker as well as the command. The command handles it
    // itself, so the worker only has to outlive it; dying first would release
    // the lease while the command is still shutting down.
    let guard = HoldThroughSignals::install();
    if let Some(signal) = guard.pending() {
        // Signalled after admission and before the command exists: there is
        // nothing to wait for, so end the way the signal would have.
        drop(permit);
        guard.end_by(signal);
    }
    let mut command = external_command(program, arguments, args.cpus);
    // A signal during startup reaches the new process too, so it checks for one
    // itself before exec and refuses to start the command.
    guard.refuse_start_if_signalled(&mut command);
    let status = match command.spawn() {
        Ok(mut child) => child.wait().map_err(eyre::Report::from),
        Err(error) => match guard.pending() {
            Some(signal) if error.kind() == std::io::ErrorKind::Interrupted => {
                drop(permit);
                guard.end_by(signal);
            }
            _ => Err(eyre::Report::from(error)),
        },
    }
    .wrap_err_with(|| format!("failed to run {program}"));
    drop(guard);
    drop(permit);
    pool.observe_reservation(admission, status.as_ref().ok());
    let status = status?;
    Ok(exit_code(status))
}

pub(super) fn external_command(program: &str, arguments: &[String], cpus: Option<u64>) -> Command {
    let mut command = Command::new(program);
    command
        .args(arguments)
        // Nested mbx work is covered by this reservation. Letting its compiler
        // shims take new permits would deadlock when this lease fills the pool.
        .env(crate::scheduler::SCHED_DIR_ENV, "")
        .env(crate::scheduler::SCHED_DISABLE_ENV, "1")
        // The private worker marker applies only to this reservation. A nested
        // `mbx reserve` must start its own worker so its lease follows its
        // command even when this external command launches it in the background.
        .env_remove("MBX_RESERVE_WORKER");
    // Tell the command how many cores it holds so a runner can size its own
    // workers from the lease. A reservation that holds no cores clears the
    // variable rather than passing on an enclosing reservation's value.
    match cpus.filter(|cpus| *cpus > 0) {
        Some(cpus) => command.env("MBX_RESERVED_CPUS", cpus.to_string()),
        None => command.env_remove("MBX_RESERVED_CPUS"),
    };
    command
}

/// Keeps the worker alive through HUP, INT, QUIT and TERM while the command
/// runs, so the lease it holds lasts until the command has exited.
///
/// The handler only records the signal. It is a handler and not `SIG_IGN`
/// because exec resets handlers, so the command keeps default dispositions,
/// and nothing is blocked in the worker, because a blocked mask would be
/// inherited. A signal
/// that was already ignored (`nohup`, a background job's INT) is left alone so
/// it stays ignored in the command too.
#[cfg(unix)]
struct HoldThroughSignals {
    previous: Vec<(libc::c_int, libc::sigaction)>,
}

#[cfg(unix)]
static RECEIVED_SIGNAL: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

#[cfg(unix)]
extern "C" fn record_signal(signal: libc::c_int) {
    RECEIVED_SIGNAL.store(signal, std::sync::atomic::Ordering::SeqCst);
}

#[cfg(unix)]
impl HoldThroughSignals {
    const SIGNALS: [libc::c_int; 4] = [libc::SIGHUP, libc::SIGINT, libc::SIGQUIT, libc::SIGTERM];

    fn install() -> Self {
        RECEIVED_SIGNAL.store(0, std::sync::atomic::Ordering::SeqCst);
        let mut previous = Vec::new();
        for signal in Self::SIGNALS {
            // SAFETY: plain sigaction calls on zeroed structs; the handler
            // only stores to an atomic, which is async-signal-safe.
            unsafe {
                let mut old: libc::sigaction = std::mem::zeroed();
                if libc::sigaction(signal, std::ptr::null(), &mut old) != 0
                    || old.sa_sigaction == libc::SIG_IGN
                {
                    continue;
                }
                let mut new: libc::sigaction = std::mem::zeroed();
                new.sa_sigaction =
                    record_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
                libc::sigemptyset(&mut new.sa_mask);
                if libc::sigaction(signal, &new, std::ptr::null_mut()) == 0 {
                    previous.push((signal, old));
                }
            }
        }
        Self { previous }
    }

    fn set() -> libc::sigset_t {
        // SAFETY: builds a signal set in zeroed storage.
        unsafe {
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            for signal in Self::SIGNALS {
                libc::sigaddset(&mut set, signal);
            }
            set
        }
    }

    /// A signal the handler has recorded.
    fn pending(&self) -> Option<libc::c_int> {
        match RECEIVED_SIGNAL.load(std::sync::atomic::Ordering::SeqCst) {
            0 => None,
            signal => Some(signal),
        }
    }

    /// Make the new process, between fork and exec, check for a signal that
    /// arrived while it was being created and fail the launch if there was one.
    ///
    /// A group signal sent after the fork reaches the new process, which still
    /// has the recording handler. It blocks the signals, looks at both what the
    /// handler recorded and what is pending, and only then puts back the
    /// original dispositions and mask, so a signal that lands afterwards takes
    /// the command's own default action. The worker never replays a signal, so
    /// the command cannot receive one twice.
    fn refuse_start_if_signalled(&self, command: &mut Command) {
        use std::os::unix::process::CommandExt as _;
        let previous = self.previous.clone();
        let set = Self::set();
        // SAFETY: the closure runs between fork and exec and only calls
        // async-signal-safe functions on data it already owns.
        unsafe {
            command.pre_exec(move || {
                let mut original: libc::sigset_t = std::mem::zeroed();
                libc::pthread_sigmask(libc::SIG_BLOCK, &set, &mut original);
                let mut pending: libc::sigset_t = std::mem::zeroed();
                libc::sigpending(&mut pending);
                let arrived = RECEIVED_SIGNAL.load(std::sync::atomic::Ordering::SeqCst) != 0
                    || Self::SIGNALS
                        .into_iter()
                        .any(|signal| libc::sigismember(&pending, signal) == 1);
                if arrived {
                    return Err(std::io::ErrorKind::Interrupted.into());
                }
                for (signal, old) in &previous {
                    libc::sigaction(*signal, old, std::ptr::null_mut());
                }
                libc::pthread_sigmask(libc::SIG_SETMASK, &original, std::ptr::null_mut());
                Ok(())
            });
        }
    }

    fn restore(&mut self) {
        for (signal, old) in self.previous.drain(..) {
            // SAFETY: reinstalls the disposition install replaced.
            unsafe { libc::sigaction(signal, &old, std::ptr::null_mut()) };
        }
    }

    /// End the process by `signal` with its default action.
    fn end_by(mut self, signal: libc::c_int) -> ! {
        self.restore();
        // SAFETY: resets one signal to its default and raises it. Unblocking
        // covers a signal that was blocked when mbx started.
        unsafe {
            libc::signal(signal, libc::SIG_DFL);
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            libc::sigaddset(&mut set, signal);
            libc::pthread_sigmask(libc::SIG_UNBLOCK, &set, std::ptr::null_mut());
            libc::raise(signal);
        }
        std::process::exit(128 + signal)
    }
}

#[cfg(unix)]
impl Drop for HoldThroughSignals {
    fn drop(&mut self) {
        self.restore();
    }
}

#[cfg(not(unix))]
struct HoldThroughSignals;

#[cfg(not(unix))]
impl Drop for HoldThroughSignals {
    /// Matches the Unix guard, whose drop restores the signal dispositions.
    fn drop(&mut self) {}
}

#[cfg(not(unix))]
impl HoldThroughSignals {
    fn install() -> Self {
        Self
    }

    fn refuse_start_if_signalled(&self, _command: &mut Command) {}

    fn pending(&self) -> Option<i32> {
        None
    }

    fn end_by(self, _signal: i32) -> ! {
        unreachable!("no signal is ever pending")
    }
}
