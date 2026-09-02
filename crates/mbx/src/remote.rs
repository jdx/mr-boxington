//! Turning remote cache configuration into a client.
//!
//! The URL's scheme picks the backend: `https` reaches a cache server speaking
//! the mbx protocol, and `s3` reaches an object store directly. Both are built
//! here so that a build and `mbx doctor` cannot disagree about what a given
//! configuration means.

use crate::config::Config;
use eyre::{Context as _, Result, bail};
use mbx_cache_core::{
    RemoteCacheClient, RemoteCacheConfig, S3ConditionalWrites, S3Credentials, S3RemoteCacheConfig,
};
use url::Url;

/// What the AWS environment variables say, read once at the edge so that
/// everything below is a decision about configuration rather than about this
/// process's environment.
pub struct AwsEnvironment {
    /// Credentials, absent when the environment carries none.
    pub credentials: Option<S3Credentials>,
    /// Region named by `AWS_REGION` or `AWS_DEFAULT_REGION`.
    pub region: Option<String>,
}

impl AwsEnvironment {
    fn from_env() -> Self {
        Self {
            credentials: S3Credentials::from_env(),
            region: ["AWS_REGION", "AWS_DEFAULT_REGION"]
                .into_iter()
                .find_map(|name| {
                    std::env::var(name)
                        .ok()
                        .map(|region| region.trim().to_string())
                        .filter(|region| !region.is_empty())
                }),
        }
    }
}

/// Build the client the configuration names, or `None` when none is configured.
pub fn remote_client(config: &Config) -> Result<Option<RemoteCacheClient>> {
    remote_client_with(config, AwsEnvironment::from_env())
}

pub(crate) fn remote_client_with(
    config: &Config,
    aws: AwsEnvironment,
) -> Result<Option<RemoteCacheClient>> {
    let Some(url) = config
        .remote
        .url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
    else {
        return Ok(None);
    };
    let url: Url = url.parse().wrap_err("invalid remote cache URL")?;
    let namespace = namespace(config)?;
    if url.scheme() == "s3" {
        return s3_client(config, &url, namespace, aws).map(Some);
    }
    if config.remote.s3_endpoint.is_some()
        || config.remote.s3_region.is_some()
        || config.remote.s3_force_path_style.is_some()
        || config.remote.s3_conditional_writes != S3ConditionalWrites::default()
    {
        bail!("remote.s3_* settings apply to an s3:// remote cache URL, but remote.url is {url}");
    }
    Ok(Some(
        RemoteCacheClient::new(RemoteCacheConfig {
            base_url: url,
            namespace,
            token: config.remote.token.clone(),
            token_file: config.remote.token_file.clone(),
            oidc_audience: config.remote.oidc_audience.clone(),
            connect_timeout: config.http.timeout,
            read_timeout: config.http.timeout,
            download_timeout: config.http.download_timeout,
            retries: config.http.retries,
        })?
        .with_read_stall_budget(config.http.read_stall_budget),
    ))
}

/// The namespace, which isolates one project's cache and is always required.
fn namespace(config: &Config) -> Result<String> {
    config
        .remote
        .namespace
        .as_deref()
        .map(str::trim)
        .filter(|namespace| !namespace.is_empty())
        .map(str::to_string)
        .ok_or_else(|| eyre::eyre!("a remote cache namespace is required when a URL is set"))
}

