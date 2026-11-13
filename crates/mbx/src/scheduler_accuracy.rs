//! Best-effort observations; rotation deliberately accepts concurrent lost lines.
use super::*;
use std::io::{BufRead, BufReader, Write};

const FILE: &str = "observations.jsonl";
const PREVIOUS: &str = "observations.jsonl.1";
const LOCK: &str = "observations.lock";
const MAX_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Clone, Copy)]
pub(crate) struct Admission {
    pub weight: u64,
    pub resources: Resources,
    pub predicted: Option<u64>,
    pub source: &'static str,
    pub wait_ms: u64,
    pub started: Instant,
}

#[derive(Debug, Serialize, Deserialize)]
struct Observation {
    v: u8,
    ts_ms: u64,
    mbx_version: String,
    pressure_domain: String,
    kind: String,
    name: String,
    key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    build_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    build_capacity: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    domain_capacity: Option<u64>,
    source: String,
    predicted_bytes: Option<u64>,
    charged_permits: u64,
    #[serde(default)]
    charged_cpu: Option<u64>,
    #[serde(default)]
    charged_memory_bytes: Option<u64>,
    bytes_per_permit: u64,
    peak_bytes: Option<u64>,
    outcome: String,
    wrapped: bool,
    admission_wait_ms: u64,
    run_ms: u64,
}

impl Pool {
    pub(super) fn observe(&self, demand: &Demand, status: &ExitStatus, peak: Option<u64>) {
        // Take once: repeated completion calls and unscheduled runs cannot log.
        let admission = demand.observation.take();
        if !self.events || self.bytes_per_permit == 0 {
            return;
        }
        let Some(admission) = admission else { return };
        let kind = if demand.per_test.is_some() {
            "nextest-test"
        } else if demand.learns_cpu {
            "test-binary"
        } else if demand.links {
            "link"
        } else {
            "compile"
        };
        let key = demand.compile_mode.clone().unwrap_or_else(|| {
            // Per-test history is keyed by binary and test; the demand name
            // represents that identity, rather than the suite ledger key.
            ledger_key(&demand.name, demand.links)
        });
        let observation = Observation {
            v: 1,
            ts_ms: (self.clock)(),
            mbx_version: env!("CARGO_PKG_VERSION").into(),
            pressure_domain: self.pressure_domain.into(),
            kind: kind.into(),
            name: demand.name.clone(),
            key,
            build_id: self.build.as_ref().map(|(id, _)| id.clone()),
            build_capacity: self.build.as_ref().map(|(_, capacity)| *capacity),
            domain_capacity: Some(self.domain_capacity),
            source: admission.source.into(),
            predicted_bytes: admission.predicted,
            charged_permits: admission.weight,
            charged_cpu: Some(admission.resources.cpu),
            charged_memory_bytes: Some(admission.resources.memory_bytes),
            bytes_per_permit: self.bytes_per_permit,
            peak_bytes: peak,
            outcome: outcome(status),
            wrapped: !demand.count_compile_samples,
            admission_wait_ms: admission.wait_ms,
            run_ms: admission
                .started
                .elapsed()
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX),
        };
        if let Err(error) = append(&self.dir, observation) {
            debug!("scheduler observation was not recorded: {error:#}");
        }
    }

    pub(crate) fn reserve_observed(
        &self,
        cpus: Option<u64>,
        memory_bytes: Option<u64>,
    ) -> Result<(super::Permit, Admission)> {
        let weight = self.reservation_weight(cpus, memory_bytes)?;
        let requested_at = Instant::now();
        let permit = self.reserve(cpus, memory_bytes)?;
        let wait_ms = requested_at
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX);
        Ok((
            permit,
            Admission {
                weight,
                resources: Resources {
                    cpu: cpus.unwrap_or(0),
                    memory_bytes: memory_bytes.unwrap_or(0),
                },
                predicted: None,
                source: "reservation",
                wait_ms,
                started: Instant::now(),
            },
        ))
    }

    pub(crate) fn observe_reservation(&self, admission: Admission, status: Option<&ExitStatus>) {
        if !self.events || self.bytes_per_permit == 0 {
            return;
        }
        let observation = Observation {
            v: 1,
            ts_ms: (self.clock)(),
            mbx_version: env!("CARGO_PKG_VERSION").into(),
            pressure_domain: self.pressure_domain.into(),
            kind: "reservation".into(),
            name: "mbx reserve".into(),
            key: "reservation".into(),
            build_id: self.build.as_ref().map(|(id, _)| id.clone()),
            build_capacity: self.build.as_ref().map(|(_, capacity)| *capacity),
            domain_capacity: Some(self.domain_capacity),
            source: admission.source.into(),
            predicted_bytes: None,
            charged_permits: admission.weight,
            charged_cpu: Some(admission.resources.cpu),
            charged_memory_bytes: Some(admission.resources.memory_bytes),
            bytes_per_permit: self.bytes_per_permit,
            peak_bytes: None,
            outcome: status.map_or_else(|| "unknown".into(), outcome),
            wrapped: false,
            admission_wait_ms: admission.wait_ms,
            run_ms: admission
                .started
                .elapsed()
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX),
        };
        if let Err(error) = append(&self.dir, observation) {
            debug!("scheduler reservation observation was not recorded: {error:#}");
        }
    }
}

