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
    let mut guard = HoldThroughSignals::install();
    // The signals stay blocked from here until the command exists, so one that
    // arrives during startup is held pending instead of falling in the gap
    // between the check and the spawn.
    guard.block();
    if let Some(signal) = guard.pending() {
        // Signalled after admission and before the command exists: there is
        // nothing to wait for, so end the way the signal would have.
        drop(permit);
        guard.end_by(signal);
    }
    let mut command = external_command(program, arguments, args.cpus);
    guard.unblock_in_child(&mut command);
    let status = command
        .spawn()
        .map_err(eyre::Report::from)
        .and_then(|mut child| {
            // A signal that stayed pending reached this worker, and may have
            // reached the group before the command existed. Give the command
            // the signal itself; a repeat of one it already got is harmless.
            guard.forward_pending(child.id());
            guard.unblock();
            child.wait().map_err(eyre::Report::from)
        })
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
/// and the signals are blocked only until the command exists, where it gets
/// the mask the worker started with, because a blocked mask would be inherited. A signal
/// that was already ignored (`nohup`, a background job's INT) is left alone so
/// it stays ignored in the command too.
#[cfg(unix)]
struct HoldThroughSignals {
    previous: Vec<(libc::c_int, libc::sigaction)>,
    /// The mask in force before [`Self::block`], restored in the command.
    original_mask: Option<libc::sigset_t>,
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
        Self {
            previous,
            original_mask: None,
        }
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

    /// Hold the signals pending until [`Self::unblock`], so none is lost
    /// between the last check and the command's creation.
    fn block(&mut self) {
        let set = Self::set();
        // SAFETY: blocks the signals and saves the previous mask.
        unsafe {
            let mut old: libc::sigset_t = std::mem::zeroed();
            if libc::pthread_sigmask(libc::SIG_BLOCK, &set, &mut old) == 0 {
                self.original_mask = Some(old);
            }
        }
    }

    /// Give the command the mask the worker started with, not the blocked one.
    fn unblock_in_child(&self, command: &mut Command) {
        use std::os::unix::process::CommandExt as _;
        if let Some(mask) = self.original_mask {
            // SAFETY: pthread_sigmask is async-signal-safe and the closure
            // allocates nothing.
            unsafe {
                command.pre_exec(move || {
                    libc::pthread_sigmask(libc::SIG_SETMASK, &mask, std::ptr::null_mut());
                    Ok(())
                });
            }
        }
    }

    fn unblock(&mut self) {
        if let Some(mask) = self.original_mask.take() {
            // SAFETY: restores the mask block saved.
            unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, &mask, std::ptr::null_mut()) };
        }
    }

    /// A signal recorded by the handler, or held pending while blocked.
    fn pending(&self) -> Option<libc::c_int> {
        match RECEIVED_SIGNAL.load(std::sync::atomic::Ordering::SeqCst) {
            0 => self.blocked_pending().into_iter().next(),
            signal => Some(signal),
        }
    }

    fn blocked_pending(&self) -> Vec<libc::c_int> {
        // SAFETY: reads the calling thread's pending set into zeroed storage.
        unsafe {
            let mut set: libc::sigset_t = std::mem::zeroed();
            if libc::sigpending(&mut set) != 0 {
                return Vec::new();
            }
            Self::SIGNALS
                .into_iter()
                .filter(|signal| libc::sigismember(&set, *signal) == 1)
                .collect()
        }
    }

    /// Send the command any signal that arrived while it was being created.
    fn forward_pending(&self, child: u32) {
        for signal in self.blocked_pending() {
            // SAFETY: signals the command this worker just started.
            unsafe { libc::kill(child as libc::pid_t, signal) };
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
        self.original_mask = None;
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
impl HoldThroughSignals {
    fn install() -> Self {
        Self
    }

    fn block(&mut self) {}

    fn unblock_in_child(&self, _command: &mut Command) {}

    fn unblock(&mut self) {}

    fn forward_pending(&self, _child: u32) {}

    fn pending(&self) -> Option<i32> {
        None
    }

    fn end_by(self, _signal: i32) -> ! {
        unreachable!("no signal is ever pending")
    }
}