/// Build a client for `s3://bucket[/prefix]`.
fn s3_client(
    config: &Config,
    url: &Url,
    namespace: String,
    aws: AwsEnvironment,
) -> Result<RemoteCacheClient> {
    // A bearer token or an OIDC audience authenticates to a cache server. An
    // object store authenticates with AWS credentials and would ignore them, so
    // a configuration naming both is a mistake worth reporting rather than
    // quietly half-honouring.
    if config.remote.token.is_some()
        || config.remote.token_file.is_some()
        || config.remote.oidc_audience.is_some()
    {
        bail!(
            "an s3:// remote cache authenticates with AWS credentials; \
             remove remote.token, remote.token_file, and remote.oidc_audience, and set \
             AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY instead"
        );
    }
    let bucket = url
        .host_str()
        .filter(|host| !host.is_empty())
        .ok_or_else(|| eyre::eyre!("an s3:// remote cache URL must name a bucket"))?
        .to_string();
    let endpoint = config
        .remote
        .s3_endpoint
        .as_deref()
        .map(str::trim)
        .filter(|endpoint| !endpoint.is_empty())
        .map(|endpoint| {
            endpoint
                .parse::<Url>()
                .wrap_err("invalid remote.s3_endpoint")
        })
        .transpose()?;
    if let Some(endpoint) = &endpoint {
        validate_endpoint(endpoint)?;
    }
    let Some(credentials) = aws.credentials else {
        bail!(
            "an s3:// remote cache needs AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY; \
             on GitHub Actions, aws-actions/configure-aws-credentials exports them from an \
             OIDC role assumption"
        );
    };
    RemoteCacheClient::new_s3(S3RemoteCacheConfig {
        bucket,
        prefix: url.path().to_string(),
        namespace,
        region: region(config, &aws.region, endpoint.is_some())?,
        endpoint,
        force_path_style: config.remote.s3_force_path_style,
        conditional_writes: config.remote.s3_conditional_writes,
        credentials,
        connect_timeout: config.http.timeout,
        read_timeout: config.http.timeout,
        download_timeout: config.http.download_timeout,
        retries: config.http.retries,
    })
}

/// The region requests are signed for.
///
/// A signature is scoped to a region whether or not the store has one, so it is
/// always needed. The AWS variables are consulted before giving up, since a
/// machine set up for the AWS tools has already answered this.
fn region(config: &Config, environment: &Option<String>, has_endpoint: bool) -> Result<String> {
    let configured = config
        .remote
        .s3_region
        .as_deref()
        .map(str::trim)
        .filter(|region| !region.is_empty())
        .map(str::to_string)
        .or_else(|| environment.clone());
    match configured {
        Some(region) => Ok(region),
        // A store reached through an endpoint usually has no region of its own,
        // and signs against whatever it is given.
        None if has_endpoint => Ok("us-east-1".to_string()),
        None => {
            bail!("an s3:// remote cache needs a region; set MBX_REMOTE_S3_REGION or AWS_REGION")
        }
    }
}