fn outcome(status: &ExitStatus) -> String {
    if oom_killed(status) {
        return "oom-killed".into();
    }
    if status.success() {
        return "success".into();
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;
        if let Some(signal) = status.signal() {
            return format!("signal-{signal}");
        }
    }
    "failed".into()
}

/// Bound escaped JSON strings too. A hash suffix preserves distinct long keys.
fn compact(value: &mut String) -> Result<()> {
    compact_to(value, 128)
}

pub(super) fn compact_to(value: &mut String, limit: usize) -> Result<()> {
    if serde_json::to_vec(value.as_str())?.len() <= limit {
        return Ok(());
    }
    let suffix = format!("…{:016x}", test_history_hash(value));
    let mut end = value.len().min(limit);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    let prefix = &value[..end];
    let mut shortened = format!("{prefix}{suffix}");
    while serde_json::to_vec(&shortened)?.len() > limit {
        end -= 1;
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        shortened = format!("{}{suffix}", &value[..end]);
    }
    *value = shortened;
    Ok(())
}

fn append(dir: &Path, mut observation: Observation) -> Result<()> {
    // Shorten every long identity the same way on every line, whatever the
    // rest of the line holds, so one key is never split between spellings.
    compact(&mut observation.name)?;
    compact(&mut observation.key)?;
    compact(&mut observation.pressure_domain)?;
    if let Some(build_id) = &mut observation.build_id {
        compact(build_id)?;
    }
    let mut line = serde_json::to_vec(&observation)?;
    line.push(b'\n');
    eyre::ensure!(line.len() < 1024, "observation exceeds line size limit");
    append_line(dir, FILE, PREVIOUS, LOCK, &line)
}

/// Shared best-effort rotation and single O_APPEND write for scheduler logs.
pub(super) fn append_line(
    dir: &Path,
    filename: &str,
    previous: &str,
    lockname: &str,
    line: &[u8],
) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(filename);
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_BYTES) {
        let mut lock = fslock::LockFile::open(&dir.join(lockname))?;
        if lock.try_lock()? && std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_BYTES) {
            // Replaces the previous generation. A writer that already opened
            // the old inode can lose its line if another rotation overtakes it.
            std::fs::rename(&path, dir.join(previous))?;
        }
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    write_line(&mut file, line)
}

fn write_line(file: &mut impl Write, line: &[u8]) -> Result<()> {
    if file.write(line)? != line.len() {
        // Do not retry the record. This newline is a separate append, so a
        // concurrent writer's line can still land before it and be lost.
        let _ = file.write(b"\n");
        eyre::bail!("short scheduler event write");
    }
    Ok(())
}

#[derive(Debug, Default, Serialize)]
pub(super) struct Counts {
    n: u64,
    success: u64,
    failed: u64,
    oom_killed: u64,
    other: u64,
    measured: u64,
    exceeded: u64,
    success_exceeded: u64,
    failed_exceeded: u64,
    oom_killed_exceeded: u64,
    other_exceeded: u64,
    worst_peak_reserved_ratio: Option<f64>,
    worst_ts_ms: Option<u64>,
    worst_key: Option<String>,
    worst_outcome: Option<String>,
    highest_peak_bytes: Option<u64>,
}

impl Counts {
    fn add(&mut self, o: &Observation) {
        self.n += 1;
        match o.outcome.as_str() {
            "success" => self.success += 1,
            "failed" => self.failed += 1,
            "oom-killed" => self.oom_killed += 1,
            _ => self.other += 1,
        }
        let Some(peak) = o.peak_bytes else { return };
        self.measured += 1;
        self.highest_peak_bytes = Some(self.highest_peak_bytes.unwrap_or(0).max(peak));
        let reserved = o
            .charged_memory_bytes
            .unwrap_or_else(|| o.charged_permits.saturating_mul(o.bytes_per_permit));
        if reserved == 0 {
            return;
        }
        if peak > reserved {
            self.exceeded += 1;
            match o.outcome.as_str() {
                "success" => self.success_exceeded += 1,
                "failed" => self.failed_exceeded += 1,
                "oom-killed" => self.oom_killed_exceeded += 1,
                _ => self.other_exceeded += 1,
            }
        }
        let ratio = peak as f64 / reserved as f64;
        if self
            .worst_peak_reserved_ratio
            .is_none_or(|worst| ratio > worst)
        {
            self.worst_peak_reserved_ratio = Some(ratio);
            self.worst_ts_ms = Some(o.ts_ms);
            self.worst_key = Some(o.key.clone());
            self.worst_outcome = Some(o.outcome.clone());
        }
    }
}

