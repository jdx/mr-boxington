//! Protocol and storage primitives for mbx build caches.
//!
//! This crate contains the types shared by cache clients, the task-scoped
//! cache agent, and remote cache implementations. Protocol records are
//! serialized with [`canonical_json`] before hashing; changing their shape is
//! therefore a wire-format change, not merely an implementation detail.
//!
//! Most consumers start with [`CacheDigest`] and the local stores
//! [`LocalCas`] and [`LocalActionCache`]. Remote clients use
//! [`RemoteCacheClient`], while mbx's compiler shim communicates with a
//! [`CacheAgent`] using [`AgentRequest`] and [`AgentResponse`].
//!
//! ```
//! use mbx_cache_core::{CacheDigest, canonical_json};
//! use serde::Serialize;
//!
//! #[derive(Serialize)]
//! struct Key<'a> {
//!     compiler: &'a str,
//!     source: CacheDigest,
//! }
//!
//! let source = CacheDigest::blake3(b"fn main() {}\n");
//! let bytes = canonical_json(&Key { compiler: "rustc", source })?;
//! let action = CacheDigest::blake3(&bytes);
//! assert_eq!(action.algorithm, "blake3");
//! # Ok::<(), eyre::Report>(())
//! ```
#![deny(missing_docs)]

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use eyre::{Result, bail, eyre};
use futures_util::TryStreamExt as _;
use log::warn;
use reqwest::StatusCode;
use reqwest::header::{
    ACCEPT, AUTHORIZATION, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, ETAG, HeaderMap,
    HeaderValue, IF_MATCH, IF_NONE_MATCH,
};
use serde::{Deserialize, Serialize};
use sha2::Digest as _;
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use url::{Host, Url};

mod agent;
mod local;

pub use agent::{
    AGENT_PROTOCOL_VERSION, ActionPrediction, AgentRemoteCache, AgentRequest, AgentResponse,
    AgentStats, CacheAgent, RestoreStats, is_task_identity, task_manifest_actions,
};
pub use local::{LocalActionCache, LocalCas};

/// Major version of the HTTP cache protocol implemented by this crate.
pub const PROTOCOL_VERSION: u8 = 1;
const PROTOCOL_HEADER: &str = "mbx-cache-protocol";
const NAMESPACE_HEADER: &str = "mbx-cache-namespace";
/// Media type for canonical [`RemoteActionResult`] JSON records.
pub const ACTION_RESULT_MEDIA_TYPE: &str = "application/vnd.mbx.cache-action-result.v1+json";
/// Media type for canonical [`CacheDirectory`] JSON records.
pub const DIRECTORY_MEDIA_TYPE: &str = "application/vnd.mbx.cache-directory.v1+json";
/// Media type for adapter-specific action metadata blobs.
pub const CLIENT_METADATA_MEDIA_TYPE: &str = "application/vnd.mbx.cache-client-metadata.v1+json";
/// Media type for task-to-action prediction manifests.
pub const TASK_ACTION_MANIFEST_MEDIA_TYPE: &str =
    "application/vnd.mbx.cache-task-action-manifest.v1+json";
/// Media type for opaque content-addressed blobs.
pub const BLOB_MEDIA_TYPE: &str = "application/octet-stream";
/// Media type for framed batches of content-addressed blobs.
pub const BLOB_PACK_MEDIA_TYPE: &str = "application/vnd.mbx.cache-blob-pack.v1";
const DIGEST_LIST_MEDIA_TYPE: &str = "application/vnd.mbx.cache-digests.v1+json";
const BLOB_PACK_BLOBS_HEADER: &str = "mbx-cache-pack-blobs";
const BLOB_PACK_BYTES_HEADER: &str = "mbx-cache-pack-bytes";
const BLOB_PACK_MAGIC: &[u8; 8] = b"MBXPACK1";
const BLOB_PACK_HEADER_BYTES: u64 = 1 + 32 + 8;
/// Cap the JSON bodies a remote cache can hand back. Blob downloads are bounded
/// by the size their digest promises, but action results and manifests carry no
/// such claim, so without an explicit ceiling a hostile or broken server can
/// stream until this process runs out of memory -- for manifests, long before
/// `validate_task_manifest` ever sees the payload. The bound matches the agent's
/// own request ceiling so both ends of the protocol refuse the same magnitude.
const MAX_REMOTE_JSON_BYTES: u64 = 16 * 1024 * 1024;
const MAX_STAGED_BLOB_PACK_BYTES: u64 = 256 * 1024 * 1024;
const MAX_STAGED_BLOB_PACK_ITEMS: usize = 2 * 1024;
const BLOB_PACK_TIMEOUT_BYTES_PER_UNIT: u64 = MAX_STAGED_BLOB_PACK_BYTES / 4;
const BLOB_PACK_TIMEOUT_ITEMS_PER_UNIT: usize = MAX_STAGED_BLOB_PACK_ITEMS / 4;

/// Serialize a protocol object using the JSON Canonicalization Scheme.
///
/// Action digests are computed from these bytes, so callers must not use
/// serde's struct field order as part of the wire contract.
pub fn canonical_json(value: &impl Serialize) -> Result<Vec<u8>> {
    Ok(serde_json_canonicalizer::to_vec(value)?)
}

#[derive(
    Debug,
    Clone,
    Copy,
    Serialize,
    Deserialize,
    Default,
    strum::EnumString,
    strum::Display,
    PartialEq,
    Eq,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
/// Operations permitted against a configured remote cache.
pub enum RemoteCacheMode {
    /// Permit reads from and writes to the remote cache.
    #[default]
    ReadWrite,
    /// Permit reads but never publish new objects.
    ReadOnly,
    /// Publish objects but never satisfy lookups from the remote cache.
    WriteOnly,
}

impl RemoteCacheMode {
    /// Whether this mode permits remote cache reads.
    pub fn reads(self) -> bool {
        matches!(self, Self::ReadWrite | Self::ReadOnly)
    }

    /// Whether this mode permits remote cache writes.
    pub fn writes(self) -> bool {
        matches!(self, Self::ReadWrite | Self::WriteOnly)
    }
}

