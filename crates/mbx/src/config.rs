//! Configuration, resolved from environment variables over an optional file.
//!
//! Precedence is environment, then the platform configuration file, then defaults.

use crate::util::parse_duration;
use bytesize::ByteSize;
use eyre::{Context, Result, bail};
use mbx_cache_core::RemoteCacheMode;
use std::path::PathBuf;
use std::time::Duration;
use usage_config::{EnvLayer, FileLayer, FileScope, Layers};

const DEFAULT_HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_HTTP_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);
const DEFAULT_HTTP_RETRIES: i64 = 3;
/// Stated in IEC units so the budget reads the way `mbx gc` reports it back.
const DEFAULT_GC_MAX_SIZE: u64 = 20 * 1024 * 1024 * 1024;
const DEFAULT_GC_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// The single declaration used to resolve and document mbx configuration.
#[derive(Debug, usage::Config)]
#[usage(file(
    path = "<config directory>/mbx/config.toml",
    scope = "global",
    format = "toml"
))]
pub(crate) struct RawConfig {
    /// Cache root.
    #[usage(env = "MBX_CACHE_DIR", default_note = "platform cache directory")]
    cache_dir: Option<PathBuf>,
    /// Write a JSON build report to this path.
    #[usage(env = "MBX_STATS_REPORT")]
    stats_report: Option<PathBuf>,
    /// Compile and consult the cache, then compare outputs.
    #[usage(env = "MBX_VERIFY", default = false, scope = "env")]
    verify: bool,
    /// Let local workspace members compile incrementally.
    #[usage(env = "MBX_INCREMENTAL", default = false)]
    incremental: bool,
    /// Share eligible compilations that read `OUT_DIR`.
    #[usage(env = "MBX_SHARE_OUT_DIR", default = false)]
    share_out_dir: bool,
    /// Append the full reason for every bypassed compilation to this path.
    #[usage(key = "bypass_log", env = "MBX_BYPASS_LOG", scope = "env")]
    _bypass_log: Option<PathBuf>,
    #[usage(flatten)]
    remote: RawRemote,
    #[usage(flatten)]
    http: RawHttp,
    #[usage(flatten)]
    gc: RawGc,
    #[usage(flatten)]
    target: RawTarget,
}

#[derive(Debug, usage::Config)]
#[usage(prefix = "remote")]
struct RawRemote {
    /// Remote cache URL.
    #[usage(env = "MBX_REMOTE_URL", ty = "url")]
    url: Option<String>,
    /// Remote namespace; required when a URL is configured.
    #[usage(env = "MBX_REMOTE_NAMESPACE")]
    namespace: Option<String>,
    /// Bearer token for the remote cache.
    #[usage(env = "MBX_REMOTE_TOKEN")]
    token: Option<String>,
    /// File containing a bearer token.
    #[usage(env = "MBX_REMOTE_TOKEN_FILE")]
    token_file: Option<PathBuf>,
    /// CI OIDC audience.
    #[usage(env = "MBX_REMOTE_OIDC_AUDIENCE")]
    oidc_audience: Option<String>,
    /// Remote access mode.
    #[usage(
        env = "MBX_REMOTE_MODE",
        default = "read-write",
        choices("read-write", "read-only", "write-only")
    )]
    mode: String,
}

#[derive(Debug, usage::Config)]
#[usage(prefix = "target")]
struct RawTarget {
    /// Let mbx place eligible target directories under the managed root.
    #[usage(env = "MBX_TARGET_VIEWS", default = true)]
    views: bool,
    /// Managed target root.
    #[usage(env = "MBX_TARGET_ROOT", default_note = "<cache_dir>/targets")]
    root: Option<PathBuf>,
}

#[derive(Debug, usage::Config)]
#[usage(prefix = "gc")]
struct RawGc {
    /// Sweep after a build when collection is due.
    #[usage(env = "MBX_GC_AUTO", default = true)]
    auto: bool,
    /// Action-store budget.
    #[usage(env = "MBX_GC_MAX_SIZE", default = "20GiB")]
    max_size: String,
    /// Minimum interval between automatic sweeps.
    #[usage(env = "MBX_GC_INTERVAL", default = "1h", ty = "duration")]
    interval: String,
}