#[derive(Debug, Serialize)]
pub(super) struct KeySummary {
    key: String,
    #[serde(flatten)]
    counts: Counts,
    clean_samples: u64,
    predicted_samples: usize,
    median_peak_predicted_ratio: Option<f64>,
    p90_peak_predicted_ratio: Option<f64>,
    average_reserved_unused_bytes: Option<f64>,
}

#[derive(Debug, Default, Serialize)]
pub(super) struct Summary {
    #[serde(flatten)]
    overall: Counts,
    start_ts_ms: Option<u64>,
    end_ts_ms: Option<u64>,
    keys: Vec<KeySummary>,
}

#[derive(Default)]
struct Samples {
    counts: Counts,
    ratios: Vec<f64>,
    unused: f64,
    clean_n: u64,
}

impl Summary {
    pub(super) fn read(dir: &Path) -> Self {
        let mut summary = Self::default();
        let mut keys: BTreeMap<String, Samples> = BTreeMap::new();
        for name in [PREVIOUS, FILE] {
            let Ok(file) = File::open(dir.join(name)) else {
                continue;
            };
            for line in BufReader::new(file)
                .split(b'\n')
                .map_while(std::result::Result::ok)
            {
                let Ok(o) = serde_json::from_slice::<Observation>(&line) else {
                    continue;
                };
                if o.v != 1 {
                    continue;
                }
                summary.start_ts_ms =
                    Some(summary.start_ts_ms.map_or(o.ts_ms, |ts| ts.min(o.ts_ms)));
                summary.end_ts_ms = Some(summary.end_ts_ms.map_or(o.ts_ms, |ts| ts.max(o.ts_ms)));
                summary.overall.add(&o);
                let samples = keys.entry(o.key.clone()).or_default();
                samples.counts.add(&o);
                // Failed, OOM and wrapper readings still contribute to safety
                // counts and worst cases, but never to clean accuracy samples.
                if o.outcome != "success" || o.wrapped {
                    continue;
                }
                let Some(peak) = o.peak_bytes else { continue };
                if let Some(predicted) = o.predicted_bytes.filter(|p| *p > 0) {
                    samples.ratios.push(peak as f64 / predicted as f64);
                }
                samples.unused += o
                    .charged_memory_bytes
                    .unwrap_or_else(|| o.charged_permits.saturating_mul(o.bytes_per_permit))
                    .saturating_sub(peak) as f64;
                samples.clean_n += 1;
            }
        }
        summary.keys = keys
            .into_iter()
            .map(|(key, mut samples)| {
                samples.ratios.sort_by(f64::total_cmp);
                let n = samples.ratios.len();
                let median =
                    (n > 0).then(|| (samples.ratios[(n - 1) / 2] + samples.ratios[n / 2]) / 2.0);
                // Nearest-rank p90, including small samples.
                let p90 = (n > 0).then(|| samples.ratios[(n * 9).div_ceil(10) - 1]);
                KeySummary {
                    key,
                    counts: samples.counts,
                    clean_samples: samples.clean_n,
                    predicted_samples: n,
                    median_peak_predicted_ratio: median,
                    p90_peak_predicted_ratio: p90,
                    average_reserved_unused_bytes: (samples.clean_n > 0)
                        .then_some(samples.unused / samples.clean_n as f64),
                }
            })
            .collect();
        summary
    }

