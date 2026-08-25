//! What mbx has saved on this machine, accumulated across builds.
//!
//! A single build's statistics say what just happened; they cannot say what
//! switching to mbx has been worth, because the compilations it skipped and the
//! directories it collected are spread over months. This module keeps a running
//! total beside the store and turns it into one line after a build.
//!
//! ```text
//! <store>/savings/v1/tally.json   the totals
//! <store>/savings/v1/tally.lock   held while they are updated
//! ```
//!
//! The tally is bookkeeping, not cache content: it is never an input to a
//! cache key, and the collector never walks it. Losing it costs a number in a
//! message, so every failure here is logged and forgotten rather than raised.

use crate::config::SavingsStyle;
use crate::util::{format_duration, write_atomic};
use bytesize::ByteSize;
use eyre::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub(crate) const TALLY_FILE: &str = "savings/v1/tally.json";
const TALLY_LOCK: &str = "savings/v1/tally.lock";
const TALLY_VERSION: u8 = 1;

/// Running totals for one machine.
///
/// `serde(default)` rather than `deny_unknown_fields`: a tally written by a
/// newer mbx must degrade to a smaller number, never to an error that costs a
/// build its savings line.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Tally {
    pub version: u8,
    /// When counting started, so a total can say what it spans.
    pub since_secs: u64,
    pub builds: u64,
    pub cached_compilations: u64,
    pub avoided_compiler_ns: u64,
    pub reflinked_bytes: u64,
    pub freed_target_bytes: u64,
    pub freed_store_bytes: u64,
    /// Bytes the user asked to have removed: a confirmed `target/` migration,
    /// or `mbx cache remove`. Kept apart from the collection counters because
    /// every line about those brags that nobody had to do anything -- which is
    /// exactly untrue of a removal somebody confirmed.
    pub freed_requested_bytes: u64,
    /// Counters a newer mbx wrote that this one does not know about.
    ///
    /// Every record rewrites the whole file, so without this an older binary
    /// would silently drop a newer one's totals while leaving its version
    /// number in place. Carried through untouched instead.
    #[serde(flatten)]
    unknown: BTreeMap<String, serde_json::Value>,
}

/// What one command contributed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Delta {
    pub builds: u64,
    pub cached_compilations: u64,
    pub avoided_compiler_ns: u64,
    pub reflinked_bytes: u64,
    pub freed_target_bytes: u64,
    pub freed_store_bytes: u64,
    pub freed_requested_bytes: u64,
}

impl Delta {
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// What this one command did, for the lines that describe the run rather than
/// the lifetime.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct SessionFacts {
    pub hits: u64,
    pub avoided_compiler_ns: u64,
}

fn tally_path(store: &Path) -> PathBuf {
    store.join(TALLY_FILE)
}

/// Add `delta` to the stored totals and return the result.
///
/// Runs after cargo has exited, so the lock it takes and the two small files it
/// touches are not on anything the user is waiting for. Concurrent commands
/// serialize on the lock the same way sweeps do.
pub(crate) fn record(store: &Path, delta: &Delta) -> Result<Tally> {
    let lock_path = store.join(TALLY_LOCK);
    std::fs::create_dir_all(
        lock_path
            .parent()
            .expect("the tally lock path has a parent"),
    )
    .wrap_err_with(|| format!("failed to create {}", lock_path.display()))?;
    let mut lock = fslock::LockFile::open(&lock_path)?;
    lock.lock()?;

    let path = tally_path(store);
    let mut tally = read(&path);
    if tally.version == 0 {
        tally.version = TALLY_VERSION;
        tally.since_secs = now_secs();
    }
    tally.builds = tally.builds.saturating_add(delta.builds);
    tally.cached_compilations = tally
        .cached_compilations
        .saturating_add(delta.cached_compilations);
    tally.avoided_compiler_ns = tally
        .avoided_compiler_ns
        .saturating_add(delta.avoided_compiler_ns);
    tally.reflinked_bytes = tally.reflinked_bytes.saturating_add(delta.reflinked_bytes);
    tally.freed_target_bytes = tally
        .freed_target_bytes
        .saturating_add(delta.freed_target_bytes);
    tally.freed_store_bytes = tally
        .freed_store_bytes
        .saturating_add(delta.freed_store_bytes);
    tally.freed_requested_bytes = tally
        .freed_requested_bytes
        .saturating_add(delta.freed_requested_bytes);

    let mut contents = serde_json::to_vec_pretty(&tally)?;
    contents.push(b'\n');
    write_atomic(&path, &contents)?;
    Ok(tally)
}