/// Connection, authentication, and retry settings for [`RemoteCacheClient`].
pub struct RemoteCacheConfig {
    /// Base URL of the remote cache service.
    pub base_url: Url,
    /// Server-side namespace used to isolate cache objects.
    pub namespace: String,
    /// Static bearer token, if configured directly.
    pub token: Option<String>,
    /// File containing a bearer token that may be refreshed externally.
    pub token_file: Option<PathBuf>,
    /// Audience used when obtaining an OIDC token from the CI environment.
    pub oidc_audience: Option<String>,
    /// Maximum time allowed to establish a connection.
    pub connect_timeout: Duration,
    /// Maximum time without response progress for ordinary requests.
    pub read_timeout: Duration,
    /// Overall deadline for an individual blob download attempt.
    pub download_timeout: Duration,
    /// Number of attempts after the initial request for retryable failures.
    pub retries: i64,
}

/// Algorithm-tagged digest and byte length of a cache object.
///
/// Digests received from an external source should be checked with
/// [`CacheDigest::validate`] before they are used to construct paths.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CacheDigest {
    /// Hash algorithm name (`blake3` or `sha256`).
    pub algorithm: String,
    /// Lowercase hexadecimal hash value.
    pub hash: String,
    /// Exact uncompressed object length in bytes.
    pub size: u64,
}

impl CacheDigest {
    /// Compute a BLAKE3 digest for an in-memory object.
    pub fn blake3(bytes: &[u8]) -> Self {
        Self {
            algorithm: "blake3".into(),
            hash: blake3::hash(bytes).to_hex().to_string(),
            size: bytes.len() as u64,
        }
    }

    /// Hash a file while counting the bytes read in the same streaming pass.
    pub fn blake3_file(path: &Path) -> Result<Self> {
        let (hash, size) = hash_file_blake3(path)?;
        Ok(Self {
            algorithm: "blake3".into(),
            hash,
            size,
        })
    }

    /// Validate the algorithm name and hexadecimal hash representation.
    pub fn validate(&self) -> Result<()> {
        if self.algorithm != "blake3" && self.algorithm != "sha256" {
            bail!("unsupported remote cache digest algorithm");
        }
        if self.hash.len() != 64
            || !self
                .hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            bail!("invalid remote cache digest");
        }
        Ok(())
    }

    /// Return whether `bytes` have this digest and declared length.
    pub fn matches_bytes(&self, bytes: &[u8]) -> Result<bool> {
        self.validate()?;
        if self.size != bytes.len() as u64 {
            return Ok(false);
        }
        let hash = match self.algorithm.as_str() {
            "blake3" => blake3::hash(bytes).to_hex().to_string(),
            "sha256" => hex::encode(sha2::Sha256::digest(bytes)),
            _ => unreachable!("digest algorithm was validated"),
        };
        Ok(self.hash == hash)
    }

    /// Stream a file and return whether it has this digest and length.
    pub fn matches_file(&self, path: &Path) -> Result<bool> {
        self.validate()?;
        let (hash, size) = match self.algorithm.as_str() {
            "blake3" => hash_file_blake3(path)?,
            "sha256" => hash_file_sha256(path)?,
            _ => unreachable!("digest algorithm was validated"),
        };
        Ok(self.size == size && self.hash == hash)
    }
}

fn hash_file_blake3(path: &Path) -> Result<(String, u64)> {
    let mut file = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0; 64 * 1024];
    let mut size = 0;
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        size += count as u64;
    }
    Ok((hasher.finalize().to_hex().to_string(), size))
}

fn hash_file_sha256(path: &Path) -> Result<(String, u64)> {
    let mut file = File::open(path)?;
    let mut hasher = sha2::Sha256::new();
    let mut buffer = [0; 64 * 1024];
    let mut size = 0;
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        size += count as u64;
    }
    Ok((hex::encode(hasher.finalize()), size))
}

/// A canonical action-result record referencing objects in the CAS.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteActionResult {
    /// Digest of the canonical action descriptor this record satisfies.
    pub action: CacheDigest,
    /// Optional adapter metadata blob, such as [`RustcMetadata`].
    #[serde(default)]
    pub metadata: Option<CacheDigest>,
    /// Optional digest of the root [`CacheDirectory`] containing outputs.
    #[serde(default)]
    pub output_root: Option<CacheDigest>,
    /// Action-result schema version.
    pub version: u8,
}

/// A canonical directory object stored in the CAS.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheDirectory {
    /// Child directory entries, sorted canonically by name.
    pub directories: Vec<CacheDirectoryNode>,
    /// Child file entries, sorted canonically by name.
    pub files: Vec<CacheFileNode>,
    /// Child symbolic-link entries, sorted canonically by name.
    pub symlinks: Vec<CacheSymlinkNode>,
    /// Directory-object schema version.
    pub version: u8,
}

/// A child directory entry in a canonical cache directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheDirectoryNode {
    /// Digest of the child [`CacheDirectory`].
    pub digest: CacheDigest,
    /// Platform mode bits recorded for the directory.
    pub mode: u32,
    /// Single path-component name within the parent directory.
    pub name: String,
}

/// A file entry in a canonical cache directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheFileNode {
    /// Digest of the file contents.
    pub digest: CacheDigest,
    /// Whether the file should be restored as executable.
    pub executable: bool,
    /// Platform mode bits recorded for the file.
    pub mode: u32,
    /// Single path-component name within the parent directory.
    pub name: String,
}

/// A symbolic-link entry in a canonical cache directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheSymlinkNode {
    /// Platform mode bits recorded for the symbolic link.
    pub mode: u32,
    /// Single path-component name within the parent directory.
    pub name: String,
    /// Link target text exactly as recorded by the producer.
    pub target: String,
}

/// Rust-specific action metadata stored alongside compiled outputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RustcMetadata {
    /// Metadata schema version.
    pub version: u8,
    /// Adapter-defined output kind.
    pub kind: String,
    /// Digest of captured compiler standard output.
    pub stdout: CacheDigest,
    /// Digest of captured compiler standard error.
    pub stderr: CacheDigest,
}

/// Backing data for a blob upload.
pub enum BlobSource {
    /// Bytes held in memory.
    Bytes(Vec<u8>),
    /// A temporary file whose lifetime is owned by the upload.
    File(tempfile::NamedTempFile),
    /// A persistent file at the given path.
    Path(PathBuf),
}

/// A digest paired with the data to upload under that digest.
pub struct BlobUpload {
    /// Expected digest and length of the source data.
    pub digest: CacheDigest,
    /// Data source read by [`RemoteCacheClient::put_blob`].
    pub source: BlobSource,
}

/// Task action-manifest bytes returned with their concurrency token.
pub struct RemoteActionManifest {
    /// Raw canonical manifest JSON.
    pub bytes: Vec<u8>,
    /// Entity tag used for conditional manifest replacement.
    pub etag: String,
}