    pub(super) fn text(&self) -> String {
        if self.overall.n == 0 {
            return "Estimate accuracy: no observations.".into();
        }
        fn ratio(value: Option<f64>) -> String {
            value.map_or_else(|| "unknown".into(), |r| format!("{r:.2}x"))
        }
        let overall = &self.overall;
        let mut lines = vec![
            format!(
                "Estimate accuracy, {} to {}: {} observations, {} exceeded their reservation.",
                utc(self.start_ts_ms),
                utc(self.end_ts_ms),
                overall.n,
                overall.exceeded,
            ),
            format!(
                "  Worst: {} of its reservation, {} ({}), at {}.",
                ratio(overall.worst_peak_reserved_ratio),
                overall.worst_key.as_deref().unwrap_or("unknown"),
                overall.worst_outcome.as_deref().unwrap_or("unknown"),
                utc(overall.worst_ts_ms),
            ),
            format!(
                "  Outcomes: {} success, {} failed, {} OOM-killed, {} other. Exceeded: {} success, {} failed, {} OOM-killed, {} other.",
                overall.success,
                overall.failed,
                overall.oom_killed,
                overall.other,
                overall.success_exceeded,
                overall.failed_exceeded,
                overall.oom_killed_exceeded,
                overall.other_exceeded,
            ),
            "  Median, p90 and unused memory count only successful runs with a measured peak that ran without another wrapper.".into(),
        ];
        let mut keys: Vec<_> = self.keys.iter().collect();
        keys.sort_by(|a, b| {
            b.counts
                .worst_peak_reserved_ratio
                .unwrap_or(-1.0)
                .total_cmp(&a.counts.worst_peak_reserved_ratio.unwrap_or(-1.0))
                .then_with(|| a.key.cmp(&b.key))
        });
        for k in keys.into_iter().take(20) {
            let counts = &k.counts;
            lines.push(format!("  {}:", k.key));
            lines.push(format!(
                "    n={} (failed={}, OOM={}), exceeded={} (success={}, failed={}, OOM={}), worst={} at {}",
                counts.n,
                counts.failed,
                counts.oom_killed,
                counts.exceeded,
                counts.success_exceeded,
                counts.failed_exceeded,
                counts.oom_killed_exceeded,
                ratio(counts.worst_peak_reserved_ratio),
                utc(counts.worst_ts_ms),
            ));
            lines.push(format!(
                "    peak/predicted median={} p90={}, highest peak={}, unused on average={}",
                ratio(k.median_peak_predicted_ratio),
                ratio(k.p90_peak_predicted_ratio),
                counts
                    .highest_peak_bytes
                    .map_or_else(|| "unknown".into(), crate::stats::size),
                k.average_reserved_unused_bytes.map_or_else(
                    || "unknown".into(),
                    |bytes| crate::stats::size(bytes as u64)
                ),
            ));
        }
        lines.join("\n")
    }
}