/// Read the tally, treating anything unreadable as a fresh start.
///
/// A tally that cannot be parsed has already lost its history, and refusing to
/// count from here would lose every future build's as well.
fn read(path: &Path) -> Tally {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default()
}

/// Thresholds below which a number is not worth a line of the user's attention.
const MIN_SESSION_HITS: u64 = 5;
const MIN_SESSION_AVOIDED: Duration = Duration::from_secs(30);
const MIN_LIFETIME_AVOIDED: Duration = Duration::from_secs(30 * 60);
const MIN_FREED_BYTES: u64 = 1024 * 1024 * 1024;
const MIN_FREED_TARGET_BYTES: u64 = 5 * 1024 * 1024 * 1024;
const MIN_REFLINKED_BYTES: u64 = 1024 * 1024 * 1024;
const MIN_CACHED_COMPILATIONS: u64 = 250;

/// One `format!` per line, with the locals in scope: a pool of tellings reads
/// as copy to be edited, not as code, so every telling gets exactly one line.
macro_rules! tellings {
    ($($line:literal),+ $(,)?) => { vec![$(format!($line)),+] };
}

/// One fact worth telling, in every voice it can be told in.
struct Candidate {
    /// The mascot's variants: a cache box in a monocle, deadpan, mentioning
    /// the chores exactly once. One is chosen at random per build.
    cheeky: Vec<String>,
    /// The same fact in the register of the `cache:` and `gc:` lines.
    plain: String,
}