/// A verified set of remote CAS objects downloaded through blob-pack streams.
pub struct RemoteBlobPack {
    _directory: tempfile::TempDir,
    /// Verified blobs paired with paths in this pack's temporary directory.
    pub blobs: Vec<(CacheDigest, PathBuf)>,
    /// Number of HTTP pack requests needed to retrieve the requested set.
    pub requests: u64,
    /// Unique digests requested from the remote service.
    pub requested: Vec<CacheDigest>,
    /// Number of verified blob frames received.
    pub blob_count: u64,
    /// Total unframed blob payload bytes received.
    pub payload_bytes: u64,
    /// Total bytes received including framing.
    pub framed_bytes: u64,
}

struct DownloadedBlobPack {
    directory: tempfile::TempDir,
    blobs: Vec<(CacheDigest, PathBuf)>,
    metadata: BlobPackResponseStats,
}

#[derive(Debug, Clone, Copy, Default)]
struct BlobPackResponseMetadata {
    content_length: Option<u64>,
    blob_count: Option<u64>,
    payload_bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
struct BlobPackResponseStats {
    blob_count: u64,
    payload_bytes: u64,
    framed_bytes: u64,
}

impl BlobPackResponseMetadata {
    fn from_headers(headers: &HeaderMap) -> Result<Self> {
        Ok(Self {
            content_length: optional_u64_header(headers, CONTENT_LENGTH.as_str())?,
            blob_count: optional_u64_header(headers, BLOB_PACK_BLOBS_HEADER)?,
            payload_bytes: optional_u64_header(headers, BLOB_PACK_BYTES_HEADER)?,
        })
    }

