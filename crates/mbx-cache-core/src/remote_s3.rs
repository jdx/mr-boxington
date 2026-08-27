//! The remote cache backed directly by an S3-compatible object store.
//!
//! This is the other backend behind [`crate::RemoteCacheClient`]. Where the
//! protocol server answers lookups, validates uploads, and advertises
//! extensions, a bucket only stores objects. The v1 protocol is designed for
//! that: blobs and action results are immutable and content-addressed, so they
//! are written create-only and re-storing one is not an error, and the single
//! record that is updated in place -- the task action manifest -- carries an
//! entity tag that S3's conditional writes can honour.
//!
//! What a bucket cannot do it declines rather than approximates. Blob packs,
//! batched lookups, and negotiated compression all report themselves absent,
//! which is the same answer the client already handles from a server that does
//! not implement them.

use crate::sigv4::{PayloadHash, S3Credentials, SigningContext, sign};
use crate::{
    BlobPackReceipt, BlobSource, BlobUpload, CacheDigest, MAX_REMOTE_BLOB_BYTES,
    MAX_REMOTE_JSON_BYTES, ManifestPutOutcome, RemoteActionManifest, RemoteActionResult,
    RemoteBlobPack, TransientRequest, parse_strong_etag, quoted_etag, read_bounded_json,
    retry_async,
};
use eyre::{Result, bail, eyre};
use log::warn;
use reqwest::StatusCode;
use reqwest::header::{CONTENT_LENGTH, ETAG, IF_MATCH, IF_NONE_MATCH};
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};
use tokio::io::AsyncWriteExt;
use url::Url;

/// The object-key layout this client reads and writes.
///
/// Independent of the protocol version, which describes a wire format rather
/// than a bucket layout, though today they are both 1.
const LAYOUT_VERSION: u8 = 1;
/// Object read to prove an endpoint answers, credentials work, and the bucket
/// exists. It is never written, so a diagnostic stays read-only.
const CONNECTIVITY_PROBE_KEY: &str = "connectivity-probe";
/// Bytes of an S3 error document read before giving up on a diagnosis.
const MAX_ERROR_BODY_BYTES: usize = 8 * 1024;

/// Whether conditional writes are required, refused, or tried and given up on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, strum::EnumString, strum::Display)]
#[strum(serialize_all = "kebab-case")]
pub enum S3ConditionalWrites {
    /// Use conditional writes, and stop using them if the store rejects them.
    #[default]
    Auto,
    /// Require conditional writes, failing if the store does not implement them.
    Required,
    /// Never send conditional headers.
    Off,
}

/// Connection, addressing, and credential settings for an S3 remote cache.
pub struct S3RemoteCacheConfig {
    /// Bucket holding the cache.
    pub bucket: String,
    /// Key prefix within the bucket, empty for the bucket root.
    pub prefix: String,
    /// Namespace isolating one project's cache, used as a key prefix.
    pub namespace: String,
    /// Region used to sign requests.
    pub region: String,
    /// Endpoint override for a non-AWS store such as MinIO or R2.
    pub endpoint: Option<Url>,
    /// Force bucket-in-path addressing, overriding what the endpoint implies.
    pub force_path_style: Option<bool>,
    /// How to treat a store that does not implement conditional writes.
    pub conditional_writes: S3ConditionalWrites,
    /// Credentials used to sign requests.
    pub credentials: S3Credentials,
    /// Maximum time allowed to establish a connection.
    pub connect_timeout: Duration,
    /// Maximum time without response progress for ordinary requests.
    pub read_timeout: Duration,
    /// Overall deadline for an individual blob download attempt.
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

pub(crate) struct S3RemoteCache {
    client: reqwest::Client,
    /// Bucket root, always ending in `/` so keys join onto it.
    base_url: Url,
    /// Key prefix covering the configured prefix, namespace, and layout version.
    root: String,
    region: String,
    credentials: S3Credentials,
    conditional_writes: S3ConditionalWrites,
    /// Latched once a store has told us it does not implement conditional
    /// writes, so the rest of the session stops asking.
    conditionals_disabled: AtomicBool,
    download_timeout: Duration,
    retries: i64,
}

impl S3RemoteCache {
    pub(crate) fn new(config: S3RemoteCacheConfig) -> Result<Self> {
        validate_bucket(&config.bucket)?;
        let prefix = normalize_prefix(&config.prefix)?;
        validate_key_path(&config.namespace, "remote cache namespace")?;
        if config.region.trim().is_empty() {
            bail!("an S3 remote cache needs a region");
        }
        let client = reqwest::Client::builder()
            .connect_timeout(config.connect_timeout)
            .read_timeout(config.read_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self {
            client,
            base_url: base_url(&config)?,
            root: format!("{prefix}{}/v{LAYOUT_VERSION}/", config.namespace.trim()),
            region: config.region.trim().to_string(),
            credentials: config.credentials,
            conditional_writes: config.conditional_writes,
            conditionals_disabled: AtomicBool::new(false),
            download_timeout: config.download_timeout,
            retries: config.retries,
        })
    }

