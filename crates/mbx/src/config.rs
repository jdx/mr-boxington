//! Configuration, resolved from environment variables over an optional file.
//!
//! Precedence is environment, then `~/.config/mbx/config.toml`, then defaults.

use crate::util::parse_duration;
use bytesize::ByteSize;
use eyre::{Context, Result};
use mbx_cache_core::RemoteCacheMode;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::Duration;

const DEFAULT_HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_HTTP_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);
const DEFAULT_HTTP_RETRIES: i64 = 3;
/// Stated in IEC units so the budget reads the way `mbx gc` reports it back.
const DEFAULT_GC_MAX_SIZE: u64 = 20 * 1024 * 1024 * 1024;
const DEFAULT_GC_INTERVAL: Duration = Duration::from_secs(60 * 60);

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct ConfigFile {
    cache_dir: Option<PathBuf>,
    stats_report: Option<PathBuf>,
    incremental: Option<bool>,
    share_out_dir: Option<bool>,
    remote: RemoteFile,
    http: HttpFile,
    gc: GcFile,
    target: TargetFile,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct RemoteFile {
    url: Option<String>,
    namespace: Option<String>,
    token: Option<String>,
    token_file: Option<PathBuf>,
    oidc_audience: Option<String>,
    mode: Option<RemoteCacheMode>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct TargetFile {
    views: Option<bool>,
    root: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct GcFile {
    auto: Option<bool>,
    max_size: Option<String>,
    interval: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct HttpFile {
    timeout: Option<String>,
    download_timeout: Option<String>,
    retries: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub cache_dir: PathBuf,
    pub stats_report: Option<PathBuf>,
    pub verify: bool,
    /// Let cargo compile workspace members incrementally, rather than forcing
    /// `CARGO_INCREMENTAL=0` for the whole build.
    pub incremental: bool,
    /// Let a compilation that reads `OUT_DIR` be shared between checkouts.
    ///
    /// Off by default: it changes the compilation, remapping the generated
    /// sources out of debug info, and its safety rests on reading the outputs
    /// rather than on the inputs alone.
    pub share_out_dir: bool,
    pub remote: RemoteSettings,
    pub http: HttpSettings,
    pub gc: GcSettings,
    pub target: TargetSettings,
}

#[derive(Debug, Clone, Default)]
pub struct RemoteSettings {
    pub url: Option<String>,
    pub namespace: Option<String>,
    pub token: Option<String>,
    pub token_file: Option<PathBuf>,
    pub oidc_audience: Option<String>,
    pub mode: RemoteCacheMode,
}

#[derive(Debug, Clone)]
pub struct HttpSettings {
    pub timeout: Duration,
    pub download_timeout: Duration,
    pub retries: i64,
}

/// Where build outputs are written, and whether mbx places them.
#[derive(Debug, Clone)]
pub struct TargetSettings {
    pub views: bool,
    pub root: PathBuf,
}

/// How the store is kept inside its budget.
#[derive(Debug, Clone)]
pub struct GcSettings {
    pub auto: bool,
    pub max_bytes: u64,
    pub interval: Duration,
}

impl Default for GcSettings {
    fn default() -> Self {
        Self {
            auto: true,
            max_bytes: DEFAULT_GC_MAX_SIZE,
            interval: DEFAULT_GC_INTERVAL,
        }
    }
}

impl Default for HttpSettings {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_HTTP_TIMEOUT,
            download_timeout: DEFAULT_HTTP_DOWNLOAD_TIMEOUT,
            retries: DEFAULT_HTTP_RETRIES,
        }
    }
}

impl Config {
    /// Load configuration for this machine.
    pub fn load() -> Result<Self> {
        let file = match config_file_path() {
            Some(path) => read_config_file(&path)?,
            None => ConfigFile::default(),
        };
        Self::from_parts(file, |name| std::env::var(name).ok())
    }

    fn from_parts(file: ConfigFile, get_env: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let cache_dir = get_env("MBX_CACHE_DIR")
            .map(PathBuf::from)
            .or(file.cache_dir)
            .or_else(default_cache_dir)
            .ok_or_else(|| {
                eyre::eyre!("could not determine a cache directory; set MBX_CACHE_DIR")
            })?;
        let mode = match get_env("MBX_REMOTE_MODE") {
            Some(value) => value
                .parse()
                .wrap_err_with(|| format!("invalid MBX_REMOTE_MODE: {value}"))?,
            None => file.remote.mode.unwrap_or_default(),
        };
        let http = HttpSettings {
            timeout: optional_duration("MBX_HTTP_TIMEOUT", &get_env, file.http.timeout.as_deref())?
                .unwrap_or(DEFAULT_HTTP_TIMEOUT),
            download_timeout: optional_duration(
                "MBX_HTTP_DOWNLOAD_TIMEOUT",
                &get_env,
                file.http.download_timeout.as_deref(),
            )?
            .unwrap_or(DEFAULT_HTTP_DOWNLOAD_TIMEOUT),
            retries: match get_env("MBX_HTTP_RETRIES") {
                Some(value) => value
                    .parse()
                    .wrap_err_with(|| format!("invalid MBX_HTTP_RETRIES: {value}"))?,
                None => file.http.retries.unwrap_or(DEFAULT_HTTP_RETRIES),
            },
        };
        let gc = GcSettings {
            auto: optional_bool("MBX_GC_AUTO", &get_env, file.gc.auto)?
                .unwrap_or(GcSettings::default().auto),
            max_bytes: optional_byte_size(
                "MBX_GC_MAX_SIZE",
                &get_env,
                file.gc.max_size.as_deref(),
            )?
            .unwrap_or(DEFAULT_GC_MAX_SIZE),
            interval: optional_duration("MBX_GC_INTERVAL", &get_env, file.gc.interval.as_deref())?
                .unwrap_or(DEFAULT_GC_INTERVAL),
        };
        let target = TargetSettings {
            views: optional_bool("MBX_TARGET_VIEWS", &get_env, file.target.views)?
                .unwrap_or_default(),
            root: get_env("MBX_TARGET_ROOT")
                .map(PathBuf::from)
                .or(file.target.root)
                .unwrap_or_else(|| cache_dir.join("targets")),
        };
        Ok(Self {
            cache_dir,
            gc,
            target,
            stats_report: get_env("MBX_STATS_REPORT")
                .map(PathBuf::from)
                .or(file.stats_report),
            verify: get_env("MBX_VERIFY").is_some_and(|value| enabled(&value)),
            incremental: match get_env("MBX_INCREMENTAL") {
                // The environment decides outright when it is set, so a `0`
                // there turns off what the file turned on.
                Some(value) => enabled(&value),
                None => file.incremental.unwrap_or(false),
            },
            share_out_dir: match get_env("MBX_SHARE_OUT_DIR") {
                Some(value) => enabled(&value),
                None => file.share_out_dir.unwrap_or(false),
            },
            remote: RemoteSettings {
                url: get_env("MBX_REMOTE_URL").or(file.remote.url),
                namespace: get_env("MBX_REMOTE_NAMESPACE").or(file.remote.namespace),
                token: get_env("MBX_REMOTE_TOKEN").or(file.remote.token),
                token_file: get_env("MBX_REMOTE_TOKEN_FILE")
                    .map(PathBuf::from)
                    .or(file.remote.token_file),
                oidc_audience: get_env("MBX_REMOTE_OIDC_AUDIENCE").or(file.remote.oidc_audience),
                mode,
            },
            http,
        })
    }

    /// Where cached actions and blobs live.
    pub fn store_dir(&self) -> PathBuf {
        self.cache_dir.join("actions")
    }
}

/// Whether an environment value asks for a feature. Empty and `0` do not.
fn enabled(value: &str) -> bool {
    !value.is_empty() && value != "0"
}

/// Resolve a byte size, written either plainly or with a unit.
///
/// Both IEC and SI spellings parse -- `20GiB` and `20GB` are different numbers,
/// and sizes are reported back in IEC, so the two are not interchangeable.
fn optional_byte_size(
    name: &str,
    get_env: &impl Fn(&str) -> Option<String>,
    from_file: Option<&str>,
) -> Result<Option<u64>> {
    let Some(value) = get_env(name).or_else(|| from_file.map(str::to_owned)) else {
        return Ok(None);
    };
    // `ByteSize`'s parse error is a bare `String`, so it cannot be a source.
    value
        .trim()
        .parse::<ByteSize>()
        .map(|size| Some(size.as_u64()))
        .map_err(|error| eyre::eyre!("invalid {name}: {error}"))
}

/// Resolve a setting that is on unless it is turned off.
///
/// `MBX_VERIFY` reads its own environment variable as "set to anything but
/// zero", which can only express a default of off. A setting that defaults to
/// on has to be able to say no, and has to reject a value it cannot read
/// rather than guess which way the user meant it.
fn optional_bool(
    name: &str,
    get_env: &impl Fn(&str) -> Option<String>,
    from_file: Option<bool>,
) -> Result<Option<bool>> {
    let Some(value) = get_env(name) else {
        return Ok(from_file);
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(Some(true)),
        "0" | "false" | "no" | "off" | "" => Ok(Some(false)),
        other => Err(eyre::eyre!("invalid {name}: {other}")),
    }
}

fn optional_duration(
    name: &str,
    get_env: &impl Fn(&str) -> Option<String>,
    from_file: Option<&str>,
) -> Result<Option<Duration>> {
    match get_env(name) {
        Some(value) => parse_duration(&value)
            .map(Some)
            .wrap_err_with(|| format!("invalid {name}")),
        None => from_file.map(parse_duration).transpose(),
    }
}

fn read_config_file(path: &Path) -> Result<ConfigFile> {
    match std::fs::read_to_string(path) {
        Ok(contents) => {
            toml::from_str(&contents).wrap_err_with(|| format!("invalid {}", path.display()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(ConfigFile::default()),
        Err(error) => Err(error).wrap_err_with(|| format!("failed to read {}", path.display())),
    }
}

fn config_file_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("mbx").join("config.toml"))
}

fn default_cache_dir() -> Option<PathBuf> {
    dirs::cache_dir().map(|dir| dir.join("mbx"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(values: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let values: HashMap<String, String> = values
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        move |name: &str| values.get(name).cloned()
    }

    #[test]
    fn environment_overrides_the_file() {
        let file: ConfigFile = toml::from_str(
            r#"
            cache_dir = "/from/file"
            [remote]
            url = "https://file.example"
            namespace = "file"
            mode = "read-only"
            [http]
            timeout = "5s"
            retries = 9
            [gc]
            auto = false
            max_size = "1GiB"
            interval = "6h"
            [target]
            views = true
            root = "/from/file/targets"
            "#,
        )
        .unwrap();
        let config = Config::from_parts(
            file,
            env(&[
                ("MBX_CACHE_DIR", "/from/env"),
                ("MBX_REMOTE_URL", "https://env.example"),
                ("MBX_REMOTE_MODE", "write-only"),
                ("MBX_HTTP_TIMEOUT", "250ms"),
                ("MBX_GC_MAX_SIZE", "2GiB"),
                ("MBX_TARGET_ROOT", "/from/env/targets"),
            ]),
        )
        .unwrap();

        assert_eq!(config.cache_dir, PathBuf::from("/from/env"));
        assert_eq!(config.remote.url.unwrap(), "https://env.example");
        assert_eq!(config.remote.mode, RemoteCacheMode::WriteOnly);
        assert_eq!(config.http.timeout, Duration::from_millis(250));
        assert_eq!(config.gc.max_bytes, 2 * 1024 * 1024 * 1024);
        // Values absent from the environment still come from the file.
        assert_eq!(config.remote.namespace.unwrap(), "file");
        assert_eq!(config.http.retries, 9);
        assert!(!config.gc.auto);
        assert_eq!(config.gc.interval, Duration::from_secs(6 * 60 * 60));
        assert_eq!(config.target.root, PathBuf::from("/from/env/targets"));
        assert!(config.target.views);
    }

    #[test]
    fn defaults_apply_without_configuration() {
        let config = Config::from_parts(ConfigFile::default(), env(&[])).unwrap();
        assert_eq!(config.http.timeout, DEFAULT_HTTP_TIMEOUT);
        assert_eq!(config.http.download_timeout, DEFAULT_HTTP_DOWNLOAD_TIMEOUT);
        assert_eq!(config.http.retries, DEFAULT_HTTP_RETRIES);
        assert_eq!(config.remote.mode, RemoteCacheMode::ReadWrite);
        assert!(config.remote.url.is_none());
        assert!(!config.verify);
        assert!(!config.incremental);
        assert!(config.store_dir().ends_with("actions"));
        assert!(config.gc.auto, "collection runs until it is turned off");
        assert_eq!(config.gc.max_bytes, DEFAULT_GC_MAX_SIZE);
        assert_eq!(config.gc.interval, DEFAULT_GC_INTERVAL);
        assert!(
            !config.target.views,
            "moving where a build writes is opted into, never assumed"
        );
        assert_eq!(config.target.root, config.cache_dir.join("targets"));
    }

    #[test]
    fn rejects_unparseable_values() {
        assert!(
            Config::from_parts(
                ConfigFile::default(),
                env(&[("MBX_REMOTE_MODE", "sideways")])
            )
            .is_err()
        );
        assert!(
            Config::from_parts(ConfigFile::default(), env(&[("MBX_HTTP_TIMEOUT", "later")]))
                .is_err()
        );
        assert!(
            Config::from_parts(ConfigFile::default(), env(&[("MBX_HTTP_RETRIES", "many")]))
                .is_err()
        );
        assert!(
            Config::from_parts(ConfigFile::default(), env(&[("MBX_GC_MAX_SIZE", "lots")])).is_err()
        );
        assert!(
            Config::from_parts(ConfigFile::default(), env(&[("MBX_GC_INTERVAL", "later")]))
                .is_err()
        );
        // A budget that cannot be read must not be guessed at, and neither must
        // a switch: silently collecting to the wrong number, or not at all, is
        // worse than saying so.
        assert!(
            Config::from_parts(ConfigFile::default(), env(&[("MBX_GC_AUTO", "maybe")])).is_err()
        );
        assert!(
            Config::from_parts(
                ConfigFile::default(),
                env(&[("MBX_TARGET_VIEWS", "sometimes")])
            )
            .is_err()
        );
    }

    #[test]
    fn collection_runs_until_it_is_turned_off() {
        for value in ["0", "false", "no", "off", ""] {
            let config =
                Config::from_parts(ConfigFile::default(), env(&[("MBX_GC_AUTO", value)])).unwrap();
            assert!(!config.gc.auto, "MBX_GC_AUTO={value:?} should disable");
        }
        for value in ["1", "true", "yes", "on", "ON"] {
            let config =
                Config::from_parts(ConfigFile::default(), env(&[("MBX_GC_AUTO", value)])).unwrap();
            assert!(config.gc.auto, "MBX_GC_AUTO={value:?} should enable");
        }
    }

    #[test]
    fn reads_a_budget_in_either_unit_convention() {
        // `20GB` and `20GiB` are different numbers and sizes are reported back
        // in IEC, so both spellings have to mean exactly what they say.
        for (value, expected) in [
            ("20GiB", 20 * 1024 * 1024 * 1024),
            ("20GB", 20_000_000_000),
            ("1024", 1024),
        ] {
            let config =
                Config::from_parts(ConfigFile::default(), env(&[("MBX_GC_MAX_SIZE", value)]))
                    .unwrap();
            assert_eq!(config.gc.max_bytes, expected, "{value} should parse");
        }
    }

    #[test]
    fn the_environment_can_turn_off_what_the_file_turned_on() {
        let file: ConfigFile = toml::from_str("incremental = true").unwrap();
        let config = Config::from_parts(file.clone(), env(&[])).unwrap();
        assert!(config.incremental);
        let config = Config::from_parts(file, env(&[("MBX_INCREMENTAL", "0")])).unwrap();
        assert!(!config.incremental);
    }

    #[test]
    fn verify_is_off_for_empty_and_zero() {
        for value in ["", "0"] {
            let config =
                Config::from_parts(ConfigFile::default(), env(&[("MBX_VERIFY", value)])).unwrap();
            assert!(!config.verify, "MBX_VERIFY={value:?} should not enable");
        }
        let config =
            Config::from_parts(ConfigFile::default(), env(&[("MBX_VERIFY", "1")])).unwrap();
        assert!(config.verify);
    }
}
