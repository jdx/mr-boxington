//! Turning remote cache configuration into a client.
//!
//! The URL's scheme picks the backend: `https` reaches a cache server speaking
//! the mbx protocol, and `s3` reaches an object store directly. Both are built
//! here so that a build and `mbx doctor` cannot disagree about what a given
//! configuration means.

use crate::config::Config;
use eyre::{Context as _, Result, bail};
use mbx_cache_core::{
    InstanceRoleCredentials, RemoteCacheClient, RemoteCacheConfig, S3ConditionalWrites,
    S3Credentials, S3RemoteCacheConfig,
};
use std::time::{Duration, SystemTime};
use url::Url;

/// What the AWS environment variables say, read once at the edge so that
/// everything below is a decision about configuration rather than about this
/// process's environment.
#[derive(Default)]
pub struct AwsEnvironment {
    /// Credentials, absent when neither the environment nor an instance role
    /// supplied any.
    pub credentials: Option<S3Credentials>,
    /// Region named by `AWS_REGION` or `AWS_DEFAULT_REGION`.
    pub region: Option<String>,
    /// The instance role `credentials` came from, and when they expire.
    /// Absent when the environment supplied them.
    pub instance_role: Option<(InstanceRoleCredentials, SystemTime)>,
    /// Why no instance role credentials were found, for the refusal message.
    pub instance_role_failure: Option<String>,
}

/// Where a remote's credentials came from, for `mbx doctor`.
pub enum CredentialOrigin {
    /// `AWS_ACCESS_KEY_ID` and its companions.
    Environment,
    /// The EC2 instance role, renewed before this instant.
    InstanceRole { expires_at: SystemTime },
}

impl CredentialOrigin {
    /// One line saying where the credentials are from and how long they last.
    pub fn describe(&self) -> String {
        match self {
            Self::Environment => "AWS_ACCESS_KEY_ID in the environment".to_string(),
            Self::InstanceRole { expires_at } => {
                match expires_at.duration_since(SystemTime::now()) {
                    Ok(left) => format!(
                        "EC2 instance role, expires in {}, renewed automatically",
                        format_remaining(left)
                    ),
                    Err(_) => "EC2 instance role, credentials have expired".to_string(),
                }
            }
        }
    }
}

fn format_remaining(left: Duration) -> String {
    let minutes = left.as_secs() / 60;
    match (minutes / 60, minutes % 60) {
        (0, minutes) => format!("{minutes}m"),
        (hours, minutes) => format!("{hours}h {minutes}m"),
    }
}