    fn object_url(&self, kind: ObjectKind, digest: &CacheDigest) -> Result<Url> {
        digest.validate()?;
        if matches!(kind, ObjectKind::ActionResult | ObjectKind::ActionManifest)
            && digest.algorithm != "blake3"
        {
            bail!("remote cache action keys must use blake3");
        }
        self.key_url(&format!(
            "{}/{}/{}/{}",
            kind.as_str(),
            digest.algorithm,
            digest.hash,
            digest.size
        ))
    }

    fn key_url(&self, key: &str) -> Result<Url> {
        Ok(self.base_url.join(&format!("{}{key}", self.root))?)
    }

    /// Build a request carrying a valid signature for this instant.
    ///
    /// Signing happens per attempt rather than once per operation: a retry
    /// after a long backoff would otherwise present a stale `x-amz-date` and be
    /// refused for clock skew.
    fn signed(
        &self,
        method: reqwest::Method,
        url: &Url,
        payload: &PayloadHash,
    ) -> Result<reqwest::RequestBuilder> {
        let context = SigningContext {
            credentials: &self.credentials,
            region: &self.region,
            timestamp: SystemTime::now(),
        };
        let mut request = self.client.request(method.clone(), url.clone());
        for (name, value) in sign(method.as_str(), url, &context, payload)? {
            request = request.header(name, value);
        }
        Ok(request)
    }

    /// Whether a conditional header should be attached to the next write.
    fn conditionals_enabled(&self) -> bool {
        self.conditional_writes != S3ConditionalWrites::Off
            && !self.conditionals_disabled.load(Ordering::Relaxed)
    }

    /// Note that a store has refused conditional writes, if that is allowed.
    ///
    /// Returns whether the caller should try the same write unconditionally.
    /// Under `required` it never is: a caller that asked for the guarantee gets
    /// an error rather than a silent downgrade.
    fn degrade_conditionals(&self, what: &str) -> bool {
        if self.conditional_writes != S3ConditionalWrites::Auto {
            return false;
        }
        if !self.conditionals_disabled.swap(true, Ordering::Relaxed) {
            warn!(
                "the remote object store does not implement conditional writes ({what}); \
                 continuing without them. Blobs and action results are content-addressed, so \
                 this is safe; concurrent task manifest updates can now lose predictions, \
                 which costs prefetch coverage on later builds"
            );
        }
        true
    }