/// A UTC time to the second, for reading observation spans at a glance.
fn utc(ms: Option<u64>) -> String {
    ms.and_then(|ms| i64::try_from(ms).ok())
        .and_then(|ms| jiff::Timestamp::from_millisecond(ms).ok())
        .map_or_else(
            || "an unknown time".into(),
            |timestamp| timestamp.strftime("%Y-%m-%d %H:%M:%S UTC").to_string(),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool(dir: &Path, bytes: u64) -> Pool {
        let mut pool = Pool::new(dir.into(), 16, bytes, SchedulerPriority::Normal);
        pool.available_memory = || None;
        pool.pressure = false;
        pool.domain_capacity = 16;
        pool.clock = || 1000;
        pool.pressure_domain = "fixture-domain";
        pool
    }

    fn observation(peak: Option<u64>, outcome: &str, ts_ms: u64) -> Observation {
        Observation {
            v: 1,
            ts_ms,
            mbx_version: "fixture".into(),
            pressure_domain: "fixture-domain".into(),
            kind: "compile".into(),
            name: "crate".into(),
            key: "crate [compile fixture]".into(),
            build_id: None,
            build_capacity: None,
            domain_capacity: None,
            source: "mode".into(),
            predicted_bytes: Some(100),
            charged_permits: 2,
            charged_cpu: None,
            charged_memory_bytes: None,
            bytes_per_permit: 100,
            peak_bytes: peak,
            outcome: outcome.into(),
            wrapped: false,
            admission_wait_ms: 4,
            run_ms: 10,
        }
    }

    fn status(code: u32) -> ExitStatus {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            ExitStatus::from_raw(code as i32)
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::ExitStatusExt;
            ExitStatus::from_raw(code)
        }
    }

    #[test]
    fn short_append_salvages_framing_without_retrying_the_record() {
        #[derive(Default)]
        struct Writer {
            bytes: Vec<u8>,
            calls: usize,
            short: Option<usize>,
            fail_newline: bool,
        }
        impl Write for Writer {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.calls += 1;
                if bytes == b"\n" && self.fail_newline {
                    return Err(std::io::Error::other("newline failed"));
                }
                let count = self.short.take().unwrap_or(bytes.len());
                self.bytes.extend_from_slice(&bytes[..count]);
                Ok(count)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut writer = Writer {
            short: Some(3),
            ..Default::default()
        };
        assert!(write_line(&mut writer, b"{\"event\":1}\n").is_err());
        assert_eq!(writer.bytes, b"{\"e\n");
        assert_eq!(writer.calls, 2);
        write_line(&mut writer, b"{\"event\":2}\n").unwrap();
        assert_eq!(writer.calls, 3);
        let last = writer.bytes.split(|b| *b == b'\n').nth(1).unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(last).unwrap()["event"],
            2
        );
        let mut failed = Writer {
            short: Some(3),
            fail_newline: true,
            ..Default::default()
        };
        assert!(write_line(&mut failed, b"{\"event\":1}\n").is_err());
        assert_eq!(failed.calls, 2);
        let mut normal = Writer::default();
        write_line(&mut normal, b"{}\n").unwrap();
        assert_eq!(normal.calls, 1);
    }

    #[test]
    fn kinds_schema_size_and_admitted_charge() {
        let dir = tempfile::tempdir().unwrap();
        let mut pool = pool(dir.path(), 100);
        pool.build = Some(("build-123".into(), 7));
        let demands = [
            (
                Demand::new("crate", false).with_link_profile(&[]),
                "compile",
            ),
            (Demand::new("link", true), "link"),
            (Demand::test("suite", Some(1)), "test-binary"),
            (Demand::nextest("binary", "case"), "nextest-test"),
        ];
        for (demand, kind) in demands {
            let permit = pool.admit(&demand).unwrap();
            let admission = demand.observation.get().unwrap();
            pool.observe(&demand, &status(0), Some(150));
            drop(permit);
            let lines = std::fs::read_to_string(dir.path().join(FILE)).unwrap();
            let line = lines.lines().next_back().unwrap();
            assert!(line.len() < 1023);
            let value: serde_json::Value = serde_json::from_str(line).unwrap();
            assert_eq!(value["v"], 1);
            assert_eq!(value["ts_ms"], 1000);
            assert_eq!(value["mbx_version"], env!("CARGO_PKG_VERSION"));
            assert_eq!(value["kind"], kind);
            assert_eq!(value["name"], demand.name);
            assert_eq!(
                value["key"],
                demand
                    .compile_mode
                    .clone()
                    .unwrap_or_else(|| ledger_key(&demand.name, demand.links))
            );
            assert_eq!(value["source"], admission.source);
            assert_eq!(
                value["predicted_bytes"],
                serde_json::json!(admission.predicted)
            );
            assert_eq!(value["charged_permits"], admission.weight);
            assert_eq!(value["charged_cpu"], admission.resources.cpu);
            assert_eq!(
                value["charged_memory_bytes"],
                admission.resources.memory_bytes
            );
            assert_eq!(value["bytes_per_permit"], 100);
            assert_eq!(value["pressure_domain"], "fixture-domain");
            assert_eq!(value["build_id"], "build-123");
            assert_eq!(value["build_capacity"], 7);
            assert_eq!(value["domain_capacity"], 16);
            assert_eq!(value["peak_bytes"], 150);
            assert_eq!(value["outcome"], "success");
            assert_eq!(value["wrapped"], false);
            assert!(value["run_ms"].is_u64());
            assert!(value["admission_wait_ms"].is_u64());
            pool.observe(&demand, &status(0), Some(150));
            assert_eq!(
                std::fs::read_to_string(dir.path().join(FILE)).unwrap(),
                lines
            );
        }
        let mut long = observation(Some(u64::MAX), "failed", u64::MAX);
        long.predicted_bytes = Some(u64::MAX);
        long.charged_permits = u64::MAX;
        long.charged_cpu = Some(u64::MAX);
        long.charged_memory_bytes = Some(u64::MAX);
        long.bytes_per_permit = u64::MAX;
        long.admission_wait_ms = u64::MAX;
        long.run_ms = u64::MAX;
        long.name = "\n🦀".repeat(500);
        long.key = "\u{0001}🦀".repeat(500);
        long.pressure_domain = "\n".repeat(500);
        append(dir.path(), long).unwrap();
        let bytes = std::fs::read(dir.path().join(FILE)).unwrap();
        for line in bytes.split(|b| *b == b'\n').filter(|line| !line.is_empty()) {
            assert!(line.len() < 1023);
            serde_json::from_slice::<Observation>(line).unwrap();
        }
    }

    #[test]
    fn old_observation_lines_parse_without_optional_capacities() {
        let mut value = serde_json::to_value(observation(Some(10), "success", 1000)).unwrap();
        value.as_object_mut().unwrap().remove("charged_cpu");
        value
            .as_object_mut()
            .unwrap()
            .remove("charged_memory_bytes");
        value.as_object_mut().unwrap().remove("build_id");
        value.as_object_mut().unwrap().remove("build_capacity");
        value.as_object_mut().unwrap().remove("domain_capacity");
        let parsed: Observation = serde_json::from_value(value).unwrap();
        assert_eq!(parsed.charged_cpu, None);
        assert_eq!(parsed.charged_memory_bytes, None);
        assert_eq!(parsed.build_id, None);
        assert_eq!(parsed.build_capacity, None);
        assert_eq!(parsed.domain_capacity, None);

        let encoded = serde_json::to_value(observation(Some(10), "success", 1000)).unwrap();
        assert!(encoded.get("build_id").is_none());
        assert!(encoded.get("build_capacity").is_none());
        assert!(encoded.get("domain_capacity").is_none());
        let dir = tempfile::tempdir().unwrap();
        append(dir.path(), observation(Some(10), "success", 1000)).unwrap();
        let summary = Summary::read(dir.path());
        assert_eq!(summary.overall.n, 1);
        assert_eq!(summary.overall.exceeded, 0);
    }

    #[test]
    fn reservation_observation_records_weight_and_respects_gates() {
        let dir = tempfile::tempdir().unwrap();
        let observed_pool = pool(dir.path(), 100);
        let (permit, admission) = observed_pool.reserve_observed(Some(3), None).unwrap();
        drop(permit);
        observed_pool.observe_reservation(admission, Some(&status(0)));
        let line = std::fs::read_to_string(dir.path().join(FILE)).unwrap();
        let value: serde_json::Value = serde_json::from_str(line.lines().next().unwrap()).unwrap();
        assert_eq!(value["kind"], "reservation");
        assert_eq!(value["key"], "reservation");
        assert_eq!(value["charged_permits"], 3);
        assert_eq!(value["charged_cpu"], 3);
        assert_eq!(value["charged_memory_bytes"], 0);
        assert_eq!(value["bytes_per_permit"], 100);
        assert_eq!(value["peak_bytes"], serde_json::Value::Null);
        assert_eq!(value["outcome"], "success");
        let summary = Summary::read(dir.path());
        assert_eq!(summary.overall.n, 1);
        assert_eq!(summary.overall.exceeded, 0);
        assert_eq!(summary.overall.measured, 0);
        assert_eq!(summary.keys.len(), 1);
        assert_eq!(summary.keys[0].key, "reservation");
        assert_eq!(summary.keys[0].counts.n, 1);

        for (events, bytes) in [(false, 100), (true, 0)] {
            let gated_dir = tempfile::tempdir().unwrap();
            let mut gated_pool = pool(gated_dir.path(), bytes);
            gated_pool.events = events;
            let (permit, admission) = gated_pool.reserve_observed(Some(1), None).unwrap();
            drop(permit);
            gated_pool.observe_reservation(admission, Some(&status(0)));
            assert!(!gated_dir.path().join(FILE).exists());
        }
    }

    #[test]
    fn summary_prefers_exact_memory_charge_and_supports_legacy_lines() {
        let dir = tempfile::tempdir().unwrap();
        let mut exact = observation(Some(150), "success", 1000);
        exact.charged_cpu = Some(2);
        exact.charged_memory_bytes = Some(100);
        append(dir.path(), exact).unwrap();
        append(dir.path(), observation(Some(150), "success", 2000)).unwrap();
        let summary = Summary::read(dir.path());
        assert_eq!(summary.overall.exceeded, 1);
        assert_eq!(summary.overall.worst_peak_reserved_ratio, Some(1.5));
        assert_eq!(summary.keys[0].average_reserved_unused_bytes, Some(25.0));
        let (permit, admission) = pool(dir.path(), 100)
            .reserve_observed(Some(2), Some(201))
            .unwrap();
        drop(permit);
        pool(dir.path(), 100).observe_reservation(admission, None);
        let lines = std::fs::read_to_string(dir.path().join(FILE)).unwrap();
        let value: serde_json::Value =
            serde_json::from_str(lines.lines().next_back().unwrap()).unwrap();
        assert_eq!(value["charged_cpu"], 2);
        assert_eq!(value["charged_memory_bytes"], 201);
        assert_eq!(value["charged_permits"], 3);
    }

    #[test]
    fn gates_and_missing_admission_do_not_log() {
        for (events, bytes) in [(false, 100), (true, 0)] {
            let dir = tempfile::tempdir().unwrap();
            let mut pool = pool(dir.path(), bytes);
            pool.events = events;
            let demand = Demand::new("crate", false);
            let _permit = pool.admit(&demand).unwrap();
            pool.observe(&demand, &status(0), Some(150));
            assert!(!dir.path().join(FILE).exists());
        }
        let dir = tempfile::tempdir().unwrap();
        pool(dir.path(), 100).observe(&Demand::new("unadmitted", false), &status(0), Some(150));
        assert!(!dir.path().join(FILE).exists());
    }

    #[test]
    fn rotation_replaces_previous_and_skips_held_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        File::create(&path).unwrap().set_len(MAX_BYTES + 1).unwrap();
        std::fs::write(dir.path().join(PREVIOUS), b"old").unwrap();
        let mut lock = fslock::LockFile::open(&dir.path().join(LOCK)).unwrap();
        lock.lock().unwrap();
        append(dir.path(), observation(Some(100), "success", 1)).unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() > MAX_BYTES);
        assert_eq!(std::fs::read(dir.path().join(PREVIOUS)).unwrap(), b"old");
        drop(lock);
        append(dir.path(), observation(Some(100), "success", 2)).unwrap();
        assert!(std::fs::metadata(dir.path().join(PREVIOUS)).unwrap().len() > MAX_BYTES);
        assert!(std::fs::metadata(path).unwrap().len() < 1024);
    }

    #[test]
    fn prediction_sources_are_from_the_selected_path() {
        let dir = tempfile::tempdir().unwrap();
        let pool = pool(dir.path(), 100);
        let demand = Demand::new("crate", false).with_link_profile(&[]);
        assert_eq!(pool.prediction(&demand).2, "default");
        pool.record_link_peak("crate", 1000, false, None).unwrap();
        assert_eq!(pool.prediction(&demand).2, "legacy");
        for _ in 0..3 {
            pool.record_compile(&demand, 200, false, true).unwrap();
        }
        assert_eq!(pool.prediction(&demand), (3, Some(300), "window"));
        let link = Demand::new("unseen", true);
        assert_eq!(pool.prediction(&link).2, "link-floor");
        for _ in 0..3 {
            pool.record_link_peak("known [link]", 300, true, None)
                .unwrap();
        }
        assert_eq!(pool.prediction(&link).2, "link-window");
        assert_eq!(pool.prediction(&Demand::new("known", true)).2, "window");
        let test = Demand::test("suite", Some(1));
        pool.record_link_peak(&test.name, 300, false, None).unwrap();
        assert_eq!(pool.prediction(&test).2, "test-history");
        pool.record_test_history("bin", "case", Some(300), None)
            .unwrap();
        assert_eq!(
            pool.prediction(&Demand::nextest("bin", "case")).2,
            "test-history"
        );
        assert_eq!(
            pool.prediction(&Demand::nextest("bin", "sibling")).2,
            "test-history"
        );
    }

    #[test]
    fn summary_fixture_counts_failures_apart_and_uses_only_clean_samples() {
        let dir = tempfile::tempdir().unwrap();
        let mut previous = File::create(dir.path().join(PREVIOUS)).unwrap();
        previous.write_all(b"\xff\n").unwrap();
        for (peak, ts) in [(50, 1000), (100, 2000)] {
            writeln!(
                previous,
                "{}",
                serde_json::to_string(&observation(Some(peak), "success", ts)).unwrap()
            )
            .unwrap();
        }
        writeln!(previous, "malformed").unwrap();
        for (peak, outcome, ts) in [
            (Some(200), "success", 3000),
            (Some(500), "failed", 4000),
            (Some(800), "oom-killed", 5000),
            (None, "success", 6000),
        ] {
            append(dir.path(), observation(peak, outcome, ts)).unwrap();
        }
        let mut wrapped = observation(Some(1000), "success", 7000);
        wrapped.wrapped = true;
        append(dir.path(), wrapped).unwrap();
        let mut unknown = observation(Some(60), "success", 8000);
        unknown.predicted_bytes = None;
        append(dir.path(), unknown).unwrap();
        let summary = Summary::read(dir.path());
        assert_eq!(summary.overall.n, 8);
        assert_eq!(summary.overall.exceeded, 3);
        assert_eq!(summary.overall.failed_exceeded, 1);
        assert_eq!(summary.overall.oom_killed_exceeded, 1);
        assert_eq!(summary.overall.worst_peak_reserved_ratio, Some(5.0));
        assert_eq!(summary.overall.worst_ts_ms, Some(7000));
        assert_eq!(summary.start_ts_ms, Some(1000));
        assert_eq!(summary.end_ts_ms, Some(8000));
        let key = &summary.keys[0];
        assert_eq!(key.counts.failed, 1);
        assert_eq!(key.counts.oom_killed, 1);
        assert_eq!(key.clean_samples, 4);
        assert_eq!(key.predicted_samples, 3);
        assert_eq!(key.median_peak_predicted_ratio, Some(1.0));
        assert_eq!(key.p90_peak_predicted_ratio, Some(2.0));
        assert_eq!(key.average_reserved_unused_bytes, Some(97.5));
        assert_eq!(key.counts.highest_peak_bytes, Some(1000));
        let json = serde_json::to_value(&summary).unwrap();
        assert_eq!(json["n"], 8);
        assert_eq!(json["keys"][0]["failed"], 1);
        let text = summary.text();
        assert!(
            text.contains("1970-01-01 00:00:01 UTC to 1970-01-01 00:00:08 UTC"),
            "{text}"
        );
        assert!(text.contains("at 1970-01-01 00:00:07 UTC"), "{text}");
    }

    #[test]
    fn failed_oom_wrapped_and_unavailable_peaks_are_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let pool = pool(dir.path(), 100);
        let mut demand = Demand::new("crate", false);
        demand.count_compile_samples = false;
        for (status, expected) in [(status(0), "success"), (status(256), "failed")] {
            let _permit = pool.admit(&demand).unwrap();
            pool.observe(&demand, &status, None);
            let text = std::fs::read_to_string(dir.path().join(FILE)).unwrap();
            let o: Observation = serde_json::from_str(text.lines().next_back().unwrap()).unwrap();
            assert_eq!(o.outcome, expected);
            assert!(o.wrapped);
            assert_eq!(o.peak_bytes, None);
        }
        #[cfg(unix)]
        {
            let _permit = pool.admit(&demand).unwrap();
            pool.observe(&demand, &status(9), Some(900));
            let summary = Summary::read(dir.path());
            assert_eq!(summary.overall.oom_killed, 1);
            assert_eq!(summary.overall.exceeded, 1);
            drop(_permit);
            let _permit = pool.admit(&demand).unwrap();
            pool.observe(&demand, &status(15), Some(900));
            let summary = Summary::read(dir.path());
            assert_eq!(summary.overall.other, 1);
            assert_eq!(summary.overall.other_exceeded, 1);
            let text = std::fs::read_to_string(dir.path().join(FILE)).unwrap();
            let o: Observation = serde_json::from_str(text.lines().next_back().unwrap()).unwrap();
            assert_eq!(o.outcome, "signal-15");
        }
    }

    #[test]
    fn session_events_setting_is_forwarded() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::for_test(dir.path());
        config.scheduler.enabled = true;
        for enabled in [false, true] {
            config.events = enabled;
            let env = session_environment_with_jobs(&config, None);
            assert!(env.contains(&("MBX_SCHED_EVENTS".into(), u8::from(enabled).to_string())));
            assert_eq!(Pool::from_config(&config).unwrap().events, enabled);
            assert_eq!(
                reservation_pool(&config, SchedulerPriority::Normal)
                    .unwrap()
                    .events,
                enabled
            );
        }
    }

    #[test]
    fn default_stats_ignores_observations_in_text_and_json() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::for_test(dir.path());
        let before = crate::stats::collect(&config.store_dir(), &config.target.root).unwrap();
        append(
            &dir.path().join(SCHEDULER_DIR),
            observation(Some(500), "failed", 1000),
        )
        .unwrap();
        let after = crate::stats::collect(&config.store_dir(), &config.target.root).unwrap();
        assert_eq!(before.text(false), after.text(false));
        assert_eq!(
            serde_json::to_value(before).unwrap(),
            serde_json::to_value(after).unwrap()
        );
    }

    #[test]
    fn text_limits_ranked_keys_while_json_retains_all() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..25 {
            let mut o = observation(Some(i * 100), "success", i + 1);
            o.key = format!("key-{i:02}");
            append(dir.path(), o).unwrap();
        }
        let summary = Summary::read(dir.path());
        let text = summary.text();
        assert_eq!(
            text.lines()
                .filter(|line| line.starts_with("  key-"))
                .count(),
            20
        );
        assert!(text.contains("  key-24:"));
        assert!(!text.contains("  key-04:"));
        assert!(text.find("  key-24:").unwrap() < text.find("  key-23:").unwrap());
        assert_eq!(
            serde_json::to_value(&summary).unwrap()["keys"]
                .as_array()
                .unwrap()
                .len(),
            25
        );
    }

    #[test]
    fn even_median_and_nearest_rank_p90() {
        let dir = tempfile::tempdir().unwrap();
        for peak in [100, 300, 200, 400] {
            append(dir.path(), observation(Some(peak), "success", 1)).unwrap();
        }
        let summary = Summary::read(dir.path());
        assert_eq!(summary.keys[0].median_peak_predicted_ratio, Some(2.5));
        assert_eq!(summary.keys[0].p90_peak_predicted_ratio, Some(4.0));
    }

    #[test]
    fn a_long_build_id_is_shortened_instead_of_dropping_the_line() {
        let dir = tempfile::tempdir().unwrap();
        let mut long = observation(Some(1), "success", 1);
        long.build_id = Some("b".repeat(2000));
        append(dir.path(), long).unwrap();
        let summary = Summary::read(dir.path());
        assert_eq!(summary.keys.len(), 1);
        assert_eq!(summary.keys[0].counts.n, 1);
    }

    #[test]
    fn a_long_key_is_shortened_the_same_way_on_every_line() {
        let dir = tempfile::tempdir().unwrap();
        let key = format!("crate [compile {}]", "x".repeat(200));
        let mut short_line = observation(Some(1), "success", 1);
        short_line.key.clone_from(&key);
        let mut long_line = observation(Some(u64::MAX), "success", u64::MAX);
        long_line.key.clone_from(&key);
        long_line.mbx_version = "v".repeat(300);
        append(dir.path(), short_line).unwrap();
        append(dir.path(), long_line).unwrap();
        let summary = Summary::read(dir.path());
        assert_eq!(summary.keys.len(), 1);
        assert_eq!(summary.keys[0].counts.n, 2);
        assert!(summary.keys[0].key.len() < key.len());
    }

    #[test]
    fn empty_summary_is_read_only() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            Summary::read(&dir.path().join("missing")).text(),
            "Estimate accuracy: no observations."
        );
        assert!(!dir.path().join("missing").exists());
    }
}
