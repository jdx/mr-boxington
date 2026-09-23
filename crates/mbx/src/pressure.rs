//! Advisory, pool-wide pressure sampling. Call `sample` under the registrar lock.
//! Missing probes and stale state fail open; no sample is an OS memory limit.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub(crate) const INTERVAL_MS: u64 = 500;
const STALE_MS: u64 = 3_000;
const VERSION: u8 = 1;

/// The newest state this process sampled but could not save. Without it, a
/// failing write would re-probe on every admission poll and reset hysteresis.
static UNSAVED: Mutex<Option<(PathBuf, State)>> = Mutex::new(None);

#[derive(Clone, Default, Serialize, Deserialize)]
pub(crate) struct Reading {
    pub total: Option<u64>,
    pub available: Option<u64>,
    /// Full-stall microseconds, keyed by the source (never sum nested groups).
    pub stalls: BTreeMap<String, u64>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub(crate) struct State {
    version: u8,
    pub sampled_ms: u64,
    reading: Reading,
    bad_samples: u8,
    healthy_since: Option<u64>,
    pub pressured: bool,
    pub valid: bool,
    pub recovered_ms: Option<u64>,
    pub last_admission_ms: Option<u64>,
}

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |time| time.as_millis().min(u128::from(u64::MAX)) as u64)
}

pub(crate) fn probe() -> Reading {
    #[allow(unused_mut)]
    let mut reading = Reading {
        total: crate::util::memory_total_bytes(),
        available: crate::util::memory_available_bytes(),
        ..Reading::default()
    };
    #[cfg(target_os = "linux")]
    for path in std::iter::once(std::path::PathBuf::from("/proc/pressure/memory"))
        .chain(crate::cgroup::pressure_files())
    {
        if let Some(total) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| full_total(&s))
        {
            reading
                .stalls
                .insert(path.to_string_lossy().into_owned(), total);
        }
    }
    reading
}

#[cfg(any(target_os = "linux", test))]
fn full_total(text: &str) -> Option<u64> {
    text.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        (fields.next()? == "full").then_some(())?;
        fields.find_map(|field| field.strip_prefix("total=")?.parse().ok())
    })
}

impl State {
    fn update(&mut self, now: u64, reading: Reading) {
        let elapsed = now
            .checked_sub(self.sampled_ms)
            .filter(|n| *n > 0 && *n <= STALE_MS);
        let comparable = self.version == VERSION && elapsed.is_some();
        let stall_percent = comparable
            .then(|| {
                reading
                    .stalls
                    .iter()
                    .filter_map(|(source, value)| {
                        let delta = value.checked_sub(*self.reading.stalls.get(source)?)?;
                        Some(delta as f64 / (elapsed.unwrap() as f64 * 10.0))
                    })
                    .reduce(f64::max)
            })
            .flatten();
        let headroom = reading
            .total
            .filter(|n| *n > 0)
            .zip(reading.available)
            .map(|(total, available)| available as f64 / total as f64);
        if !comparable {
            *self = Self::default();
        }
        self.valid = headroom.is_some() || stall_percent.is_some();
        let bad = headroom.is_some_and(|v| v < 0.05) || stall_percent.is_some_and(|v| v >= 10.0);
        let healthy = self.valid
            && headroom.is_none_or(|v| v > 0.10)
            && stall_percent.is_none_or(|v| v < 2.0);
        if !self.valid {
            self.pressured = false;
            self.bad_samples = 0;
            self.healthy_since = None;
            self.recovered_ms = None;
        } else if bad {
            self.bad_samples = self.bad_samples.saturating_add(1);
            self.healthy_since = None;
            if self.bad_samples >= 2 {
                self.pressured = true;
                self.recovered_ms = None;
            }
        } else {
            self.bad_samples = 0;
            if self.pressured && healthy {
                let start = *self.healthy_since.get_or_insert(now);
                if now.saturating_sub(start) >= 5_000 {
                    self.pressured = false;
                    self.recovered_ms = Some(now);
                    self.healthy_since = None;
                }
            } else {
                self.healthy_since = None;
            }
        }
        self.version = VERSION;
        self.sampled_ms = now;
        self.reading = reading;
    }
}

