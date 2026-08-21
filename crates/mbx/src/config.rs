//! Configuration, resolved from environment variables over an optional file.
//!
//! Precedence is environment, then `~/.config/mbx/config.toml`, then defaults.

use crate::util::parse_duration;
use eyre::{Context, Result};
use mbx_cache_core::RemoteCacheMode;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::Duration;

const DEFAULT_HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_HTTP_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);
const DEFAULT_HTTP_RETRIES: i64 = 3;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct ConfigFile {
    cache_dir: Option<PathBuf>,
    stats_report: Option<PathBuf>,
    incremental: Option<bool>,
    remote: RemoteFile,
    http: HttpFile,
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
    pub remote: RemoteSettings,
    pub http: HttpSettings,
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
        Ok(Self {
            cache_dir,
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
            ]),
        )
        .unwrap();

        assert_eq!(config.cache_dir, PathBuf::from("/from/env"));
        assert_eq!(config.remote.url.unwrap(), "https://env.example");
        assert_eq!(config.remote.mode, RemoteCacheMode::WriteOnly);
        assert_eq!(config.http.timeout, Duration::from_millis(250));
        // Values absent from the environment still come from the file.
        assert_eq!(config.remote.namespace.unwrap(), "file");
        assert_eq!(config.http.retries, 9);
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