/// The facts this machine has earned the right to mention.
///
/// Every line carries a number this machine actually produced -- a line that
/// could have been written before the build ran is an advertisement, not a
/// report. Thresholds keep small numbers quiet.
fn facts_worth_telling(tally: &Tally, facts: &SessionFacts) -> Vec<Candidate> {
    let freed = tally
        .freed_target_bytes
        .saturating_add(tally.freed_store_bytes);
    let mut eligible = Vec::new();

    if facts.hits >= MIN_SESSION_HITS
        && facts.avoided_compiler_ns >= duration_ns(MIN_SESSION_AVOIDED)
    {
        let hits = facts.hits;
        let dur = nanos(facts.avoided_compiler_ns);
        eligible.push(Candidate {
            cheeky: tellings![
                "mbx: served {hits} compilations from cache; rustc showed up ready to do {dur} of work and was sent home",
                "mbx: {hits} compilations you did not wait for. the {dur} is yours now. spend it wisely.",
                "mbx: this build borrowed {hits} compilations from an earlier one. rustc was not informed. ({dur} saved)",
                "mbx: {hits} compilations arrived precompiled. somewhere a fan is not spinning. ({dur} saved)",
                "mbx: cargo asked for {hits} compilations; mbx had them filed under already-done. ({dur} saved)",
                "mbx: {hits} compilations replayed from cache. rustc's services were not required. ({dur} saved)",
                "mbx: intercepted {hits} compilations before rustc could get attached. ({dur} saved)",
                "mbx: rustc had {dur} of work planned. mbx had {hits} of its answers already.",
                "mbx: {hits} compilations answered from memory. rustc can finish its coffee. ({dur} saved)",
                "mbx: rustc arrived to find {hits} compilations already done. awkward. ({dur} saved)",
                "mbx: quietly substituted {hits} cached compilations. nobody noticed. that is the job. ({dur} saved)",
                "mbx: {hits} compilations in, zero compiled. {dur} returned to circulation.",
            ],
            plain: format!(
                "savings: {hits} compilations restored from cache this build, avoiding {dur} of compiler time"
            ),
        });
    }
    if tally.avoided_compiler_ns >= duration_ns(MIN_LIFETIME_AVOIDED) {
        let dur = nanos(tally.avoided_compiler_ns);
        let over = builds(tally.builds);
        eligible.push(Candidate {
            cheeky: tellings![
                "mbx: {dur} of compiling skipped across {over}. rustc suspects nothing.",
                "mbx: {dur} refunded over {over}. no receipt necessary.",
                "mbx: rustc believes it compiled everything. it is down {dur} across {over}. let it believe.",
                "mbx: has saved {dur} across {over}. this line is the only thanks it needs.",
                "mbx: across {over}, rustc has been excused from {dur} of its duties.",
                "mbx: {dur} not spent compiling, over {over}. what you did instead is between you and the terminal.",
                "mbx: the running total stands at {dur} across {over}. nobody is counting except the box.",
                "mbx: {dur} of compilation avoided across {over}. imagine the fan noise. now stop.",
                "mbx: {dur} saved across {over}. put it toward something with a progress bar.",
                "mbx: {dur} of rustc's calendar cleared across {over}.",
                "mbx: {dur} unspent across {over}. compounding, in a sense.",
            ],
            plain: format!("savings: {dur} of compiler time avoided across {over}"),
        });
    }
    if freed >= MIN_FREED_BYTES {
        let size = iec(freed);
        eligible.push(Candidate {
            cheeky: tellings![
                "mbx: {size} of build debris binned so far. cargo clean remains unemployed.",
                "mbx: has quietly disposed of {size} of build leftovers. nobody saw anything.",
                "mbx: {size} reclaimed to date. the disk sends its regards.",
                "mbx: {size} of build debris has left the building.",
                "mbx: swept up {size} so far. the broom is content-addressed.",
                "mbx: {size} tidied away. you may continue not thinking about it.",
                "mbx: {size} reclaimed while you were busy compiling other things.",
                "mbx: {size} of old build output has been shown the door.",
                "mbx: disposed of {size} without being asked. that is rather the point.",
                "mbx: the sweep found {size} nobody would miss. nobody has.",
            ],
            plain: format!("savings: {size} reclaimed by collection so far"),
        });
    }
    if tally.freed_target_bytes >= MIN_FREED_TARGET_BYTES {
        let size = iec(tally.freed_target_bytes);
        eligible.push(Candidate {
            cheeky: tellings![
                "mbx: {size} of target/ had outlived its checkouts. it has been dealt with.",
                "mbx: found {size} of target/ whose checkouts left long ago. the estate has been settled.",
                "mbx: your deleted worktrees left {size} behind. left. past tense.",
                "mbx: {size} of target/ belonged to checkouts that no longer exist. neither does it.",
                "mbx: cleaned up {size} after checkouts that left without saying goodbye.",
                "mbx: {size} of outputs had survived their checkouts. an unnatural state, now corrected.",
                "mbx: the checkouts are gone. their {size} has gone to join them.",
                "mbx: {size} of target/ was waiting for checkouts that are never coming back. it has stopped waiting.",
                "mbx: escorted {size} of ownerless outputs off the premises.",
                "mbx: worktrees come and go. their {size} now goes with them.",
            ],
            plain: format!("savings: {size} of abandoned target directories reclaimed"),
        });
    }
    if tally.reflinked_bytes >= MIN_REFLINKED_BYTES {
        let size = iec(tally.reflinked_bytes);
        eligible.push(Candidate {
            cheeky: tellings![
                "mbx: every checkout believes it owns {size} of outputs. the disk keeps one copy and says nothing.",
                "mbx: {size} of outputs, one copy, several very confident checkouts.",
                "mbx: reflinked {size} into place. copying is for people with disk to spare.",
                "mbx: {size} of outputs exist once and appear everywhere. do not tell the checkouts.",
                "mbx: {size} of target/ is an elaborate illusion. the disk is in on it.",
                "mbx: lent the same {size} to every checkout at once. none of them has checked.",
                "mbx: {size} shared among checkouts that each believe they are an only child.",
                "mbx: {size} on loan to every checkout simultaneously. the paperwork is an inode.",
                "mbx: {size} of outputs materialized by reflink. the copy machine stays off.",
                "mbx: {size} in every checkout, {size} on disk. arithmetic declined to comment.",
            ],
            plain: format!("savings: {size} of outputs reflinked rather than copied"),
        });
    }
    if tally.cached_compilations >= MIN_CACHED_COMPILATIONS {
        let count = tally.cached_compilations;
        eligible.push(Candidate {
            cheeky: tellings![
                "mbx: {count} compilations, each compiled once and served warm ever since",
                "mbx: {count} cache hits and counting. rustc thinks it has been busy.",
                "mbx: {count} compilations on file. the box remembers.",
                "mbx: {count} compilations old. not one compiled twice.",
                "mbx: {count} compilations served. the monocle stays on.",
                "mbx: {count} compilations dispensed from stock. inventory immaculate.",
                "mbx: {count} compilations, and the box has forgotten none of them.",
                "mbx: {count} compilations retrieved with a straight face.",
                "mbx: {count} compilations. rustc did each once; mbx did the rest of the showing up.",
                "mbx: {count} compilations served. ask rustc how many it remembers doing.",
            ],
            plain: format!("savings: {count} compilations served from cache to date"),
        });
    }
    eligible
}