/// Refuse an endpoint that would carry credentials over plain HTTP.
///
/// The same rule the protocol client applies to its own URL: a signature and
/// the objects it fetches are readable in transit without TLS, and a developer
/// running MinIO on loopback is the one case where that does not matter.
fn validate_endpoint(endpoint: &Url) -> Result<()> {
    if endpoint.scheme() == "https" {
        return Ok(());
    }
    let loopback = endpoint.host().is_some_and(|host| match host {
        url::Host::Domain(host) => host.eq_ignore_ascii_case("localhost"),
        url::Host::Ipv4(address) => address.is_loopback(),
        url::Host::Ipv6(address) => address.is_loopback(),
    });
    if endpoint.scheme() == "http" && loopback {
        Ok(())
    } else {
        bail!("remote.s3_endpoint must use HTTPS except for loopback development servers")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RemoteSettings;

    fn aws() -> AwsEnvironment {
        AwsEnvironment {
            credentials: Some(S3Credentials {
                access_key_id: "AKIDEXAMPLE".into(),
                secret_access_key: "secret".into(),
                session_token: None,
            }),
            region: Some("us-west-2".into()),
        }
    }

    fn s3_remote() -> RemoteSettings {
        RemoteSettings {
            url: Some("s3://cache-bucket".into()),
            namespace: Some("acme".into()),
            ..RemoteSettings::default()
        }
    }

    /// The message a configuration is refused with. A client has no `Debug`,
    /// deliberately, so `unwrap_err` is not available here.
    fn refusal(remote: RemoteSettings, aws: AwsEnvironment) -> String {
        match client(remote, aws) {
            Err(error) => error.to_string(),
            Ok(_) => panic!("this configuration should have been refused"),
        }
    }

    fn client(remote: RemoteSettings, aws: AwsEnvironment) -> Result<Option<RemoteCacheClient>> {
        let directory = tempfile::tempdir().unwrap();
        remote_client_with(
            &Config {
                remote,
                ..Config::for_test(directory.path())
            },
            aws,
        )
    }

    #[test]
    fn an_s3_url_builds_an_object_store_client() {
        assert!(client(s3_remote(), aws()).unwrap().is_some());
    }

    #[test]
    fn no_remote_url_builds_no_client() {
        assert!(client(RemoteSettings::default(), aws()).unwrap().is_none());
    }

    #[test]
    fn a_remote_cache_always_needs_a_namespace() {
        let refusal = refusal(
            RemoteSettings {
                namespace: None,
                ..s3_remote()
            },
            aws(),
        );

        assert!(refusal.contains("namespace is required"));
    }

    #[test]
    fn an_s3_remote_without_credentials_says_which_variables_to_set() {
        let refusal = refusal(
            s3_remote(),
            AwsEnvironment {
                credentials: None,
                ..aws()
            },
        );

        assert!(refusal.contains("AWS_ACCESS_KEY_ID"));
    }

    #[test]
    fn an_s3_remote_falls_back_to_the_aws_environment_for_its_region() {
        let without_region = || AwsEnvironment {
            region: None,
            ..aws()
        };

        // Nothing names a region, and there is no endpoint to excuse it.
        assert!(refusal(s3_remote(), without_region()).contains("MBX_REMOTE_S3_REGION"));

        // The environment names one.
        assert!(client(s3_remote(), aws()).unwrap().is_some());

        // A store behind an endpoint signs against a default instead.
        assert!(
            client(
                RemoteSettings {
                    s3_endpoint: Some("http://127.0.0.1:9000".into()),
                    ..s3_remote()
                },
                without_region(),
            )
            .unwrap()
            .is_some()
        );
    }

    #[test]
    fn bearer_credentials_and_an_object_store_are_not_combined() {
        let refusal = refusal(
            RemoteSettings {
                token: Some("a-token".into()),
                ..s3_remote()
            },
            aws(),
        );

        assert!(refusal.contains("AWS credentials"));
    }

    #[test]
    fn s3_settings_on_a_protocol_url_are_refused() {
        // A setting that quietly does nothing is worse than one that is
        // refused, so every S3-only key is checked, including the one with a
        // default that makes its absence look like its presence.
        for remote in [
            RemoteSettings {
                s3_region: Some("us-west-2".into()),
                ..RemoteSettings::default()
            },
            RemoteSettings {
                s3_endpoint: Some("https://store.example.com".into()),
                ..RemoteSettings::default()
            },
            RemoteSettings {
                s3_force_path_style: Some(true),
                ..RemoteSettings::default()
            },
            RemoteSettings {
                s3_conditional_writes: S3ConditionalWrites::Required,
                ..RemoteSettings::default()
            },
        ] {
            let refusal = refusal(
                RemoteSettings {
                    url: Some("https://cache.example.com".into()),
                    namespace: Some("acme".into()),
                    ..remote
                },
                aws(),
            );
            assert!(refusal.contains("apply to an s3:// remote"), "{refusal}");
        }
    }

    #[test]
    fn a_protocol_url_is_accepted_with_the_s3_settings_left_alone() {
        assert!(
            client(
                RemoteSettings {
                    url: Some("https://cache.example.com".into()),
                    namespace: Some("acme".into()),
                    ..RemoteSettings::default()
                },
                aws(),
            )
            .unwrap()
            .is_some()
        );
    }

    #[test]
    fn a_plaintext_endpoint_is_refused_unless_it_is_loopback() {
        let endpoint = |endpoint: &str| {
            client(
                RemoteSettings {
                    s3_endpoint: Some(endpoint.into()),
                    ..s3_remote()
                },
                aws(),
            )
        };

        assert!(endpoint("http://127.0.0.1:9000").unwrap().is_some());
        assert!(endpoint("http://localhost:9000").unwrap().is_some());
        assert!(endpoint("https://store.example.com").unwrap().is_some());
        assert!(
            refusal(
                RemoteSettings {
                    s3_endpoint: Some("http://store.example.com".into()),
                    ..s3_remote()
                },
                aws(),
            )
            .contains("must use HTTPS")
        );
    }
}