impl AwsEnvironment {
    fn from_env() -> Self {
        Self {
            credentials: S3Credentials::from_env(),
            instance_role: None,
            instance_role_failure: None,
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

/// A remote cache client and where its credentials came from.
pub struct ConnectedRemote {
    pub client: RemoteCacheClient,
    /// Absent for a cache server, which authenticates with a token instead.
    pub credentials: Option<CredentialOrigin>,
}

/// Build the client the configuration names, or `None` when none is configured.
pub async fn remote_client(config: &Config) -> Result<Option<RemoteCacheClient>> {
    Ok(connect(config).await?.map(|remote| remote.client))
}

/// Like [`remote_client`], also reporting where the credentials came from.
///
/// An `s3://` remote with no `AWS_ACCESS_KEY_ID` asks the EC2 metadata service
/// for an instance role before giving up, which is the only reason this is
/// async.
pub async fn connect(config: &Config) -> Result<Option<ConnectedRemote>> {
    let mut aws = AwsEnvironment::from_env();
    let is_s3 = config
        .remote
        .url
        .as_deref()
        .is_some_and(|url| url.trim_start().starts_with("s3://"));
    if is_s3 && aws.credentials.is_none() {
        aws.use_instance_role().await;
    }
    let credentials = match (&aws.credentials, &aws.instance_role) {
        _ if !is_s3 => None,
        (_, Some((_, expires_at))) => Some(CredentialOrigin::InstanceRole {
            expires_at: *expires_at,
        }),
        (Some(_), None) => Some(CredentialOrigin::Environment),
        (None, None) => None,
    };
    Ok(
        remote_client_with(config, aws)?.map(|client| ConnectedRemote {
            client,
            credentials,
        }),
    )
}

impl AwsEnvironment {
    /// Try the EC2 instance role, filling in credentials when it has some.
    async fn use_instance_role(&mut self) {
        let provider = match InstanceRoleCredentials::from_env() {
            Ok(Some(provider)) => provider,
            Ok(None) => {
                self.instance_role_failure =
                    Some("instance role lookup is disabled by AWS_EC2_METADATA_DISABLED".into());
                return;
            }
            Err(error) => {
                self.instance_role_failure = Some(format!("{error:#}"));
                return;
            }
        };
        self.fetch_instance_role(provider).await;
    }

    async fn fetch_instance_role(&mut self, provider: InstanceRoleCredentials) {
        match provider.fetch().await {
            Ok(fetched) => {
                self.credentials = Some(fetched.credentials);
                self.instance_role = Some((provider, fetched.expires_at));
            }
            Err(error) => self.instance_role_failure = Some(format!("{error:#}")),
        }
    }
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
        let instance_role = aws
            .instance_role_failure
            .map(|reason| format!(" No EC2 instance role was used: {reason}."))
            .unwrap_or_default();
        bail!(
            "an s3:// remote cache needs AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY, or an EC2 \
             instance role.{instance_role} On GitHub Actions, \
             aws-actions/configure-aws-credentials exports credentials from an OIDC role \
             assumption"
        );
    };
    let config = S3RemoteCacheConfig {
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
    };
    match aws.instance_role {
        Some((provider, expires_at)) => {
            RemoteCacheClient::new_s3_with_instance_role(config, provider, expires_at)
        }
        None => RemoteCacheClient::new_s3(config),
    }
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
            bail!(
                "an s3:// remote cache needs a region; set MBX_REMOTE_S3_REGION or AWS_REGION, or run `mbx settings set remote.s3_region <region>`"
            )
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
            ..AwsEnvironment::default()
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
    fn a_missing_instance_role_is_named_in_the_refusal() {
        let refusal = refusal(
            s3_remote(),
            AwsEnvironment {
                credentials: None,
                instance_role_failure: Some("the metadata service answered 404".into()),
                ..aws()
            },
        );

        assert!(refusal.contains("AWS_ACCESS_KEY_ID"));
        assert!(refusal.contains("EC2 instance role"));
        assert!(refusal.contains("the metadata service answered 404"));
    }

    #[tokio::test]
    async fn an_instance_role_supplies_credentials_the_environment_lacks() {
        let mut metadata = mockito::Server::new_async().await;
        metadata
            .mock("PUT", "/latest/api/token")
            .with_body("session-token")
            .create_async()
            .await;
        metadata
            .mock("GET", "/latest/meta-data/iam/security-credentials/")
            .with_body("build-runner")
            .create_async()
            .await;
        metadata
            .mock(
                "GET",
                "/latest/meta-data/iam/security-credentials/build-runner",
            )
            .with_body(
                r#"{"Code":"Success","AccessKeyId":"ASIAROLE","SecretAccessKey":"secret","Token":"token","Expiration":"2999-01-01T00:00:00Z"}"#,
            )
            .create_async()
            .await;
        let provider = InstanceRoleCredentials::new(metadata.url().parse().unwrap()).unwrap();
        let mut environment = AwsEnvironment {
            credentials: None,
            ..aws()
        };

        environment.fetch_instance_role(provider).await;

        let credentials = environment.credentials.as_ref().unwrap();
        assert_eq!(credentials.access_key_id, "ASIAROLE");
        assert!(environment.instance_role.is_some());
        assert!(client(s3_remote(), environment).unwrap().is_some());
    }

    #[tokio::test]
    async fn an_unreachable_metadata_service_leaves_the_refusal_to_explain() {
        let mut environment = AwsEnvironment {
            credentials: None,
            ..aws()
        };
        // Nothing listens on a port that was just released.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let provider =
            InstanceRoleCredentials::new(format!("http://127.0.0.1:{port}").parse().unwrap())
                .unwrap();

        environment.fetch_instance_role(provider).await;

        assert!(environment.credentials.is_none());
        assert!(environment.instance_role_failure.is_some());
    }

    #[test]
    fn the_credential_origin_says_when_an_instance_role_expires() {
        assert_eq!(
            CredentialOrigin::Environment.describe(),
            "AWS_ACCESS_KEY_ID in the environment"
        );
        let in_six_hours = CredentialOrigin::InstanceRole {
            expires_at: SystemTime::now() + Duration::from_secs(6 * 3_600 - 30),
        };
        assert_eq!(
            in_six_hours.describe(),
            "EC2 instance role, expires in 5h 59m, renewed automatically"
        );
        let lapsed = CredentialOrigin::InstanceRole {
            expires_at: SystemTime::now() - Duration::from_secs(1),
        };
        assert!(lapsed.describe().contains("expired"));
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