/// One line about what mbx has saved, or nothing worth saying yet.
///
/// Which fact appears -- and, in the default style, which telling of it -- is
/// random. A machine that qualifies for several facts should not recite them
/// in a fixed order, and a joke wears out at exactly the speed it repeats.
/// `pick` is injected so tests can reach every line; callers use [`quip`].
fn quip_choosing(
    tally: &Tally,
    facts: &SessionFacts,
    style: SavingsStyle,
    mut pick: impl FnMut(usize) -> usize,
) -> Option<String> {
    if style == SavingsStyle::Off {
        return None;
    }
    let mut eligible = facts_worth_telling(tally, facts);
    if eligible.is_empty() {
        return None;
    }
    let index = pick(eligible.len()) % eligible.len();
    let candidate = eligible.swap_remove(index);
    match style {
        SavingsStyle::Quips => {
            let mut variants = candidate.cheeky;
            let index = pick(variants.len()) % variants.len();
            Some(variants.swap_remove(index))
        }
        SavingsStyle::Plain => Some(candidate.plain),
        SavingsStyle::Off => unreachable!("handled above"),
    }
}

pub(crate) fn quip(tally: &Tally, facts: &SessionFacts, style: SavingsStyle) -> Option<String> {
    use rand::RngExt as _;
    quip_choosing(tally, facts, style, |len| rand::rng().random_range(0..len))
}

fn iec(bytes: u64) -> String {
    ByteSize::b(bytes).display().iec().to_string()
}

/// A duration the way a person would say it: "6h 14m", "2m 51s", "45s".
///
/// [`format_duration`] is for measurements and renders six hours as
/// `22440.00s`, which is no way to tell somebody good news. Two units carry
/// all the precision a brag needs.
fn nanos(nanoseconds: u64) -> String {
    let total = Duration::from_nanos(nanoseconds).as_secs();
    if total == 0 {
        return format_duration(Duration::from_nanos(nanoseconds));
    }
    let units = [(86_400, "d"), (3_600, "h"), (60, "m"), (1, "s")];
    // The two largest units that are actually present. A zero unit in between
    // must not spend a slot: "1d 0h" would hide real minutes behind it and
    // understate the number this line exists to state.
    let mut parts = Vec::new();
    let mut rest = total;
    for (size, suffix) in units {
        let amount = rest / size;
        rest %= size;
        if amount > 0 {
            parts.push(format!("{amount}{suffix}"));
            if parts.len() == 2 {
                break;
            }
        }
    }
    parts.join(" ")
}

/// "1 build" is not "1 builds", and the one machine that hits it would be the
/// machine of someone deciding whether to keep mbx.
fn builds(count: u64) -> String {
    if count == 1 {
        "1 build".to_string()
    } else {
        format!("{count} builds")
    }
}

fn duration_ns(duration: Duration) -> u64 {
    crate::util::duration_ns(duration)
}

/// Record `delta` without letting a bookkeeping failure reach the build.
///
/// Callers are past the point where anything can be done about an error: cargo
/// has exited and its status is the answer the user came for.
pub(crate) fn record_quietly(store: &Path, delta: &Delta) -> Option<Tally> {
    match record(store, delta) {
        Ok(tally) => Some(tally),
        Err(error) => {
            log::debug!("savings were not recorded: {error}");
            None
        }
    }
}

/// Record and, when there is something worth saying, return the line to say.
pub(crate) fn record_and_describe(
    store: &Path,
    delta: &Delta,
    facts: &SessionFacts,
    style: SavingsStyle,
) -> Option<String> {
    // A run that neither compiled nor collected anything moves no counter, so
    // it does not write -- not even to start the file. A sweep that came due
    // during `cargo build --help` did free bytes, and that delta is not empty,
    // so those are still counted.
    if delta.is_empty() && facts.hits == 0 {
        return None;
    }
    let tally = record_quietly(store, delta)?;
    quip(&tally, facts, style)
}

#[cfg(test)]
#[path = "savings_tests.rs"]
mod tests;