    fn validate(self, decoded: BlobPackResponseStats) -> Result<BlobPackResponseStats> {
        if let Some(content_length) = self.content_length
            && content_length != decoded.framed_bytes
        {
            bail!(
                "remote cache blob pack content length metadata mismatch: expected {}, decoded {}",
                content_length,
                decoded.framed_bytes
            );
        }
        if let Some(blob_count) = self.blob_count
            && blob_count != decoded.blob_count
        {
            bail!(
                "remote cache blob pack blob count metadata mismatch: expected {}, decoded {}",
                blob_count,
                decoded.blob_count
            );
        }
        if let Some(payload_bytes) = self.payload_bytes
            && payload_bytes != decoded.payload_bytes
        {
            bail!(
                "remote cache blob pack payload byte metadata mismatch: expected {}, decoded {}",
                payload_bytes,
                decoded.payload_bytes
            );
        }
        Ok(BlobPackResponseStats {
            blob_count: self.blob_count.unwrap_or(decoded.blob_count),
            payload_bytes: self.payload_bytes.unwrap_or(decoded.payload_bytes),
            framed_bytes: self.content_length.unwrap_or(decoded.framed_bytes),
        })
    }
}

fn optional_u64_header(headers: &HeaderMap, name: &str) -> Result<Option<u64>> {
    let Some(value) = headers.get(name) else {
        return Ok(None);
    };
    let value = value
        .to_str()
        .map_err(|_| eyre!("remote cache blob pack {name} header is not valid UTF-8"))?;
    let value = value
        .parse::<u64>()
        .map_err(|_| eyre!("remote cache blob pack {name} header is not an unsigned integer"))?;
    Ok(Some(value))
}

#[derive(Debug, Deserialize)]
struct RemoteCacheCapabilities {
    protocol: CapabilityProtocol,
    #[serde(default)]
    features: CapabilityFeatures,
    #[serde(default)]
    limits: CapabilityLimits,
    /// Content codings the server accepts and produces; always includes
    /// `identity`, and compression is used only when `zstd` is offered.
    #[serde(default)]
    compressors: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct CapabilityProtocol {
    major: u8,
}

#[derive(Debug, Default, Deserialize)]
struct CapabilityFeatures {
    #[serde(default)]
    blob_packs: bool,
}

#[derive(Debug, Default, Deserialize)]
struct CapabilityLimits {
    #[serde(default)]
    max_batch_items: u64,
    #[serde(default)]
    max_pack_bytes: u64,
}

#[derive(Debug, Clone, Copy)]
struct BlobPackLimits {
    max_items: usize,
    max_bytes: u64,
}

/// What one capabilities exchange settled, cached for the session.
///
/// `Default` is also the answer for a server with no capabilities endpoint:
/// no blob packs and no compression, which is exactly how every request
/// behaved before either feature existed.
#[derive(Debug, Clone, Copy, Default)]
struct NegotiatedCapabilities {
    blob_packs: Option<BlobPackLimits>,
    zstd_uploads: bool,
}

#[derive(Serialize)]
struct DigestList<'a> {
    digests: &'a [CacheDigest],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Result of a conditional task-manifest write.
pub enum ManifestPutOutcome {
    /// The manifest was stored.
    Stored,
    /// The supplied entity-tag precondition did not match.
    PreconditionFailed,
}

/// HTTP client for the mbx remote cache protocol.
///
/// The client validates digests, response sizes, media types, and redirects at
/// the protocol boundary. It is safe to share between asynchronous tasks.
pub struct RemoteCacheClient {
    base_url: Url,
    namespace: String,
    client: reqwest::Client,
    credential: RemoteCacheCredential,
    download_timeout: Duration,
    retries: i64,
    capabilities: tokio::sync::OnceCell<NegotiatedCapabilities>,
    blob_packs_disabled: AtomicBool,
}

impl RemoteCacheClient {
    /// Construct a client and validate its URL and authentication settings.
    pub fn new(config: RemoteCacheConfig) -> Result<Self> {
        let authenticated = config
            .token
            .as_deref()
            .is_some_and(|token| !token.trim().is_empty())
            || config.token_file.is_some()
            || config
                .oidc_audience
                .as_deref()
                .is_some_and(|audience| !audience.trim().is_empty());
        validate_remote_url(&config.base_url, authenticated)?;
        let client = reqwest::Client::builder()
            .connect_timeout(config.connect_timeout)
            .read_timeout(config.read_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let credential = remote_credential(&config, client.clone())?;
        Ok(Self {
            base_url: normalized_base_url(config.base_url),
            namespace: config.namespace,
            client,
            credential,
            download_timeout: config.download_timeout,
            retries: config.retries,
            capabilities: tokio::sync::OnceCell::new(),
            blob_packs_disabled: AtomicBool::new(false),
        })
    }

    fn action_result_endpoint(&self, action: &CacheDigest) -> Result<Url> {
        action.validate()?;
        if action.algorithm != "blake3" {
            bail!("remote cache action keys must use blake3");
        }
        Ok(self.base_url.join(&format!(
            "v{PROTOCOL_VERSION}/action-results/{}/{}/{}",
            action.algorithm, action.hash, action.size
        ))?)
    }

    fn blob_endpoint(&self, digest: &CacheDigest) -> Result<Url> {
        digest.validate()?;
        Ok(self.base_url.join(&format!(
            "v{PROTOCOL_VERSION}/blobs/{}/{}/{}",
            digest.algorithm, digest.hash, digest.size
        ))?)
    }

    fn action_manifest_endpoint(&self, key: &CacheDigest) -> Result<Url> {
        key.validate()?;
        if key.algorithm != "blake3" {
            bail!("remote action manifest keys must use blake3");
        }
        Ok(self.base_url.join(&format!(
            "v{PROTOCOL_VERSION}/action-manifests/{}/{}/{}",
            key.algorithm, key.hash, key.size
        ))?)
    }

    fn capabilities_endpoint(&self) -> Result<Url> {
        Ok(self
            .base_url
            .join(&format!("v{PROTOCOL_VERSION}/capabilities"))?)
    }

    fn blob_pack_endpoint(&self) -> Result<Url> {
        Ok(self
            .base_url
            .join(&format!("v{PROTOCOL_VERSION}/blobs:pack"))?)
    }

    async fn request(
        &self,
        method: reqwest::Method,
        url: Url,
        media_type: &'static str,
    ) -> Result<reqwest::RequestBuilder> {
        let request = self
            .client
            .request(method, url)
            .header(PROTOCOL_HEADER, u16::from(PROTOCOL_VERSION))
            .header(NAMESPACE_HEADER, &self.namespace)
            .header(ACCEPT, media_type);
        if let Some(authorization) = self.credential.authorization().await? {
            Ok(request.header(AUTHORIZATION, authorization))
        } else {
            Ok(request)
        }
    }

    async fn blob_pack_limits(&self) -> Result<Option<BlobPackLimits>> {
        Ok(self.negotiated_capabilities().await?.blob_packs)
    }

    async fn negotiated_capabilities(&self) -> Result<NegotiatedCapabilities> {
        self.capabilities
            .get_or_try_init(|| async {
                let url = self.capabilities_endpoint()?;
                let response = self
                    .request(reqwest::Method::GET, url, "application/json")
                    .await?
                    .send()
                    .await?;
                if matches!(
                    response.status(),
                    StatusCode::NOT_FOUND
                        | StatusCode::METHOD_NOT_ALLOWED
                        | StatusCode::NOT_IMPLEMENTED
                ) {
                    return Ok(NegotiatedCapabilities::default());
                }
                let bytes =
                    read_bounded_json(response.error_for_status()?, "capabilities").await?;
                let capabilities: RemoteCacheCapabilities = serde_json::from_slice(&bytes)?;
                if capabilities.protocol.major != PROTOCOL_VERSION {
                    bail!(
                        "remote cache capability protocol {} is incompatible with client protocol {PROTOCOL_VERSION}",
                        capabilities.protocol.major
                    );
                }
                // Compression is negotiated, never assumed: a body sent with a
                // coding the server did not offer would be stored corrupt or
                // rejected, so absence of the advertisement means identity.
                let zstd_uploads = capabilities
                    .compressors
                    .iter()
                    .any(|compressor| compressor == "zstd");
                let blob_packs = if capabilities.features.blob_packs {
                    let max_items = usize::try_from(capabilities.limits.max_batch_items)
                        .ok()
                        .filter(|limit| *limit > 0)
                        .ok_or_else(|| {
                            eyre!(
                                "remote cache blob packs require a positive max_batch_items limit"
                            )
                        })?;
                    if capabilities.limits.max_pack_bytes == 0 {
                        bail!("remote cache blob packs require a positive max_pack_bytes limit");
                    }
                    Some(BlobPackLimits {
                        max_items: max_items.min(MAX_STAGED_BLOB_PACK_ITEMS),
                        max_bytes: capabilities
                            .limits
                            .max_pack_bytes
                            .min(MAX_STAGED_BLOB_PACK_BYTES),
                    })
                } else {
                    None
                };
                Ok(NegotiatedCapabilities {
                    blob_packs,
                    zstd_uploads,
                })
            })
            .await
            .copied()
    }

    /// Download verified CAS objects using the server's negotiated blob-pack extension.
    ///
    /// `None` means the server does not support blob packs. Objects omitted by a
    /// supported server are absent from `blobs`, so callers can retry them through
    /// the ordinary single-blob endpoint.
    pub async fn get_blob_pack(
        &self,
        digests: &[CacheDigest],
        staging_dir: &Path,
    ) -> Result<Option<RemoteBlobPack>> {
        if digests.is_empty() || self.blob_packs_disabled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let Some(limits) = self.blob_pack_limits().await? else {
            return Ok(None);
        };
        fs::create_dir_all(staging_dir)?;
        let chunk = blob_pack_chunk(digests, limits)?;
        if chunk.is_empty() {
            return Ok(Some(RemoteBlobPack {
                _directory: tempfile::tempdir_in(staging_dir)?,
                blobs: Vec::new(),
                requests: 0,
                requested: Vec::new(),
                blob_count: 0,
                payload_bytes: 0,
                framed_bytes: BLOB_PACK_MAGIC.len() as u64,
            }));
        }
        match self.download_blob_pack_chunk(&chunk, staging_dir).await? {
            Some(pack) => Ok(Some(RemoteBlobPack {
                _directory: pack.directory,
                blobs: pack.blobs,
                requests: 1,
                requested: chunk,
                blob_count: pack.metadata.blob_count,
                payload_bytes: pack.metadata.payload_bytes,
                framed_bytes: pack.metadata.framed_bytes,
            })),
            None => {
                self.blob_packs_disabled.store(true, Ordering::Relaxed);
                Ok(None)
            }
        }
    }

    async fn download_blob_pack_chunk(
        &self,
        digests: &[CacheDigest],
        staging_dir: &Path,
    ) -> Result<Option<DownloadedBlobPack>> {
        let url = self.blob_pack_endpoint()?;
        let body = serde_json::to_vec(&DigestList { digests })?;
        let download_timeout = blob_pack_download_timeout(self.download_timeout, digests);
        let download = retry_async("POST", &url, self.retries, || async {
            let response = self
                .request(reqwest::Method::POST, url.clone(), BLOB_PACK_MEDIA_TYPE)
                .await?
                .header(CONTENT_TYPE, DIGEST_LIST_MEDIA_TYPE)
                .body(body.clone())
                .send()
                .await?;
            if matches!(
                response.status(),
                StatusCode::NOT_FOUND
                    | StatusCode::METHOD_NOT_ALLOWED
                    | StatusCode::NOT_IMPLEMENTED
            ) {
                return Ok(None);
            }
            let response = response.error_for_status()?;
            let media_type = response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.split(';').next())
                .map(str::trim);
            if media_type != Some(BLOB_PACK_MEDIA_TYPE) {
                bail!("remote cache blob pack has an invalid content type");
            }
            Ok(Some(
                decode_blob_pack(response, digests, staging_dir).await?,
            ))
        });
        tokio::time::timeout(download_timeout, download)
            .await
            .map_err(|_| eyre!("remote cache blob pack download timed out for {url}"))?
    }

