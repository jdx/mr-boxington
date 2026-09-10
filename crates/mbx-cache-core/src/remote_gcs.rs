//! The remote cache backed directly by a Google Cloud Storage bucket.
//!
//! This is the GCS backend behind [`crate::RemoteCacheClient`]. Similar to the
//! S3 backend, a GCS bucket stores objects: blobs and action results are
//! immutable and content-addressed, so they are written create-only, and the
//! task action manifest is updated using GCS generation-match preconditions.
//!
//! Authentication uses Google Cloud credentials resolved in order of priority:
//! 1. Explicit token (if configured)
//! 2. `GOOGLE_APPLICATION_CREDENTIALS` (token or file)
//! 3. GCP Instance Metadata server (`http://metadata.google.internal`)
//! 4. Local fallback via `gcloud auth application-default print-access-token`

use crate::{
    BlobPackReceipt, BlobSource, BlobUpload, CacheDigest, MAX_REMOTE_BLOB_BYTES,
    MAX_REMOTE_JSON_BYTES, ManifestPutOutcome, RemoteActionManifest, RemoteActionResult,
    RemoteBlobPack, TransientRequest, parse_strong_etag, read_bounded_json, retry_async,
};
use eyre::{Result, bail, eyre};
use log::warn;
use reqwest::StatusCode;
use reqwest::header::{AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE, ETAG, HeaderMap, HeaderValue};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use url::Url;

/// The object-key layout this client reads and writes.
const LAYOUT_VERSION: u8 = 1;
/// Object read to prove an endpoint answers, credentials work, and the bucket exists.
const CONNECTIVITY_PROBE_KEY: &str = "connectivity-probe";
/// Bytes of an error document read before giving up on a diagnosis.
const MAX_ERROR_BODY_BYTES: usize = 8 * 1024;
/// Default GCS API host.
const DEFAULT_GCS_HOST: &str = "storage.googleapis.com";
/// Default GCP Instance Metadata Server token endpoint.
const GCP_METADATA_TOKEN_URL: &str =
    "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token";

/// Connection, addressing, and credential settings for a GCS remote cache.
pub struct GcsRemoteCacheConfig {
    /// Bucket holding the cache.
    pub bucket: String,
    /// Key prefix within the bucket, empty for the bucket root.
    pub prefix: String,
    /// Namespace isolating one project's cache, used as a key prefix.
    pub namespace: String,
    /// Endpoint override for a non-Google or emulator store.
    pub endpoint: Option<Url>,
    /// Optional bearer token, if explicitly configured.
    pub token: Option<String>,
    /// Optional file containing a bearer token.
    pub token_file: Option<PathBuf>,
    /// Maximum time allowed to establish a connection.
    pub connect_timeout: Duration,
    /// Maximum time without response progress for ordinary requests.
    pub read_timeout: Duration,
    /// Deadline for one blob download, spanning every retry attempt.
    pub download_timeout: Duration,
    /// Number of attempts after the initial request for retryable failures.
    pub retries: i64,
}

/// Kinds of object the cache stores, which are also their key prefixes.
#[derive(Clone, Copy)]
enum ObjectKind {
    Blob,
    ActionResult,
    ActionManifest,
}

impl ObjectKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Blob => "blobs",
            Self::ActionResult => "action-results",
            Self::ActionManifest => "action-manifests",
        }
    }
}

pub(crate) struct GcsRemoteCache {
    client: reqwest::Client,
    bucket: String,
    /// Raw prefix covering the configured prefix, namespace, and layout version.
    root: String,
    endpoint: Url,
    token_provider: Arc<GcsTokenProvider>,
    absence_is_ambiguous: AtomicBool,
    download_timeout: Duration,
    retries: i64,
}