#[derive(Debug, usage::Config)]
#[usage(prefix = "http")]
struct RawHttp {
    /// Connect and request timeout.
    #[usage(env = "MBX_HTTP_TIMEOUT", default = "30s", ty = "duration")]
    timeout: String,
    /// Blob download timeout.
    #[usage(env = "MBX_HTTP_DOWNLOAD_TIMEOUT", default = "10m", ty = "duration")]
    download_timeout: String,
    /// Request retries.
    #[usage(env = "MBX_HTTP_RETRIES", default = 3)]
    retries: i64,
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
        let env = EnvLayer::from_process();
        let file = config_file_path().map(|path| FileLayer::at(path, FileScope::Global));
        Self::from_layers(&env, file.as_ref())
    }

    fn from_layers(env: &EnvLayer, file: Option<&FileLayer>) -> Result<Self> {
        let mut layers = Layers::new().then(env);
        if let Some(file) = file {
            layers = layers.then(file);
        }
        let resolved = usage_config::resolve(RawConfig::SETTINGS_REGISTRY, layers)?;
        if !resolved.warnings.is_empty() {
            let problems = resolved
                .warnings
                .iter()
                .map(|warning| match &warning.origin {
                    Some(origin) => format!("{} ({})", warning.message, origin.describe()),
                    None => warning.message.clone(),
                })
                .collect::<Vec<_>>()
                .join("\n");
            bail!("invalid configuration:\n{problems}");
        }
        let raw = RawConfig::read(&resolved)?;
        Self::from_raw(raw)
    }

    fn from_raw(raw: RawConfig) -> Result<Self> {
        let cache_dir = raw.cache_dir.or_else(default_cache_dir).ok_or_else(|| {
            eyre::eyre!("could not determine a cache directory; set MBX_CACHE_DIR")
        })?;
        let mode = raw.remote.mode.parse().wrap_err("invalid remote.mode")?;
        let http = HttpSettings {
            timeout: parse_duration(&raw.http.timeout).wrap_err("invalid http.timeout")?,
            download_timeout: parse_duration(&raw.http.download_timeout)
                .wrap_err("invalid http.download_timeout")?,
            retries: raw.http.retries,
        };
        let gc = GcSettings {
            auto: raw.gc.auto,
            max_bytes: parse_byte_size(&raw.gc.max_size).wrap_err("invalid gc.max_size")?,
            interval: parse_duration(&raw.gc.interval).wrap_err("invalid gc.interval")?,
        };
        let target = TargetSettings {
            views: raw.target.views,
            root: match raw.target.root {
                Some(root) if root.is_absolute() => root,
                Some(root) => cache_dir.join(root),
                None => cache_dir.join("targets"),
            },
        };
        Ok(Self {
            cache_dir,
            gc,
            target,
            stats_report: raw.stats_report,
            verify: raw.verify,
            incremental: raw.incremental,
            share_out_dir: raw.share_out_dir,
            remote: RemoteSettings {
                url: raw.remote.url,
                namespace: raw.remote.namespace,
                token: raw.remote.token,
                token_file: raw.remote.token_file,
                oidc_audience: raw.remote.oidc_audience,
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

/// Resolve a byte size, written either plainly or with a unit.
///
/// Both IEC and SI spellings parse -- `20GiB` and `20GB` are different numbers,
/// and sizes are reported back in IEC, so the two are not interchangeable.
fn parse_byte_size(value: &str) -> Result<u64> {
    // `ByteSize`'s parse error is a bare `String`, so it cannot be a source.
    value
        .trim()
        .parse::<ByteSize>()
        .map(|size| size.as_u64())
        .map_err(|error| eyre::eyre!(error))
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

    fn configured(file: Option<&str>, values: &[(&str, &str)]) -> Result<Config> {
        let env = EnvLayer::new(
            values
                .iter()
                .map(|(name, value)| ((*name).to_string(), (*value).to_string())),
        );
        let directory = tempfile::tempdir().unwrap();
        let file = file.map(|contents| {
            let path = directory.path().join("config.toml");
            std::fs::write(&path, contents).unwrap();
            FileLayer::at(path, FileScope::Global)
        });
        Config::from_layers(&env, file.as_ref())
    }

    #[test]
    fn environment_overrides_the_file() {
        let file = r#"
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
            "#;
        let config = configured(
            Some(file),
            &[
                ("MBX_CACHE_DIR", "/from/env"),
                ("MBX_REMOTE_URL", "https://env.example"),
                ("MBX_REMOTE_MODE", "write-only"),
                ("MBX_HTTP_TIMEOUT", "250ms"),
                ("MBX_GC_MAX_SIZE", "2GiB"),
                ("MBX_TARGET_ROOT", "/from/env/targets"),
            ],
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
        let config = configured(None, &[]).unwrap();
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
            config.target.views,
            "eligible target directories are managed"
        );
        assert_eq!(config.target.root, config.cache_dir.join("targets"));
    }

    #[test]
    fn managed_targets_can_be_turned_off() {
        for value in ["0", "false", "no", "off", ""] {
            let config = configured(None, &[("MBX_TARGET_VIEWS", value)]).unwrap();
            assert!(
                !config.target.views,
                "MBX_TARGET_VIEWS={value:?} should disable managed targets"
            );
        }
    }

    #[test]
    fn relative_target_roots_are_anchored_to_the_cache_directory() {
        let config = configured(
            None,
            &[
                ("MBX_CACHE_DIR", "/cache"),
                ("MBX_TARGET_ROOT", "target-views"),
            ],
        )
        .unwrap();

        assert_eq!(config.target.root, PathBuf::from("/cache/target-views"));
    }

    #[test]
    fn rejects_unparseable_values() {
        assert!(configured(None, &[("MBX_REMOTE_MODE", "sideways")]).is_err());
        assert!(configured(None, &[("MBX_HTTP_TIMEOUT", "later")]).is_err());
        assert!(configured(None, &[("MBX_HTTP_RETRIES", "many")]).is_err());
        assert!(configured(None, &[("MBX_GC_MAX_SIZE", "lots")]).is_err());
        assert!(configured(None, &[("MBX_GC_INTERVAL", "later")]).is_err());
        // A budget that cannot be read must not be guessed at, and neither must
        // a switch: silently collecting to the wrong number, or not at all, is
        // worse than saying so.
        assert!(configured(None, &[("MBX_GC_AUTO", "maybe")]).is_err());
        assert!(configured(None, &[("MBX_TARGET_VIEWS", "sometimes")]).is_err());
    }

    #[test]
    fn collection_runs_until_it_is_turned_off() {
        for value in ["0", "false", "no", "off", ""] {
            let config = configured(None, &[("MBX_GC_AUTO", value)]).unwrap();
            assert!(!config.gc.auto, "MBX_GC_AUTO={value:?} should disable");
        }
        for value in ["1", "true", "yes", "on", "ON"] {
            let config = configured(None, &[("MBX_GC_AUTO", value)]).unwrap();
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
            let config = configured(None, &[("MBX_GC_MAX_SIZE", value)]).unwrap();
            assert_eq!(config.gc.max_bytes, expected, "{value} should parse");
        }
    }

    #[test]
    fn the_environment_can_turn_off_what_the_file_turned_on() {
        let file = "incremental = true";
        let config = configured(Some(file), &[]).unwrap();
        assert!(config.incremental);
        let config = configured(Some(file), &[("MBX_INCREMENTAL", "0")]).unwrap();
        assert!(!config.incremental);
    }

    #[test]
    fn unknown_file_keys_are_rejected() {
        let error = configured(Some("not_a_setting = true"), &[]).unwrap_err();
        assert!(error.to_string().contains("not_a_setting"), "{error}");
    }

    #[test]
    fn verify_is_off_for_empty_and_zero() {
        for value in ["", "0"] {
            let config = configured(None, &[("MBX_VERIFY", value)]).unwrap();
            assert!(!config.verify, "MBX_VERIFY={value:?} should not enable");
        }
        let config = configured(None, &[("MBX_VERIFY", "1")]).unwrap();
        assert!(config.verify);
    }

    #[test]
    fn the_usage_spec_declares_files_environment_and_defaults() {
        let spec = RawConfig::spec_kdl();
        assert!(spec.contains(r#"file "<config directory>/mbx/config.toml""#));
        assert!(spec.contains(r#"env "MBX_GC_MAX_SIZE""#));
        assert!(spec.contains(r#"prop "gc.max_size""#));
        assert!(spec.contains(r#"default="20GiB""#));
    }
}