    /// Fetch and validate an action-result record, returning `None` on a miss.
    pub async fn get_action_result(
        &self,
        action: &CacheDigest,
    ) -> Result<Option<RemoteActionResult>> {
        let url = self.action_result_endpoint(action)?;
        let result = retry_async("GET", &url, self.retries, || async {
            let response = self
                .request(reqwest::Method::GET, url.clone(), ACTION_RESULT_MEDIA_TYPE)
                .await?
                .send()
                .await?;
            if response.status() == StatusCode::NOT_FOUND {
                return Ok(None);
            }
            let bytes = read_bounded_json(response.error_for_status()?, "action result").await?;
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

    /// Canonically serialize and store an action-result record.
    pub async fn put_action_result(&self, result: &RemoteActionResult) -> Result<()> {
        let url = self.action_result_endpoint(&result.action)?;
        let body = serde_json::to_vec(result)?;
        retry_async("PUT", &url, self.retries, || async {
            let response = self
                .request(reqwest::Method::PUT, url.clone(), ACTION_RESULT_MEDIA_TYPE)
                .await?
                .header(CONTENT_TYPE, ACTION_RESULT_MEDIA_TYPE)
                .header(IF_NONE_MATCH, "*")
                .body(body.clone())
                .send()
                .await?;
            if response.status() != StatusCode::PRECONDITION_FAILED {
                response.error_for_status()?;
            }
            Ok(())
        })
        .await
    }

    /// Fetch a task action manifest and the entity tag needed to update it.
    pub async fn get_action_manifest(
        &self,
        key: &CacheDigest,
    ) -> Result<Option<RemoteActionManifest>> {
        let url = self.action_manifest_endpoint(key)?;
        retry_async("GET", &url, self.retries, || async {
            let response = self
                .request(
                    reqwest::Method::GET,
                    url.clone(),
                    TASK_ACTION_MANIFEST_MEDIA_TYPE,
                )
                .await?
                .send()
                .await?;
            if response.status() == StatusCode::NOT_FOUND {
                return Ok(None);
            }
            let response = response.error_for_status()?;
            let etag = parse_strong_etag(response.headers().get(ETAG))?;
            let bytes = read_bounded_json(response, "action manifest").await?;
            if blake3::hash(&bytes).to_hex().as_str() != etag {
                bail!("remote action manifest ETag does not match its body");
            }
            Ok(Some(RemoteActionManifest { bytes, etag }))
        })
        .await
    }

    /// Store a task action manifest, optionally requiring an entity-tag match.
    pub async fn put_action_manifest(
        &self,
        key: &CacheDigest,
        bytes: &[u8],
        expected_etag: Option<&str>,
    ) -> Result<ManifestPutOutcome> {
        let url = self.action_manifest_endpoint(key)?;
        let body = bytes.to_vec();
        let expected_etag = expected_etag.map(quoted_etag).transpose()?;
        retry_async("PUT", &url, self.retries, || async {
            let mut request = self
                .request(
                    reqwest::Method::PUT,
                    url.clone(),
                    TASK_ACTION_MANIFEST_MEDIA_TYPE,
                )
                .await?
                .header(CONTENT_TYPE, TASK_ACTION_MANIFEST_MEDIA_TYPE)
                .body(body.clone());
            request = if let Some(etag) = &expected_etag {
                request.header(IF_MATCH, etag)
            } else {
                request.header(IF_NONE_MATCH, "*")
            };
            let response = request.send().await?;
            if response.status() == StatusCode::PRECONDITION_FAILED {
                return Ok(ManifestPutOutcome::PreconditionFailed);
            }
            response.error_for_status()?;
            Ok(ManifestPutOutcome::Stored)
        })
        .await
    }

    /// Download a small blob into memory and verify its digest.
    pub async fn get_blob(
        &self,
        digest: &CacheDigest,
        media_type: &'static str,
    ) -> Result<Vec<u8>> {
        digest.validate()?;
        let url = self.blob_endpoint(digest)?;
        retry_async("GET", &url, self.retries, || async {
            let mut response = self
                .request(reqwest::Method::GET, url.clone(), media_type)
                .await?
                .send()
                .await?
                .error_for_status()?;
            // Stop reading as soon as the response outgrows the digest it claims
            // to satisfy. A server that streams more than it promised must not be
            // able to exhaust this process before verification rejects it.
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

    /// Download a blob to a temporary file and verify its digest.
    pub async fn get_blob_file(
        &self,
        digest: &CacheDigest,
        staging_dir: &Path,
    ) -> Result<tempfile::NamedTempFile> {
        let url = self.blob_endpoint(digest)?;
        let download = retry_async("GET", &url, self.retries, || async {
            let mut response = self
                .request(reqwest::Method::GET, url.clone(), BLOB_MEDIA_TYPE)
                .await?
                .send()
                .await?;
            response.error_for_status_ref()?;
            fs::create_dir_all(staging_dir)?;
            let temporary = tempfile::NamedTempFile::new_in(staging_dir)?;
            let mut output = tokio::fs::File::from_std(temporary.reopen()?);
            // Bound the download by the digest's own size so an oversized
            // response cannot fill the disk before verification rejects it.
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

    /// Verify and upload a content-addressed blob.
    pub async fn put_blob(&self, upload: &BlobUpload) -> Result<()> {
        let url = self.blob_endpoint(&upload.digest)?;
        // A failed negotiation downgrades to identity rather than failing the
        // upload: compression is an economy, not a requirement.
        let compress = self
            .negotiated_capabilities()
            .await
            .map(|capabilities| capabilities.zstd_uploads)
            .unwrap_or(false);
        retry_async("PUT", &url, self.retries, || async {
            let request = self
                .request(reqwest::Method::PUT, url.clone(), BLOB_MEDIA_TYPE)
                .await?
                .header(CONTENT_TYPE, BLOB_MEDIA_TYPE)
                .header(IF_NONE_MATCH, "*");
            let request = if compress {
                // Compressed and therefore chunked: the length of the encoded
                // stream is not known up front, and the digest already tells
                // the server the decompressed size it must enforce.
                let reader: Box<dyn tokio::io::AsyncRead + Send + Sync + Unpin> = match &upload
                    .source
                {
                    BlobSource::Bytes(bytes) => Box::new(std::io::Cursor::new(bytes.clone())),
                    BlobSource::File(file) => Box::new(tokio::fs::File::open(file.path()).await?),
                    BlobSource::Path(path) => Box::new(tokio::fs::File::open(path).await?),
                };
                let encoder = async_compression::tokio::bufread::ZstdEncoder::new(
                    tokio::io::BufReader::new(reader),
                );
                request
                    .header(CONTENT_ENCODING, "zstd")
                    .body(reqwest::Body::wrap_stream(
                        tokio_util::io::ReaderStream::new(encoder),
                    ))
            } else {
                let (length, body) = match &upload.source {
                    BlobSource::Bytes(bytes) => {
                        (bytes.len() as u64, reqwest::Body::from(bytes.clone()))
                    }
                    BlobSource::File(file) => {
                        let file = tokio::fs::File::open(file.path()).await?;
                        let length = file.metadata().await?.len();
                        let stream = tokio_util::io::ReaderStream::new(file);
                        (length, reqwest::Body::wrap_stream(stream))
                    }
                    BlobSource::Path(path) => {
                        let file = tokio::fs::File::open(path).await?;
                        let length = file.metadata().await?.len();
                        let stream = tokio_util::io::ReaderStream::new(file);
                        (length, reqwest::Body::wrap_stream(stream))
                    }
                };
                request.header(CONTENT_LENGTH, length).body(body)
            };
            let response = request.send().await?;
            if response.status() != StatusCode::PRECONDITION_FAILED {
                response.error_for_status()?;
            }
            Ok(())
        })
        .await
    }
}

fn blob_pack_chunk(digests: &[CacheDigest], limits: BlobPackLimits) -> Result<Vec<CacheDigest>> {
    let mut seen = BTreeSet::new();
    let mut chunk = Vec::new();
    let mut chunk_bytes = 0_u64;
    for digest in digests {
        digest.validate()?;
        if !seen.insert(digest.clone()) || digest.size > limits.max_bytes {
            continue;
        }
        if chunk.len() == limits.max_items
            || chunk_bytes.saturating_add(digest.size) > limits.max_bytes
        {
            break;
        }
        chunk_bytes = chunk_bytes.saturating_add(digest.size);
        chunk.push(digest.clone());
    }
    Ok(chunk)
}

fn blob_pack_download_timeout(base: Duration, digests: &[CacheDigest]) -> Duration {
    let bytes = digests
        .iter()
        .fold(0_u64, |total, digest| total.saturating_add(digest.size));
    let byte_units = bytes.div_ceil(BLOB_PACK_TIMEOUT_BYTES_PER_UNIT);
    let item_units = digests.len().div_ceil(BLOB_PACK_TIMEOUT_ITEMS_PER_UNIT);
    let item_units = u64::try_from(item_units).unwrap_or(u64::MAX);
    let multiplier = byte_units.max(item_units).max(1);
    base.saturating_mul(u32::try_from(multiplier).unwrap_or(u32::MAX))
}

/// Buffer a JSON response body, refusing to grow past [`MAX_REMOTE_JSON_BYTES`].
/// A declared `Content-Length` is rejected up front so an oversized body costs
/// nothing to refuse; the streaming check then covers servers that understate or
/// omit it.
async fn read_bounded_json(response: reqwest::Response, what: &str) -> Result<Vec<u8>> {
    if let Some(length) = response.content_length()
        && length > MAX_REMOTE_JSON_BYTES
    {
        bail!(
            "remote cache {what} declared {length} bytes, over the {MAX_REMOTE_JSON_BYTES} byte limit"
        );
    }
    let mut response = response;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len() as u64 + chunk.len() as u64 > MAX_REMOTE_JSON_BYTES {
            bail!("remote cache {what} exceeded the {MAX_REMOTE_JSON_BYTES} byte limit");
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn decode_blob_pack(
    response: reqwest::Response,
    requested: &[CacheDigest],
    staging_dir: &Path,
) -> Result<DownloadedBlobPack> {
    let metadata = BlobPackResponseMetadata::from_headers(response.headers())?;
    let requested = requested.iter().cloned().collect::<BTreeSet<_>>();
    let stream = response.bytes_stream().map_err(std::io::Error::other);
    let mut reader = tokio_util::io::StreamReader::new(stream);
    let mut magic = [0_u8; BLOB_PACK_MAGIC.len()];
    reader.read_exact(&mut magic).await?;
    if &magic != BLOB_PACK_MAGIC {
        bail!("remote cache blob pack has invalid magic");
    }

    let directory = tempfile::tempdir_in(staging_dir)?;
    let mut seen = BTreeSet::new();
    let mut blobs = Vec::new();
    let mut payload_bytes = 0_u64;
    let mut framed_bytes = BLOB_PACK_MAGIC.len() as u64;
    loop {
        let mut algorithm = [0_u8; 1];
        if reader.read(&mut algorithm).await? == 0 {
            break;
        }
        let (algorithm, mut hasher) = match algorithm[0] {
            1 => (
                "blake3",
                BlobPackHasher::Blake3(Box::new(blake3::Hasher::new())),
            ),
            2 => ("sha256", BlobPackHasher::Sha256(sha2::Sha256::new())),
            _ => bail!("remote cache blob pack has an invalid digest algorithm"),
        };
        let mut hash = [0_u8; 32];
        reader.read_exact(&mut hash).await?;
        let mut size = [0_u8; 8];
        reader.read_exact(&mut size).await?;
        let digest = CacheDigest {
            algorithm: algorithm.into(),
            hash: hex::encode(hash),
            size: u64::from_be_bytes(size),
        };
        if !requested.contains(&digest) {
            bail!("remote cache blob pack returned an unrequested digest");
        }
        if !seen.insert(digest.clone()) {
            bail!("remote cache blob pack returned a duplicate digest");
        }
        framed_bytes = framed_bytes
            .checked_add(BLOB_PACK_HEADER_BYTES)
            .and_then(|bytes| bytes.checked_add(digest.size))
            .ok_or_else(|| eyre!("remote cache blob pack is too large"))?;
        payload_bytes = payload_bytes
            .checked_add(digest.size)
            .ok_or_else(|| eyre!("remote cache blob pack payload is too large"))?;

        let path = directory.path().join(blobs.len().to_string());
        let mut output = tokio::fs::File::create(&path).await?;
        let mut remaining = digest.size;
        let mut buffer = [0_u8; 64 * 1024];
        while remaining > 0 {
            let limit = usize::try_from(remaining.min(buffer.len() as u64)).unwrap();
            let count = reader.read(&mut buffer[..limit]).await?;
            if count == 0 {
                bail!("remote cache blob pack ended before a blob was complete");
            }
            output.write_all(&buffer[..count]).await?;
            hasher.update(&buffer[..count]);
            remaining -= count as u64;
        }
        output.flush().await?;
        drop(output);
        if !hasher.matches(&digest.hash) {
            bail!("remote cache blob pack failed digest verification");
        }
        blobs.push((digest, path));
    }
    let blob_count = blobs.len().try_into().unwrap_or(u64::MAX);
    let metadata = metadata.validate(BlobPackResponseStats {
        blob_count,
        payload_bytes,
        framed_bytes,
    })?;
    Ok(DownloadedBlobPack {
        directory,
        blobs,
        metadata,
    })
}

enum BlobPackHasher {
    Blake3(Box<blake3::Hasher>),
    Sha256(sha2::Sha256),
}

impl BlobPackHasher {
    fn update(&mut self, bytes: &[u8]) {
        match self {
            Self::Blake3(hasher) => {
                hasher.update(bytes);
            }
            Self::Sha256(hasher) => {
                hasher.update(bytes);
            }
        }
    }

    fn matches(self, expected: &str) -> bool {
        match self {
            Self::Blake3(hasher) => hasher.finalize().to_hex().as_str() == expected,
            Self::Sha256(hasher) => hex::encode(hasher.finalize()) == expected,
        }
    }
}

fn parse_strong_etag(value: Option<&HeaderValue>) -> Result<String> {
    let value = value
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| eyre!("remote action manifest response is missing an ETag"))?;
    let etag = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .filter(|value| is_lower_hex_digest(value))
        .ok_or_else(|| eyre!("remote action manifest response has an invalid ETag"))?;
    Ok(etag.to_owned())
}

fn quoted_etag(etag: &str) -> Result<HeaderValue> {
    if !is_lower_hex_digest(etag) {
        bail!("invalid remote action manifest ETag");
    }
    Ok(HeaderValue::from_str(&format!("\"{etag}\""))?)
}

fn is_lower_hex_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[derive(Clone)]
enum RemoteCacheCredential {
    None,
    Static(HeaderValue),
    File(PathBuf),
    GithubActions(Arc<GithubActionsOidcCredential>),
}

struct GithubActionsOidcCredential {
    audience: String,
    request_url: Url,
    request_token: HeaderValue,
    client: reqwest::Client,
    retries: i64,
    cached: tokio::sync::Mutex<Option<CachedOidcToken>>,
}

struct CachedOidcToken {
    authorization: HeaderValue,
    expires_at: u64,
}

#[derive(Deserialize)]
struct GithubActionsOidcResponse {
    value: String,
}

#[derive(Deserialize)]
struct JwtExpiry {
    exp: u64,
}

fn remote_credential(
    config: &RemoteCacheConfig,
    client: reqwest::Client,
) -> Result<RemoteCacheCredential> {
    if let Some(authorization) = authorization_header(config.token.as_deref())? {
        return Ok(RemoteCacheCredential::Static(authorization));
    }
    if let Some(path) = &config.token_file {
        return Ok(RemoteCacheCredential::File(path.clone()));
    }
    let Some(audience) = config
        .oidc_audience
        .as_deref()
        .map(str::trim)
        .filter(|audience| !audience.is_empty())
    else {
        return Ok(RemoteCacheCredential::None);
    };
    Ok(RemoteCacheCredential::GithubActions(Arc::new(
        GithubActionsOidcCredential::from_env(audience, client, config.retries)?,
    )))
}

fn authorization_header(token: Option<&str>) -> Result<Option<HeaderValue>> {
    let Some(token) = token.map(str::trim).filter(|token| !token.is_empty()) else {
        return Ok(None);
    };
    let mut value = HeaderValue::from_str(&format!("Bearer {token}"))?;
    value.set_sensitive(true);
    Ok(Some(value))
}

impl RemoteCacheCredential {
    async fn authorization(&self) -> Result<Option<HeaderValue>> {
        match self {
            Self::None => Ok(None),
            Self::Static(value) => Ok(Some(value.clone())),
            Self::File(path) => {
                let token = tokio::fs::read_to_string(path).await.map_err(|err| {
                    eyre!(
                        "failed to read remote cache token file {}: {err}",
                        path.display()
                    )
                })?;
                authorization_header(Some(&token))?
                    .ok_or_else(|| eyre!("remote cache token file {} is empty", path.display()))
                    .map(Some)
            }
            Self::GithubActions(credential) => credential.authorization().await.map(Some),
        }
    }
}

impl GithubActionsOidcCredential {
    fn from_env(audience: &str, client: reqwest::Client, retries: i64) -> Result<Self> {
        let request_url = std::env::var("ACTIONS_ID_TOKEN_REQUEST_URL").map_err(|_| {
            eyre!(
                "remote cache OIDC audience requires GitHub Actions OIDC; \
                 grant `id-token: write` or set MBX_REMOTE_TOKEN"
            )
        })?;
        let request_token = std::env::var("ACTIONS_ID_TOKEN_REQUEST_TOKEN").map_err(|_| {
            eyre!(
                "remote cache OIDC audience requires GitHub Actions OIDC; \
                 ACTIONS_ID_TOKEN_REQUEST_TOKEN is missing"
            )
        })?;
        let request_url: Url = request_url
            .parse()
            .map_err(|err| eyre!("invalid GitHub Actions OIDC request URL: {err}"))?;
        Self::new(audience, request_url, &request_token, client, retries)
    }

    fn new(
        audience: &str,
        mut request_url: Url,
        request_token: &str,
        client: reqwest::Client,
        retries: i64,
    ) -> Result<Self> {
        validate_oidc_request_url(&request_url)?;
        let query = request_url
            .query_pairs()
            .filter(|(key, _)| key != "audience")
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect::<Vec<_>>();
        request_url.set_query(None);
        request_url
            .query_pairs_mut()
            .extend_pairs(query)
            .append_pair("audience", audience);
        let request_token = authorization_header(Some(request_token))?
            .ok_or_else(|| eyre!("GitHub Actions OIDC request token is empty"))?;
        Ok(Self {
            audience: audience.to_string(),
            request_url,
            request_token,
            client,
            retries,
            cached: tokio::sync::Mutex::new(None),
        })
    }

    async fn authorization(&self) -> Result<HeaderValue> {
        const REFRESH_LEEWAY_SECONDS: u64 = 60;
        let mut cached = self.cached.lock().await;
        let now = unix_timestamp()?;
        if let Some(token) = cached.as_ref()
            && token.expires_at > now.saturating_add(REFRESH_LEEWAY_SECONDS)
        {
            return Ok(token.authorization.clone());
        }
        let response: GithubActionsOidcResponse =
            retry_async("GET", &self.request_url, self.retries, || async {
                Ok(self
                    .client
                    .get(self.request_url.clone())
                    .header(AUTHORIZATION, self.request_token.clone())
                    .send()
                    .await?
                    .error_for_status()?
                    .json()
                    .await?)
            })
            .await
            .map_err(|err| {
                eyre!(
                    "failed to acquire GitHub Actions OIDC token for audience {:?}: {err}",
                    self.audience
                )
            })?;
        let expires_at = jwt_expiry(&response.value)?;
        if expires_at <= now.saturating_add(REFRESH_LEEWAY_SECONDS) {
            bail!("GitHub Actions OIDC token expires too soon");
        }
        let authorization = authorization_header(Some(&response.value))?
            .ok_or_else(|| eyre!("GitHub Actions returned an empty OIDC token"))?;
        *cached = Some(CachedOidcToken {
            authorization: authorization.clone(),
            expires_at,
        });
        Ok(authorization)
    }
}

fn jwt_expiry(token: &str) -> Result<u64> {
    let payload = token
        .split('.')
        .nth(1)
        .ok_or_else(|| eyre!("GitHub Actions returned a malformed OIDC token"))?;
    let payload = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| eyre!("GitHub Actions returned a malformed OIDC token"))?;
    let claims: JwtExpiry = serde_json::from_slice(&payload)
        .map_err(|_| eyre!("GitHub Actions OIDC token is missing a valid expiry"))?;
    Ok(claims.exp)
}

fn unix_timestamp() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| eyre!("system clock is before the Unix epoch: {err}"))?
        .as_secs())
}

fn validate_oidc_request_url(url: &Url) -> Result<()> {
    if url.scheme() == "https"
        || url.scheme() == "http"
            && url.host().is_some_and(|host| match host {
                Host::Domain(host) => host.eq_ignore_ascii_case("localhost"),
                Host::Ipv4(address) => address.is_loopback(),
                Host::Ipv6(address) => address.is_loopback(),
            })
    {
        Ok(())
    } else {
        bail!("GitHub Actions OIDC request URL must use HTTPS")
    }
}

fn validate_remote_url(base_url: &Url, authenticated: bool) -> Result<()> {
    if base_url.scheme() == "https" {
        return Ok(());
    }
    if base_url.scheme() != "http" {
        bail!("remote cache URL must use HTTPS");
    }
    let is_loopback = base_url.host().is_some_and(|host| match host {
        Host::Domain(host) => host.eq_ignore_ascii_case("localhost"),
        Host::Ipv4(address) => address.is_loopback(),
        Host::Ipv6(address) => address.is_loopback(),
    });
    if !is_loopback && authenticated {
        bail!("remote cache URL must use HTTPS except for loopback development servers");
    }
    if !is_loopback {
        warn!(
            "using an unauthenticated remote build cache over plain HTTP; cache traffic can be read \
             or modified in transit"
        );
    }
    Ok(())
}

fn normalized_base_url(mut url: Url) -> Url {
    if !url.path().ends_with('/') {
        url.set_path(&format!("{}/", url.path()));
    }
    url
}

fn retry_delays(retries: i64) -> impl Iterator<Item = Duration> {
    [200u64, 1_000, 4_000, 15_000]
        .into_iter()
        .chain(std::iter::repeat(15_000))
        .map(Duration::from_millis)
        .map(|duration| {
            let factor = 0.5 + rand::random::<f64>() * 0.5;
            Duration::from_secs_f64(duration.as_secs_f64() * factor)
        })
        .take(retries.max(0) as usize)
}

/// hyper-util exposes DNS failures in the error chain as a `dns error` source,
/// but reqwest intentionally erases the concrete connector type. Match that
/// stable connector error label rather than platform-specific resolver text.
fn is_dns_error(error: &(dyn std::error::Error + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(source) = current {
        if source.to_string() == "dns error" {
            return true;
        }
        current = source.source();
    }
    false
}

fn is_transient(error: &eyre::Report) -> bool {
    // An unavailable hostname is a deterministic configuration error. reqwest
    // categorizes it as a connect error, but retrying only delays the diagnosis.
    if is_dns_error(error.as_ref()) {
        return false;
    }
    error.chain().any(|source| {
        let Some(error) = source.downcast_ref::<reqwest::Error>() else {
            return false;
        };
        if error.is_timeout() || error.is_connect() || error.is_body() {
            return true;
        }
        error.status().is_some_and(|status| {
            let status = status.as_u16();
            status == 408 || status == 429 || (500..600).contains(&status)
        })
    })
}

async fn retry_async<F, Fut, T>(verb: &str, url: &Url, retries: i64, mut operation: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let mut delays = retry_delays(retries);
    let mut attempt = 1;
    loop {
        let started_at = Instant::now();
        match operation().await {
            Ok(value) => return Ok(value),
            Err(error) if is_transient(&error) => {
                let Some(delay) = delays.next() else {
                    return Err(error);
                };
                warn!(
                    "HTTP {verb} {url} attempt {attempt} failed after {:?} (transient): {error}; retrying in {delay:?}",
                    started_at.elapsed()
                );
                tokio::time::sleep(delay).await;
                attempt += 1;
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
#[path = "core_tests.rs"]
mod tests;