impl GcsRemoteCache {
    pub(crate) fn new(config: GcsRemoteCacheConfig) -> Result<Self> {
        validate_bucket(&config.bucket)?;
        let prefix = normalize_prefix(&config.prefix)?;
        validate_key_path(&config.namespace, "remote cache namespace")?;

        let client = reqwest::Client::builder()
            .connect_timeout(config.connect_timeout)
            .read_timeout(config.read_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .tcp_keepalive(Some(Duration::from_secs(60)))
            .pool_idle_timeout(Some(Duration::from_secs(90)))
            .build()?;

        let endpoint = match &config.endpoint {
            Some(endpoint) => endpoint.clone(),
            None => format!("https://{DEFAULT_GCS_HOST}").parse()?,
        };

        let token_provider = Arc::new(GcsTokenProvider::new(
            config.token.clone(),
            config.token_file.clone(),
            client.clone(),
        ));

        Ok(Self {
            client,
            bucket: config.bucket.trim().to_string(),
            root: format!("{prefix}{}/v{LAYOUT_VERSION}/", config.namespace.trim()),
            endpoint,
            token_provider,
            absence_is_ambiguous: AtomicBool::new(false),
            download_timeout: config.download_timeout,
            retries: config.retries,
        })
    }

    /// Construct the full key for an object kind and digest.
    fn object_key(&self, kind: ObjectKind, digest: &CacheDigest) -> Result<String> {
        digest.validate()?;
        if matches!(kind, ObjectKind::ActionResult | ObjectKind::ActionManifest)
            && digest.algorithm != "blake3"
        {
            bail!("remote cache action keys must use blake3");
        }
        Ok(format!(
            "{}{}/{}/{}/{}",
            self.root,
            kind.as_str(),
            digest.algorithm,
            digest.hash,
            digest.size
        ))
    }

    /// URL for GET/DELETE operations:
    /// `<endpoint>/storage/v1/b/<bucket>/o/<url-encoded-key>?alt=media`
    fn read_object_url(&self, key: &str) -> Result<Url> {
        let encoded_key = percent_encode_key(key);
        let path = format!("storage/v1/b/{}/o/{encoded_key}", self.bucket);
        let mut url = self.endpoint.join(&path)?;
        url.set_query(Some("alt=media"));
        Ok(url)
    }

    /// URL for simple upload:
    /// `<endpoint>/upload/storage/v1/b/<bucket>/o?uploadType=media&name=<raw-or-query-encoded-key>`
    fn upload_object_url(&self, key: &str, query_params: &[(&str, &str)]) -> Result<Url> {
        let path = format!("upload/storage/v1/b/{}/o", self.bucket);
        let mut url = self.endpoint.join(&path)?;
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("uploadType", "media");
        pairs.append_pair("name", key);
        for (k, v) in query_params {
            pairs.append_pair(k, v);
        }
        drop(pairs);
        Ok(url)
    }

    async fn authenticate_request(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::RequestBuilder> {
        let is_https = self.endpoint.scheme() == "https";
        let is_loopback = self.endpoint.host().is_some_and(|host| match host {
            url::Host::Domain(host) => host.eq_ignore_ascii_case("localhost"),
            url::Host::Ipv4(address) => address.is_loopback(),
            url::Host::Ipv6(address) => address.is_loopback(),
        });
        if (is_https || is_loopback)
            && let Some(token) = self.token_provider.get_token().await?
        {
            let mut value = HeaderValue::from_str(&format!("Bearer {token}"))?;
            value.set_sensitive(true);
            return Ok(request.header(AUTHORIZATION, value));
        }
        Ok(request)
    }

    pub(crate) async fn check_connection(&self) -> Result<()> {
        let key = format!("{}{CONNECTIVITY_PROBE_KEY}", self.root);
        let url = self.read_object_url(&key)?;
        retry_async("GET", &url, self.retries, || async {
            let request = self.client.get(url.clone());
            let request = self.authenticate_request(request).await?;
            let response = request.send().await?;
            match response.status() {
                StatusCode::OK | StatusCode::NOT_FOUND => Ok(()),
                StatusCode::FORBIDDEN | StatusCode::UNAUTHORIZED => {
                    let failure = FailedRequest::read(response).await;
                    if failure.is_credentials_rejected() {
                        bail!(
                            "the remote GCS bucket rejected credentials for {url}: {}. \
                             Check Google Application Default Credentials or token configuration",
                            failure.message.as_deref().unwrap_or("forbidden")
                        );
                    }
                    warn!(
                        "the remote GCS bucket did not confirm read access to {url}. \
                         If the cache never hits, these credentials may not be allowed to read the bucket"
                    );
                    Ok(())
                }
                _ => Err(FailedRequest::read(response).await.report("connect to", &url)),
            }
        })
        .await
    }

    fn reads_as_absent(&self, failure: &FailedRequest) -> bool {
        if failure.status == StatusCode::NOT_FOUND {
            return true;
        }
        if (failure.status != StatusCode::FORBIDDEN && failure.status != StatusCode::UNAUTHORIZED)
            || failure.is_credentials_rejected()
        {
            return false;
        }
        if !self.absence_is_ambiguous.swap(true, Ordering::Relaxed) {
            warn!(
                "the remote GCS bucket refused a read instead of reporting the object absent. \
                 Treating it as a cache miss."
            );
        }
        true
    }

    pub(crate) async fn get_blob(
        &self,
        digest: &CacheDigest,
        _media_type: &'static str,
    ) -> Result<Vec<u8>> {
        if digest.size > MAX_REMOTE_JSON_BYTES {
            bail!(
                "remote cache in-memory blob declared {} bytes, over the {} byte limit",
                digest.size,
                MAX_REMOTE_JSON_BYTES
            );
        }
        let key = self.object_key(ObjectKind::Blob, digest)?;
        let url = self.read_object_url(&key)?;
        retry_async("GET", &url, self.retries, || async {
            let request = self.client.get(url.clone());
            let request = self.authenticate_request(request).await?;
            let response = request.send().await?;
            if !response.status().is_success() {
                let failure = FailedRequest::read(response).await;
                return Err(failure.report("read", &url));
            }
            let mut response = response;
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await? {
                if bytes.len() as u64 + chunk.len() as u64 > digest.size {
                    bail!("remote cache blob exceeded the size of its digest");
                }
                bytes.extend_from_slice(&chunk);
            }
            if !digest.matches_bytes(&bytes)? {
                bail!("remote cache blob failed digest verification");
            }
            Ok(bytes)
        })
        .await
    }

    pub(crate) async fn get_blob_file(
        &self,
        digest: &CacheDigest,
        staging_dir: &Path,
    ) -> Result<tempfile::NamedTempFile> {
        if digest.size > MAX_REMOTE_BLOB_BYTES {
            bail!(
                "remote cache blob declared {} bytes, over the {} byte limit",
                digest.size,
                MAX_REMOTE_BLOB_BYTES
            );
        }
        let key = self.object_key(ObjectKind::Blob, digest)?;
        let url = self.read_object_url(&key)?;
        let download = retry_async("GET", &url, self.retries, || async {
            let request = self.client.get(url.clone());
            let request = self.authenticate_request(request).await?;
            let response = request.send().await?;
            if !response.status().is_success() {
                let failure = FailedRequest::read(response).await;
                return Err(failure.report("read", &url));
            }
            let mut response = response;
            fs::create_dir_all(staging_dir)?;
            let temporary = tempfile::NamedTempFile::new_in(staging_dir)?;
            let mut output = tokio::fs::File::from_std(temporary.reopen()?);
            let mut hasher = StreamHasher::for_digest(digest)?;
            let mut written = 0u64;
            while let Some(chunk) = response.chunk().await? {
                written += chunk.len() as u64;
                if written > digest.size {
                    bail!("remote cache blob exceeded the size of its digest");
                }
                hasher.update(&chunk);
                output.write_all(&chunk).await?;
            }
            output.flush().await?;
            drop(output);
            if !hasher.matches(&digest.hash) {
                bail!("remote cache blob failed digest verification");
            }
            Ok(temporary)
        });
        let download_timeout = self.download_timeout;
        tokio::time::timeout(download_timeout, download)
            .await
            .map_err(|_| {
                eyre!(
                    "remote cache blob download for {url} exceeded its {download_timeout:?} budget across all attempts"
                )
            })?
    }

    pub(crate) async fn get_action_result(
        &self,
        action: &CacheDigest,
    ) -> Result<Option<RemoteActionResult>> {
        let key = self.object_key(ObjectKind::ActionResult, action)?;
        let url = self.read_object_url(&key)?;
        let result = retry_async("GET", &url, self.retries, || async {
            let request = self.client.get(url.clone());
            let request = self.authenticate_request(request).await?;
            let response = request.send().await?;
            if !response.status().is_success() {
                let failure = FailedRequest::read(response).await;
                return if self.reads_as_absent(&failure) {
                    Ok(None)
                } else {
                    Err(failure.report("read", &url))
                };
            }
            let bytes = read_bounded_json(response, "action result").await?;
            Ok(Some(serde_json::from_slice::<RemoteActionResult>(&bytes)?))
        })
        .await?;
        if let Some(result) = &result
            && (result.version != 1 || result.action != *action)
        {
            bail!("remote action result does not match requested action");
        }
        Ok(result)
    }

    pub(crate) async fn put_action_result(&self, result: &RemoteActionResult) -> Result<()> {
        let key = self.object_key(ObjectKind::ActionResult, &result.action)?;
        let body = serde_json::to_vec(result)?;
        let url = self.upload_object_url(&key, &[("ifGenerationMatch", "0")])?;
        retry_async("POST", &url, self.retries, || async {
            let request = self
                .client
                .post(url.clone())
                .header(CONTENT_TYPE, "application/json")
                .header(CONTENT_LENGTH, body.len())
                .body(body.clone());
            let request = self.authenticate_request(request).await?;
            let response = request.send().await?;
            let status = response.status();
            if status.is_success() || status == StatusCode::PRECONDITION_FAILED {
                // Success or already exists
                return Ok(());
            }
            Err(FailedRequest::read(response).await.report("store", &url))
        })
        .await
    }

    pub(crate) async fn get_action_manifest(
        &self,
        key: &CacheDigest,
    ) -> Result<Option<RemoteActionManifest>> {
        let object_key = self.object_key(ObjectKind::ActionManifest, key)?;
        let url = self.read_object_url(&object_key)?;
        retry_async("GET", &url, self.retries, || async {
            let request = self.client.get(url.clone());
            let request = self.authenticate_request(request).await?;
            let response = request.send().await?;
            if !response.status().is_success() {
                let failure = FailedRequest::read(response).await;
                return if self.reads_as_absent(&failure) {
                    Ok(None)
                } else {
                    Err(failure.report("read", &url))
                };
            }
            // GCS provides ETag in header, or generation header `x-goog-generation`.
            // GCS ETag format is strong or quoted. parse_strong_etag validates strong etag.
            let etag = match parse_strong_etag(response.headers().get(ETAG)) {
                Ok(etag) => etag,
                Err(_) => {
                    // Fall back to x-goog-generation if ETag is weak or missing
                    if let Some(generation) = response
                        .headers()
                        .get("x-goog-generation")
                        .and_then(|v| v.to_str().ok())
                    {
                        generation.to_string()
                    } else {
                        bail!(
                            "remote action manifest response is missing a valid ETag or generation"
                        );
                    }
                }
            };
            let bytes = read_bounded_json(response, "action manifest").await?;
            Ok(Some(RemoteActionManifest { bytes, etag }))
        })
        .await
    }

    pub(crate) async fn put_action_manifest(
        &self,
        key: &CacheDigest,
        bytes: &[u8],
        expected_etag: Option<&str>,
    ) -> Result<ManifestPutOutcome> {
        let object_key = self.object_key(ObjectKind::ActionManifest, key)?;
        let body = bytes.to_vec();

        // GCS uses ifGenerationMatch:
        // if expected_etag is None: ifGenerationMatch=0 (only create if absent)
        // if expected_etag is Some(gen_or_etag): ifGenerationMatch=<generation>
        // Note: in GCS, generation is a positive u64 integer representing the object's generation.
        // If expected_etag is an ETag or generation string, we determine whether it is numeric or ifMetagenerationMatch.
        // If expected_etag is numeric, pass ifGenerationMatch. If not, pass header If-Match.
        retry_async("POST", &self.endpoint, self.retries, || async {
            let mut query_params = Vec::new();
            let mut if_match_header = None;

            match expected_etag {
                None => {
                    query_params.push(("ifGenerationMatch", "0"));
                }
                Some(etag) => {
                    if etag.chars().all(|c| c.is_ascii_digit()) {
                        query_params.push(("ifGenerationMatch", etag));
                    } else {
                        if_match_header = Some(if etag.starts_with('"') && etag.ends_with('"') {
                            etag.to_string()
                        } else {
                            format!("\"{etag}\"")
                        });
                    }
                }
            }

            let url = self.upload_object_url(&object_key, &query_params)?;
            let mut request = self
                .client
                .post(url.clone())
                .header(CONTENT_TYPE, "application/json")
                .header(CONTENT_LENGTH, body.len())
                .body(body.clone());

            if let Some(ref header_val) = if_match_header {
                request = request.header("if-match", header_val.as_str());
            }

            let request = self.authenticate_request(request).await?;
            let response = request.send().await?;
            let status = response.status();

            if status.is_success() {
                return Ok(ManifestPutOutcome::Stored);
            }
            if status == StatusCode::PRECONDITION_FAILED {
                return Ok(ManifestPutOutcome::PreconditionFailed);
            }
            let failure = FailedRequest::read(response).await;
            Err(failure.report("update", &url))
        })
        .await
    }

    pub(crate) async fn put_blob(&self, upload: &BlobUpload) -> Result<()> {
        upload.digest.validate()?;
        match &upload.source {
            BlobSource::Bytes(bytes) => {
                if !upload.digest.matches_bytes(bytes)? {
                    bail!("source bytes do not match expected digest");
                }
            }
            BlobSource::File(file) => {
                if !upload.digest.matches_file(file.path())? {
                    bail!("source file does not match expected digest");
                }
            }
            BlobSource::Path(path) => {
                if !upload.digest.matches_file(path)? {
                    bail!(
                        "source file at {} does not match expected digest",
                        path.display()
                    );
                }
            }
        }
        let key = self.object_key(ObjectKind::Blob, &upload.digest)?;
        let url = self.upload_object_url(&key, &[("ifGenerationMatch", "0")])?;
        retry_async("POST", &url, self.retries, || async {
            match &upload.source {
                BlobSource::Bytes(bytes) => {
                    let request = self
                        .client
                        .post(url.clone())
                        .header(CONTENT_TYPE, "application/octet-stream")
                        .header(CONTENT_LENGTH, bytes.len())
                        .body(bytes.clone());
                    let request = self.authenticate_request(request).await?;
                    let response = request.send().await?;
                    let status = response.status();
                    if status.is_success() || status == StatusCode::PRECONDITION_FAILED {
                        Ok(())
                    } else {
                        Err(FailedRequest::read(response).await.report("store", &url))
                    }
                }
                BlobSource::File(file) => self.put_blob_file(&url, file.path()).await,
                BlobSource::Path(path) => self.put_blob_file(&url, path).await,
            }
        })
        .await
    }

    async fn put_blob_file(&self, url: &Url, path: &Path) -> Result<()> {
        let file = tokio::fs::File::open(path).await?;
        let length = file.metadata().await?.len();
        let request = self
            .client
            .post(url.clone())
            .header(CONTENT_TYPE, "application/octet-stream")
            .header(CONTENT_LENGTH, length)
            .body(reqwest::Body::wrap_stream(
                tokio_util::io::ReaderStream::new(file),
            ));
        let request = self.authenticate_request(request).await?;
        let response = request.send().await?;
        let status = response.status();
        if status.is_success() || status == StatusCode::PRECONDITION_FAILED {
            Ok(())
        } else {
            Err(FailedRequest::read(response).await.report("store", url))
        }
    }

    // Unsupported object store extensions (same as S3)
    pub(crate) async fn get_action_results(
        &self,
        actions: &[CacheDigest],
    ) -> Result<Option<Vec<RemoteActionResult>>> {
        Ok(actions.is_empty().then(Vec::new))
    }

    pub(crate) async fn action_batch_limit(&self) -> Result<Option<usize>> {
        Ok(None)
    }

    pub(crate) async fn get_blob_pack(
        &self,
        _digests: &[CacheDigest],
        _staging_dir: &Path,
    ) -> Result<Option<RemoteBlobPack>> {
        Ok(None)
    }

    pub(crate) async fn get_blob_pack_with_limit(
        &self,
        _digests: &[CacheDigest],
        _staging_dir: &Path,
        _max_bytes: u64,
    ) -> Result<Option<RemoteBlobPack>> {
        Ok(None)
    }

    pub(crate) async fn blob_pack_upload_limits(&self) -> Result<Option<crate::BlobPackLimits>> {
        Ok(None)
    }

    pub(crate) async fn put_blob_pack(
        &self,
        uploads: &[BlobUpload],
    ) -> Result<Option<BlobPackReceipt>> {
        Ok(uploads.is_empty().then_some(BlobPackReceipt {
            created: 0,
            existing: 0,
        }))
    }
}

/// Incremental stream hasher supporting blake3 and sha256 to avoid re-reading downloaded blobs from disk.
enum StreamHasher {
    Blake3(Box<blake3::Hasher>),
    Sha256(sha2::Sha256),
}

impl StreamHasher {
    fn for_digest(digest: &CacheDigest) -> Result<Self> {
        match digest.algorithm.as_str() {
            "blake3" => Ok(Self::Blake3(Box::new(blake3::Hasher::new()))),
            "sha256" => Ok(Self::Sha256(<sha2::Sha256 as sha2::Digest>::new())),
            other => bail!("unsupported digest algorithm {other:?}"),
        }
    }

    fn update(&mut self, bytes: &[u8]) {
        match self {
            Self::Blake3(hasher) => {
                hasher.update(bytes);
            }
            Self::Sha256(hasher) => {
                <sha2::Sha256 as sha2::Digest>::update(hasher, bytes);
            }
        }
    }

    fn matches(self, expected: &str) -> bool {
        match self {
            Self::Blake3(hasher) => hasher.finalize().to_hex().as_str() == expected,
            Self::Sha256(hasher) => {
                hex::encode(<sha2::Sha256 as sha2::Digest>::finalize(hasher)) == expected
            }
        }
    }
}

/// Token cache structure.
struct CachedToken {
    token: String,
    expires_at: Instant,
}

/// Resolves Google Cloud access tokens.
struct GcsTokenProvider {
    static_token: Option<String>,
    token_file: Option<PathBuf>,
    client: reqwest::Client,
    cached: Mutex<Option<CachedToken>>,
}

impl GcsTokenProvider {
    fn new(
        static_token: Option<String>,
        token_file: Option<PathBuf>,
        client: reqwest::Client,
    ) -> Self {
        Self {
            static_token,
            token_file,
            client,
            cached: Mutex::new(None),
        }
    }

    async fn get_token(&self) -> Result<Option<String>> {
        // 1. Explicitly configured token
        if let Some(token) = &self.static_token {
            let token = token.trim();
            if !token.is_empty() {
                return Ok(Some(token.to_string()));
            }
        }

        // 2. Explicitly configured token file
        if let Some(path) = &self.token_file {
            let content = tokio::fs::read_to_string(path).await.map_err(|err| {
                eyre!(
                    "failed to read remote GCS token file {}: {err}",
                    path.display()
                )
            })?;
            let token = content.trim();
            if token.starts_with('{') {
                bail!(
                    "remote GCS token file {} contains a JSON service account key. \
                     Set GOOGLE_APPLICATION_CREDENTIALS or configure a bearer token",
                    path.display()
                );
            }
            if !token.is_empty() {
                return Ok(Some(token.to_string()));
            }
        }

        // 3. In-memory cached token (if still valid for at least 30 seconds)
        {
            let lock = self.cached.lock().await;
            if let Some(cached) = &*lock
                && cached.expires_at > Instant::now() + Duration::from_secs(30)
            {
                return Ok(Some(cached.token.clone()));
            }
        }

        // 4. Resolve via environment or metadata server or gcloud
        if let Some((token, expiry)) = self.resolve_credential().await? {
            let mut lock = self.cached.lock().await;
            *lock = Some(CachedToken {
                token: token.clone(),
                expires_at: expiry,
            });
            return Ok(Some(token));
        }

        Ok(None)
    }

    async fn resolve_credential(&self) -> Result<Option<(String, Instant)>> {
        // Priority A: GOOGLE_APPLICATION_CREDENTIALS environment variable
        if let Ok(path_str) = std::env::var("GOOGLE_APPLICATION_CREDENTIALS") {
            let path = PathBuf::from(path_str.trim());
            if path.exists() {
                // If it's a token file (starts with ya29 or single line plain text) or JSON file
                let content = tokio::fs::read_to_string(&path).await?;
                let content = content.trim();
                if !content.starts_with('{') && !content.is_empty() {
                    return Ok(Some((
                        content.to_string(),
                        Instant::now() + Duration::from_secs(3600),
                    )));
                }
            }
        }

        // Priority B: GCP Instance Metadata server
        if let Some(token_info) = self.query_metadata_server().await {
            return Ok(Some(token_info));
        }

        // Priority C: gcloud CLI fallback
        if let Some(token) = self.run_gcloud_auth_token().await {
            return Ok(Some((token, Instant::now() + Duration::from_secs(300))));
        }

        Ok(None)
    }

    async fn query_metadata_server(&self) -> Option<(String, Instant)> {
        let metadata_url = std::env::var("GCP_METADATA_SERVER_URL")
            .unwrap_or_else(|_| GCP_METADATA_TOKEN_URL.to_string());

        let mut headers = HeaderMap::new();
        headers.insert("Metadata-Flavor", HeaderValue::from_static("Google"));

        let response = self
            .client
            .get(&metadata_url)
            .headers(headers)
            .timeout(Duration::from_millis(1500))
            .send()
            .await
            .ok()?;

        if !response.status().is_success() {
            return None;
        }

        #[derive(Deserialize)]
        struct MetadataTokenResponse {
            access_token: String,
            #[serde(default = "default_expires_in")]
            expires_in: u64,
        }

        fn default_expires_in() -> u64 {
            3599
        }

        let resp: MetadataTokenResponse = response.json().await.ok()?;
        let duration = Duration::from_secs(resp.expires_in.max(60));
        Some((resp.access_token, Instant::now() + duration))
    }

    async fn run_gcloud_auth_token(&self) -> Option<String> {
        let output = tokio::task::spawn_blocking(|| {
            std::process::Command::new("gcloud")
                .args(["auth", "application-default", "print-access-token"])
                .output()
        })
        .await
        .ok()?
        .ok()?;

        if output.status.success() {
            let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !token.is_empty() {
                return Some(token);
            }
        }
        None
    }
}

/// Failed request interpretation.
struct FailedRequest {
    status: StatusCode,
    message: Option<String>,
}

impl FailedRequest {
    async fn read(mut response: reqwest::Response) -> Self {
        let status = response.status();
        let mut body = Vec::new();
        while body.len() < MAX_ERROR_BODY_BYTES {
            match response.chunk().await {
                Ok(Some(chunk)) => body.extend_from_slice(&chunk),
                Ok(None) | Err(_) => break,
            }
        }
        body.truncate(MAX_ERROR_BODY_BYTES);
        let text = String::from_utf8_lossy(&body);
        let message = error_message(&text);
        Self { status, message }
    }

    fn is_credentials_rejected(&self) -> bool {
        self.status == StatusCode::UNAUTHORIZED
            || (self.status == StatusCode::FORBIDDEN
                && self.message.as_deref().is_some_and(|msg| {
                    msg.contains("Invalid Credentials")
                        || msg.contains("Token")
                        || msg.contains("Signature")
                }))
    }

    fn is_retryable(&self) -> bool {
        matches!(self.status.as_u16(), 408 | 429 | 500 | 502 | 503 | 504)
    }

    fn report(&self, verb: &str, url: &Url) -> eyre::Report {
        let detail = match &self.message {
            Some(msg) => format!("failed to {verb} {url}: {} ({msg})", self.status),
            None => format!("failed to {verb} {url}: {}", self.status),
        };
        if self.is_retryable() {
            return eyre::Report::new(TransientRequest("the store asked to be retried"))
                .wrap_err(detail);
        }
        eyre!(detail)
    }
}

/// Extract error message from GCS error response (JSON or XML or plain).
fn error_message(body: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct GcsJsonError {
        error: GcsErrorDetail,
    }
    #[derive(Deserialize)]
    struct GcsErrorDetail {
        message: String,
    }

    if let Ok(parsed) = serde_json::from_str::<GcsJsonError>(body) {
        return Some(parsed.error.message);
    }

    if let Some(start) = body.find("<Message>") {
        let start = start + "<Message>".len();
        if let Some(end) = body[start..].find("</Message>") {
            return Some(body[start..start + end].trim().to_string());
        }
    }

    let trimmed = body.trim();
    if !trimmed.is_empty() && trimmed.len() < 256 {
        Some(trimmed.to_string())
    } else {
        None
    }
}

/// Percent-encode a GCS object key for URL path inclusion (`storage/v1/b/<bucket>/o/<key>`).
/// All characters other than alphanumeric and `-._~` are percent-encoded (including `/`).
fn percent_encode_key(key: &str) -> String {
    let mut encoded = String::with_capacity(key.len() * 3);
    for byte in key.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// Reject a bucket name that could not address the store it names.
fn validate_bucket(bucket: &str) -> Result<()> {
    let bucket = bucket.trim();
    if bucket.is_empty() {
        bail!("a GCS remote cache needs a bucket");
    }
    if bucket.contains('/') || bucket.starts_with('.') || bucket.ends_with('.') {
        bail!("invalid GCS bucket name {bucket:?}");
    }
    Ok(())
}

/// Normalize a key prefix to empty, or to something ending in a single `/`.
fn normalize_prefix(prefix: &str) -> Result<String> {
    let prefix = prefix.trim().trim_matches('/');
    if prefix.is_empty() {
        return Ok(String::new());
    }
    validate_key_path(prefix, "remote cache prefix")?;
    Ok(format!("{prefix}/"))
}

/// Reject anything that would not survive being spliced into an object key.
fn validate_key_path(value: &str, what: &str) -> Result<()> {
    let value = value.trim();
    if value.is_empty() {
        bail!("{what} must not be empty");
    }
    if value.starts_with('/') || value.ends_with('/') {
        bail!("{what} {value:?} must not start or end with a slash");
    }
    for segment in value.split('/') {
        if segment.is_empty() {
            bail!("{what} {value:?} must not contain an empty path segment");
        }
        if segment == "." || segment == ".." {
            bail!("{what} {value:?} must not contain a relative path segment");
        }
        if !segment
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            bail!(
                "{what} {value:?} must use only letters, digits, '.', '_', '-', and '/' \
                 when the remote cache is an object store"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "remote_gcs_tests.rs"]
mod tests;