    pub(crate) async fn check_connection(&self) -> Result<()> {
        let url = self.key_url(CONNECTIVITY_PROBE_KEY)?;
        retry_async("HEAD", &url, self.retries, || async {
            let response = self
                .signed(reqwest::Method::HEAD, &url, &PayloadHash::empty())?
                .send()
                .await?;
            match response.status() {
                // The bucket answered and authorized the request. Whether this
                // one key exists is beside the point.
                StatusCode::OK | StatusCode::NOT_FOUND => Ok(()),
                StatusCode::FORBIDDEN => bail!(
                    "the remote object store rejected these credentials for {url}; \
                     check AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY, that the key may \
                     read the bucket, and that this machine's clock is correct"
                ),
                StatusCode::MOVED_PERMANENTLY | StatusCode::TEMPORARY_REDIRECT => {
                    let region = response
                        .headers()
                        .get("x-amz-bucket-region")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or("another region");
                    bail!(
                        "the bucket is in {region}, not {}; set the remote region to match",
                        self.region
                    )
                }
                _ => Err(FailedRequest::read(response)
                    .await
                    .report("connect to", &url)),
            }
        })
        .await
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
        let url = self.object_url(ObjectKind::Blob, digest)?;
        retry_async("GET", &url, self.retries, || async {
            let mut response = self.get(&url).await?;
            // Stop reading as soon as the response outgrows the digest it claims
            // to satisfy, exactly as the protocol client does.
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
        let url = self.object_url(ObjectKind::Blob, digest)?;
        let download = retry_async("GET", &url, self.retries, || async {
            let mut response = self.get(&url).await?;
            fs::create_dir_all(staging_dir)?;
            let temporary = tempfile::NamedTempFile::new_in(staging_dir)?;
            let mut output = tokio::fs::File::from_std(temporary.reopen()?);
            let mut written = 0u64;
            while let Some(chunk) = response.chunk().await? {
                written += chunk.len() as u64;
                if written > digest.size {
                    bail!("remote cache blob exceeded the size of its digest");
                }
                output.write_all(&chunk).await?;
            }
            output.flush().await?;
            drop(output);
            if !digest.matches_file(temporary.path())? {
                bail!("remote cache blob failed digest verification");
            }
            Ok(temporary)
        });
        tokio::time::timeout(self.download_timeout, download)
            .await
            .map_err(|_| eyre!("remote cache blob download timed out for {url}"))?
    }

    /// Fetch an object that must exist, turning any other status into an error.
    async fn get(&self, url: &Url) -> Result<reqwest::Response> {
        let response = self
            .signed(reqwest::Method::GET, url, &PayloadHash::empty())?
            .send()
            .await?;
        if response.status().is_success() {
            Ok(response)
        } else {
            Err(FailedRequest::read(response).await.report("read", url))
        }
    }

    pub(crate) async fn get_action_result(
        &self,
        action: &CacheDigest,
    ) -> Result<Option<RemoteActionResult>> {
        let url = self.object_url(ObjectKind::ActionResult, action)?;
        let result = retry_async("GET", &url, self.retries, || async {
            let response = self
                .signed(reqwest::Method::GET, &url, &PayloadHash::empty())?
                .send()
                .await?;
            if response.status() == StatusCode::NOT_FOUND {
                return Ok(None);
            }
            if !response.status().is_success() {
                return Err(FailedRequest::read(response).await.report("read", &url));
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
        let url = self.object_url(ObjectKind::ActionResult, &result.action)?;
        let body = serde_json::to_vec(result)?;
        retry_async("PUT", &url, self.retries, || async {
            // Storing the same record twice is not an error: the key is its
            // content address, so an existing object already holds these bytes.
            self.put_create(&url, &body).await.map(drop)
        })
        .await
    }

    pub(crate) async fn get_action_manifest(
        &self,
        key: &CacheDigest,
    ) -> Result<Option<RemoteActionManifest>> {
        let url = self.object_url(ObjectKind::ActionManifest, key)?;
        retry_async("GET", &url, self.retries, || async {
            let response = self
                .signed(reqwest::Method::GET, &url, &PayloadHash::empty())?
                .send()
                .await?;
            if response.status() == StatusCode::NOT_FOUND {
                return Ok(None);
            }
            if !response.status().is_success() {
                return Err(FailedRequest::read(response).await.report("read", &url));
            }
            let etag = parse_strong_etag(response.headers().get(ETAG))?;
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
        let url = self.object_url(ObjectKind::ActionManifest, key)?;
        let body = bytes.to_vec();
        let expected_etag = expected_etag.map(quoted_etag).transpose()?;
        retry_async("PUT", &url, self.retries, || async {
            let outcome = loop {
                let conditional = self.conditionals_enabled();
                let mut request = self
                    .signed(reqwest::Method::PUT, &url, &PayloadHash::of(&body))?
                    .header(CONTENT_LENGTH, body.len())
                    .body(body.clone());
                if conditional {
                    request = match &expected_etag {
                        Some(etag) => request.header(IF_MATCH, etag),
                        None => request.header(IF_NONE_MATCH, "*"),
                    };
                }
                let response = request.send().await?;
                let status = response.status();
                if status.is_success() {
                    break ManifestPutOutcome::Stored;
                }
                if status == StatusCode::PRECONDITION_FAILED {
                    // Either the manifest moved under us or, for a create, one
                    // already exists. Both mean re-reading and merging.
                    break ManifestPutOutcome::PreconditionFailed;
                }
                if status == StatusCode::CONFLICT {
                    // A concurrent conditional write on the same key. S3 asks
                    // that this be retried rather than treated as a conflict of
                    // content.
                    return Err(conditional_request_conflict(&url));
                }
                let failure = FailedRequest::read(response).await;
                if conditional && failure.is_not_implemented() {
                    if self.degrade_conditionals("rejected a conditional write") {
                        continue;
                    }
                    return Err(failure.report("update", &url).wrap_err(
                        "conditional writes are required but this store does not implement them",
                    ));
                }
                return Err(failure.report("update", &url));
            };
            Ok(outcome)
        })
        .await
    }

    pub(crate) async fn put_blob(&self, upload: &BlobUpload) -> Result<()> {
        upload.digest.validate()?;
        let url = self.object_url(ObjectKind::Blob, &upload.digest)?;
        retry_async("PUT", &url, self.retries, || async {
            match &upload.source {
                BlobSource::Bytes(bytes) => self.put_create(&url, bytes).await.map(drop),
                BlobSource::File(file) => self.put_create_file(&url, file.path()).await,
                BlobSource::Path(path) => self.put_create_file(&url, path).await,
            }
        })
        .await
    }

    /// Store bytes under a key that is their content address.
    ///
    /// Returns whether this request is what created the object; an object that
    /// was already there is success, since its key names these exact bytes.
    async fn put_create(&self, url: &Url, body: &[u8]) -> Result<bool> {
        loop {
            let conditional = self.conditionals_enabled();
            let mut request = self
                .signed(reqwest::Method::PUT, url, &PayloadHash::of(body))?
                .header(CONTENT_LENGTH, body.len())
                .body(body.to_vec());
            if conditional {
                request = request.header(IF_NONE_MATCH, "*");
            }
            match self
                .finish_create(url, request.send().await?, conditional)
                .await?
            {
                Some(created) => return Ok(created),
                None => continue,
            }
        }
    }

    /// Store a file under a key that is its content address.
    ///
    /// The body is streamed with an explicit length rather than chunked, which
    /// S3 requires, and is signed as an unsigned payload so that a multi-gigabyte
    /// artifact is not read twice just to compute a hash TLS and the content
    /// address already stand behind.
    async fn put_create_file(&self, url: &Url, path: &Path) -> Result<()> {
        loop {
            let conditional = self.conditionals_enabled();
            let file = tokio::fs::File::open(path).await?;
            let length = file.metadata().await?.len();
            let mut request = self
                .signed(reqwest::Method::PUT, url, &PayloadHash::Unsigned)?
                .header(CONTENT_LENGTH, length)
                .body(reqwest::Body::wrap_stream(
                    tokio_util::io::ReaderStream::new(file),
                ));
            if conditional {
                request = request.header(IF_NONE_MATCH, "*");
            }
            match self
                .finish_create(url, request.send().await?, conditional)
                .await?
            {
                Some(_) => return Ok(()),
                None => continue,
            }
        }
    }

    /// Interpret the response to a create-only write.
    ///
    /// `None` asks the caller to send the same request again without its
    /// conditional header, having just learned the store does not support one.
    async fn finish_create(
        &self,
        url: &Url,
        response: reqwest::Response,
        conditional: bool,
    ) -> Result<Option<bool>> {
        let status = response.status();
        if status.is_success() {
            return Ok(Some(true));
        }
        if status == StatusCode::PRECONDITION_FAILED {
            return Ok(Some(false));
        }
        if status == StatusCode::CONFLICT {
            return Err(conditional_request_conflict(url));
        }
        let failure = FailedRequest::read(response).await;
        if conditional && failure.is_not_implemented() {
            if self.degrade_conditionals("rejected a create-only write") {
                return Ok(None);
            }
            return Err(failure.report("store", url).wrap_err(
                "conditional writes are required but this store does not implement them",
            ));
        }
        Err(failure.report("store", url))
    }

    // Everything below is an extension a protocol server offers and a bucket
    // does not. Reporting absence is what routes the caller to the per-object
    // requests that every version of the protocol has made.

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

/// A concurrent conditional write, which S3 asks callers to retry.
fn conditional_request_conflict(url: &Url) -> eyre::Report {
    eyre::Report::new(TransientRequest("conditional request conflict")).wrap_err(format!(
        "a concurrent conditional write to {url} conflicted"
    ))
}

/// A request that failed, read far enough to say why.
///
/// The body is consumed here because deciding what a failure means can depend
/// on the store's own error code, and a response cannot be read twice.
struct FailedRequest {
    status: StatusCode,
    code: Option<String>,
}

impl FailedRequest {
    async fn read(mut response: reqwest::Response) -> Self {
        let status = response.status();
        // Bounded like every other body this client reads. An error document is
        // a diagnostic, and a store that streams without end must not be able to
        // exhaust this process through one.
        let mut body = Vec::new();
        while body.len() < MAX_ERROR_BODY_BYTES {
            match response.chunk().await {
                Ok(Some(chunk)) => body.extend_from_slice(&chunk),
                Ok(None) | Err(_) => break,
            }
        }
        body.truncate(MAX_ERROR_BODY_BYTES);
        Self {
            status,
            code: error_code(&String::from_utf8_lossy(&body)).map(str::to_string),
        }
    }

    /// Whether the store is saying it does not implement what was asked of it.
    ///
    /// S3-compatible stores disagree about how to say this. AWS answers `501`
    /// for an unimplemented feature, while some others answer `400` with
    /// `NotImplemented` in the document. A bare `400` is not enough on its own:
    /// it is also how a store reports a request it merely disliked.
    fn is_not_implemented(&self) -> bool {
        self.status == StatusCode::NOT_IMPLEMENTED
            || (self.status == StatusCode::BAD_REQUEST
                && self.code.as_deref() == Some("NotImplemented"))
    }

    fn report(&self, verb: &str, url: &Url) -> eyre::Report {
        match &self.code {
            Some(code) => eyre!("failed to {verb} {url}: {} ({code})", self.status),
            None => eyre!("failed to {verb} {url}: {}", self.status),
        }
    }
}

/// Extract the `<Code>` of an S3 error document.
///
/// S3 reports errors as XML, but only the code is ever acted on, so it is
/// scanned for rather than parsed with an XML reader this crate would otherwise
/// not need.
fn error_code(body: &str) -> Option<&str> {
    let start = body.find("<Code>")? + "<Code>".len();
    let end = body[start..].find("</Code>")? + start;
    Some(body[start..end].trim()).filter(|code| !code.is_empty())
}

/// Reject a bucket name that could not address the store it names.
fn validate_bucket(bucket: &str) -> Result<()> {
    let bucket = bucket.trim();
    if bucket.is_empty() {
        bail!("an S3 remote cache needs a bucket");
    }
    if bucket.contains('/') || bucket.starts_with('.') || bucket.ends_with('.') {
        bail!("invalid S3 bucket name {bucket:?}");
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
///
/// The protocol carries the namespace in a header, where a server decides what
/// it means. In a bucket it is part of the key, so it has to be something a key
/// can hold and something that cannot climb out of its own prefix.
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

/// Resolve the bucket root a key is joined onto.
///
/// A custom endpoint addresses the bucket in the path, which is what MinIO and
/// R2 expect and what a bucket whose name is not a valid DNS label requires.
/// AWS itself is addressed by host, since a bucket-named host is what its TLS
/// certificate covers -- unless the name contains a dot, which the wildcard
/// certificate does not match.
fn base_url(config: &S3RemoteCacheConfig) -> Result<Url> {
    let bucket = config.bucket.trim();
    let (mut url, path_style) = match &config.endpoint {
        Some(endpoint) => (endpoint.clone(), config.force_path_style.unwrap_or(true)),
        None => (
            format!("https://s3.{}.amazonaws.com", config.region.trim()).parse()?,
            config
                .force_path_style
                .unwrap_or_else(|| bucket.contains('.')),
        ),
    };
    if path_style {
        let path = url.path().trim_end_matches('/').to_string();
        url.set_path(&format!("{path}/{bucket}/"));
    } else {
        let host = url
            .host_str()
            .ok_or_else(|| eyre!("an S3 endpoint must have a host"))?;
        url.set_host(Some(&format!("{bucket}.{host}")))?;
        url.set_path("/");
    }
    Ok(url)
}

#[cfg(test)]
#[path = "remote_s3_tests.rs"]
mod tests;