/// The caller holds the pool registrar lock across sampling and any state write.
pub(crate) fn sample(dir: &Path, now: u64, probe: fn() -> Reading) -> eyre::Result<State> {
    let mut state: State = std::fs::read(dir.join("pressure.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let mut unsaved = UNSAVED.lock().unwrap_or_else(|error| error.into_inner());
    if let Some((path, kept)) = unsaved.as_ref()
        && path == dir
        && (state.version != VERSION || kept.sampled_ms > state.sampled_ms)
    {
        state = kept.clone();
    }
    if state.version == VERSION
        && now
            .checked_sub(state.sampled_ms)
            .is_some_and(|age| age < INTERVAL_MS)
    {
        return Ok(state);
    }
    state.update(now, probe());
    let saved = save(dir, &state);
    if saved.is_err() {
        *unsaved = Some((dir.to_path_buf(), state.clone()));
    } else if unsaved.as_ref().is_some_and(|(path, _)| path == dir) {
        *unsaved = None;
    }
    saved.map(|()| state)
}

pub(crate) fn save(dir: &Path, state: &State) -> eyre::Result<()> {
    crate::util::write_advisory(&dir.join("pressure.json"), &serde_json::to_vec(state)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn memory(available: u64) -> Reading {
        Reading {
            total: Some(100),
            available: Some(available),
            ..Reading::default()
        }
    }
    #[test]
    fn hysteresis_and_stale_recovery() {
        let mut state = State::default();
        state.update(1_000, memory(4));
        assert!(!state.pressured);
        state.update(1_500, memory(4));
        assert!(state.pressured);
        for now in (2_000..7_000).step_by(500) {
            state.update(now, memory(11));
            assert!(state.pressured);
        }
        state.update(7_000, memory(11));
        assert!(!state.pressured);
        assert_eq!(state.recovered_ms, Some(7_000));
        state.update(7_500, memory(0));
        state.update(8_000, memory(0));
        assert!(state.pressured);
        state.update(12_000, memory(0));
        assert!(
            !state.pressured,
            "stale observations cannot count as consecutive"
        );
        state.update(11_000, Reading::default());
        assert!(!state.valid);
    }
    #[test]
    fn psi_uses_deltas_and_discards_reset_counters() {
        fn psi(total: u64) -> Reading {
            Reading {
                stalls: BTreeMap::from([("host".into(), total)]),
                ..Reading::default()
            }
        }
        let mut state = State::default();
        state.update(1_000, psi(100));
        assert!(!state.valid);
        state.update(1_500, psi(50_100));
        state.update(2_000, psi(100_100));
        assert!(state.pressured);
        state.update(2_500, psi(0));
        assert!(!state.valid);
        assert!(!state.pressured);
        assert_eq!(
            full_total("some total=55\nfull avg10=0.0 total=42"),
            Some(42)
        );
        assert_eq!(full_total("some total=55"), None);
    }
    #[test]
    fn sampling_is_shared_and_rate_limited() {
        let dir = tempfile::tempdir().unwrap();
        sample(dir.path(), 1_000, || memory(0)).unwrap();
        sample(dir.path(), 1_100, || panic!("too soon")).unwrap();
        let state = sample(dir.path(), 1_500, || memory(0)).unwrap();
        assert!(state.pressured);
        let state = sample(dir.path(), 1_600, || panic!("shared state")).unwrap();
        assert!(state.pressured);
    }
    #[test]
    fn failed_saves_keep_rate_limit_and_hysteresis() {
        let dir = tempfile::tempdir().unwrap();
        // A directory in place of the file makes every atomic write fail.
        std::fs::create_dir(dir.path().join("pressure.json")).unwrap();
        assert!(sample(dir.path(), 1_000, || memory(0)).is_err());
        let state = sample(dir.path(), 1_100, || panic!("too soon")).unwrap();
        assert_eq!(state.sampled_ms, 1_000);
        assert!(sample(dir.path(), 1_500, || memory(0)).is_err());
        let state = sample(dir.path(), 1_600, || panic!("too soon")).unwrap();
        assert!(state.pressured);
    }
}
