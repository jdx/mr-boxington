use mbx_cache_core::AgentStats;
use serde_json::Value;
use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub(super) struct Warning {
    pub message: String,
    pub rendered: String,
    pub location: Option<String>,
    pub help: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct Row {
    pub name: String,
    pub duration: Option<Duration>,
    pub fresh: bool,
    pub failed: bool,
}

#[derive(Clone, Debug, Default)]
pub(super) struct CacheMix {
    pub hits: u64,
    pub misses: u64,
    pub bypasses: u64,
    pub saved_ns: u64,
    pub unconsulted: u64,
}

impl From<AgentStats> for CacheMix {
    fn from(stats: AgentStats) -> Self {
        Self {
            hits: stats.hits,
            unconsulted: stats.unconsulted,
            misses: crate::session::cache_misses(&stats),
            bypasses: crate::session::unexpected_bypasses(&stats),
            saved_ns: stats.avoided_compiler_duration_ns,
        }
    }
}

/// Color proportions describe cache actions, not Cargo artifacts. Rounding at
/// cumulative boundaries guarantees that the segments use exactly `width` cells.
pub(super) fn segments(mix: &CacheMix, width: usize) -> [usize; 3] {
    let total = u128::from(mix.hits)
        + u128::from(mix.misses)
        + u128::from(mix.bypasses)
        + u128::from(mix.unconsulted);
    if total == 0 {
        return [0, 0, width];
    }
    let hit_end = (u128::from(mix.hits) * width as u128 / total) as usize;
    let miss_end =
        ((u128::from(mix.hits) + u128::from(mix.misses)) * width as u128 / total) as usize;
    [hit_end, miss_end - hit_end, width - miss_end]
}

pub(super) struct Model {
    pub command: String,
    pub started: Instant,
    pub live: BTreeMap<String, Instant>,
    pub done: VecDeque<Row>,
    pub units_total: Option<usize>,
    pub units_done: usize,
    pub artifacts: usize,
    pub fresh: usize,
    pub warnings: Vec<Warning>,
    pub errors: Vec<String>,
    pub build_finished: bool,
    pub build_ok: Option<bool>,
    pub testing: bool,
    pub suite_total: usize,
    pub suite_done: usize,
    pub tests_passed: usize,
    pub tests_failed: usize,
    pub tests_ignored: usize,
    pub failures: Vec<(String, String)>,
    pub mix: CacheMix,
    pub finished: Option<(bool, Duration)>,
    failure_capture: Option<usize>,
    failure_section: bool,
    suite_before: [usize; 3],
}

impl Model {
    pub fn new(arguments: &[String]) -> Self {
        Self {
            command: format!("cargo {}", arguments.join(" ")),
            started: Instant::now(),
            live: BTreeMap::new(),
            done: VecDeque::new(),
            units_total: None,
            units_done: 0,
            artifacts: 0,
            fresh: 0,
            warnings: Vec::new(),
            errors: Vec::new(),
            build_finished: false,
            build_ok: None,
            testing: false,
            suite_total: 0,
            suite_done: 0,
            tests_passed: 0,
            tests_failed: 0,
            tests_ignored: 0,
            failures: Vec::new(),
            mix: CacheMix::default(),
            finished: None,
            failure_capture: None,
            failure_section: false,
            suite_before: [0; 3],
        }
    }

    fn row(&mut self, row: Row) {
        self.done.push_back(row);
        while self.done.len() > 6 {
            self.done.pop_front();
        }
    }

    /// Consume only recognized Cargo protocol records. Other output is retained
    /// verbatim by the transport, including custom harnesses and invalid UTF-8.
    pub fn cargo(&mut self, line: &str) -> bool {
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            return false;
        };
        match message["reason"].as_str() {
            Some("compiler-artifact") => {
                let target = message["target"]["name"].as_str().unwrap_or("crate");
                let id = message["package_id"].as_str().unwrap_or(target);
                let suffix = id.rsplit('#').next().unwrap_or(id);
                let (name, version) = suffix.split_once('@').unwrap_or((target, suffix));
                let expected = format!("{name} v{version}").replace('_', "-");
                let key = self
                    .live
                    .keys()
                    .find(|key| key.replace('_', "-") == expected)
                    .cloned()
                    .unwrap_or_else(|| format!("{name} v{version}"));
                let start = self.live.remove(&key);
                let fresh = message["fresh"].as_bool().unwrap_or(false);
                if fresh {
                    self.fresh += 1;
                } else {
                    self.artifacts += 1;
                }
                self.row(Row {
                    name: key,
                    duration: start.map(|start| start.elapsed()),
                    fresh,
                    failed: false,
                });
                true
            }
            Some("compiler-message") => {
                let diagnostic = &message["message"];
                let rendered = diagnostic["rendered"].as_str().unwrap_or("").to_owned();
                let text = diagnostic["message"].as_str().unwrap_or("").to_owned();
                if diagnostic["level"] == "warning" {
                    let span = diagnostic["spans"]
                        .as_array()
                        .and_then(|spans| spans.iter().find(|s| s["is_primary"] == true));
                    let location = span.map(|span| {
                        format!(
                            "{}:{}",
                            span["file_name"].as_str().unwrap_or(""),
                            span["line_start"]
                        )
                    });
                    self.warnings.push(Warning {
                        message: text,
                        rendered,
                        location,
                        help: None,
                    });
                } else {
                    self.errors
                        .push(if rendered.is_empty() { text } else { rendered });
                }
                true
            }
            Some("build-script-executed") => true,
            Some("build-finished") => {
                self.build_finished = true;
                self.build_ok = message["success"].as_bool();
                self.live.clear();
                true
            }
            _ => false,
        }
    }

    pub fn status(&mut self, line: &str) -> bool {
        // Cargo right-aligns these labels in twelve columns and follows them
        // with a package name and version. Leave ordinary child output alone.
        for verb in ["   Compiling ", "    Checking "] {
            if let Some(package) = line.strip_prefix(verb) {
                let package = package.split(" (").next().unwrap_or(package).trim();
                let Some((name, version)) = package.split_once(" v") else {
                    return false;
                };
                if name.is_empty()
                    || !name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                    || version.split('.').take(3).count() != 3
                    || !version.starts_with(|c: char| c.is_ascii_digit())
                {
                    return false;
                }
                self.live
                    .entry(package.to_string())
                    .or_insert_with(Instant::now);
                return true;
            }
        }
        if line.starts_with("    Building [")
            && let Some((_, tail)) = line.split_once(']')
            && let Some(count) = tail.split_whitespace().next()
            && let Some((done, total)) = count.trim_end_matches(':').split_once('/')
            && let (Ok(done), Ok(total)) = (done.parse::<usize>(), total.parse::<usize>())
            && total > 0
            && done <= total
        {
            self.units_done = done;
            self.units_total = Some(total);
            return true;
        }
        if line.starts_with("    Finished `") && line.contains(" target(s) in ") {
            return true;
        }
        // Cargo's native progress supplies a reliable unit total when emitted.
        // No nightly probe or guessed metadata denominator is needed.
        false
    }

    /// Parse stable libtest output while Cargo retains orchestration of test
    /// executables, doctests and configured runners. Stable libtest does not
    /// report per-test start times, so those durations stay unknown.
    pub fn test_line(&mut self, line: &str) -> bool {
        if let Some(total) = line
            .strip_prefix("running ")
            .and_then(|s| s.strip_suffix(" tests").or_else(|| s.strip_suffix(" test")))
            .and_then(|s| s.parse().ok())
        {
            self.testing = true;
            self.failure_section = false;
            self.failure_capture = None;
            self.suite_before = [self.tests_passed, self.tests_failed, self.tests_ignored];
            self.suite_total = total;
            self.suite_done = 0;
            self.done.clear();
            return true;
        }
        if self.testing
            && let Some(rest) = line.strip_prefix("test ")
            && let Some((name, result)) = rest.rsplit_once(" ... ")
        {
            let failed = result == "FAILED";
            match result {
                "ok" => self.tests_passed += 1,
                "FAILED" => self.tests_failed += 1,
                "ignored" => self.tests_ignored += 1,
                _ => return false,
            }
            self.suite_done += 1;
            self.row(Row {
                name: name.into(),
                duration: None,
                fresh: false,
                failed,
            });
            return true;
        }
        if let Some(summary) = line.strip_prefix("test result:") {
            for part in summary.split(['.', ';']) {
                let mut words = part.split_whitespace();
                if let Some(count) = words.next().and_then(|word| word.parse::<usize>().ok()) {
                    match words.next() {
                        Some("passed") => self.tests_passed = self.suite_before[0] + count,
                        Some("failed") => self.tests_failed = self.suite_before[1] + count,
                        Some("ignored") => self.tests_ignored = self.suite_before[2] + count,
                        _ => {}
                    }
                }
            }
            self.suite_done = (self.tests_passed - self.suite_before[0])
                + (self.tests_failed - self.suite_before[1])
                + (self.tests_ignored - self.suite_before[2]);
        }
        if line == "failures:" {
            self.failure_section = true;
            self.failure_capture = None;
        } else if line == "successes:" || line.starts_with("test result:") {
            self.failure_section = false;
            self.failure_capture = None;
        }
        if self.failure_section
            && let Some(name) = line
                .strip_prefix("---- ")
                .and_then(|s| s.strip_suffix(" stdout ----"))
        {
            self.failures.push((name.to_string(), String::new()));
            self.failure_capture = Some(self.failures.len() - 1);
        } else if line.starts_with("failures:") || line.starts_with("test result:") {
            self.failure_capture = None;
        } else if let Some(index) = self.failure_capture {
            let details = &mut self.failures[index].1;
            if details.len() < 256 * 1024 {
                details.push_str(line);
                details.push('\n');
            }
        }
        false
    }
}

pub(super) fn strip_ansi(text: &str) -> String {
    let mut output = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.next() == Some('[') {
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            }
        } else if c != '\r' {
            output.push(c);
        }
    }
    output
}
