use super::*;
use crate::*;

fn config(server: &mockito::ServerGuard) -> GcsRemoteCacheConfig {
    GcsRemoteCacheConfig {
        bucket: "cache-bucket".into(),
        prefix: String::new(),
        namespace: "acme".into(),
        endpoint: Some(server.url().parse().unwrap()),
        token: Some("test-token".into()),
        token_file: None,
        connect_timeout: Duration::from_secs(1),
        read_timeout: Duration::from_secs(1),
        download_timeout: Duration::from_secs(1),
        retries: 0,
    }
}

fn test_store(server: &mockito::ServerGuard) -> GcsRemoteCache {
    GcsRemoteCache::new(config(server)).unwrap()
}

fn blob_read_path(digest: &CacheDigest) -> String {
    let raw_key = format!(
        "acme/v1/blobs/{}/{}/{}",
        digest.algorithm, digest.hash, digest.size
    );
    format!(
        "/storage/v1/b/cache-bucket/o/{}?alt=media",
        percent_encode_key(&raw_key)
    )
}

fn blob_upload_path(digest: &CacheDigest) -> String {
    let raw_key = format!(
        "acme/v1/blobs/{}/{}/{}",
        digest.algorithm, digest.hash, digest.size
    );
    let mut url = Url::parse("http://dummy/upload/storage/v1/b/cache-bucket/o").unwrap();
    let mut pairs = url.query_pairs_mut();
    pairs.append_pair("uploadType", "media");
    pairs.append_pair("name", &raw_key);
    pairs.append_pair("ifGenerationMatch", "0");
    drop(pairs);
    format!("{}?{}", url.path(), url.query().unwrap())
}

#[tokio::test]
async fn a_blob_round_trips_through_its_content_addressed_key() {
    let mut server = mockito::Server::new_async().await;
    let contents = b"a cached object file";
    let digest = CacheDigest::blake3(contents);

    let request = server
        .mock("GET", blob_read_path(&digest).as_str())
        .match_header("authorization", "Bearer test-token")
        .with_status(200)
        .with_body(contents)
        .expect(1)
        .create_async()
        .await;

    let staging = tempfile::tempdir().unwrap();
    let file = test_store(&server)
        .get_blob_file(&digest, staging.path())
        .await
        .unwrap();

    assert_eq!(fs::read(file.path()).unwrap(), contents);
    request.assert_async().await;
}

#[tokio::test]
async fn a_blob_in_memory_downloads_successfully() {
    let mut server = mockito::Server::new_async().await;
    let contents = b"small cached blob";
    let digest = CacheDigest::blake3(contents);

    let request = server
        .mock("GET", blob_read_path(&digest).as_str())
        .match_header("authorization", "Bearer test-token")
        .with_status(200)
        .with_body(contents)
        .expect(1)
        .create_async()
        .await;

    let bytes = test_store(&server)
        .get_blob(&digest, BLOB_MEDIA_TYPE)
        .await
        .unwrap();

    assert_eq!(bytes, contents);
    request.assert_async().await;
}

#[tokio::test]
async fn a_blob_that_fails_verification_is_refused() {
    let mut server = mockito::Server::new_async().await;
    let digest = CacheDigest::blake3(b"what was asked for");

    server
        .mock("GET", blob_read_path(&digest).as_str())
        .with_status(200)
        .with_body(b"something else!!!!")
        .create_async()
        .await;

    let staging = tempfile::tempdir().unwrap();
    let error = test_store(&server)
        .get_blob_file(&digest, staging.path())
        .await
        .unwrap_err();

    assert!(error.to_string().contains("failed digest verification"));
}

#[tokio::test]
async fn a_blob_larger_than_its_digest_stops_being_read() {
    let mut server = mockito::Server::new_async().await;
    let digest = CacheDigest::blake3(b"small");

    server
        .mock("GET", blob_read_path(&digest).as_str())
        .with_status(200)
        .with_body(vec![b'x'; 4096])
        .create_async()
        .await;

    let staging = tempfile::tempdir().unwrap();
    let error = test_store(&server)
        .get_blob_file(&digest, staging.path())
        .await
        .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("exceeded the size of its digest")
    );
}

#[tokio::test]
async fn storing_a_blob_writes_it_create_only() {
    let mut server = mockito::Server::new_async().await;
    let contents = b"a published object";
    let digest = CacheDigest::blake3(contents);

    let request = server
        .mock("POST", blob_upload_path(&digest).as_str())
        .match_header("authorization", "Bearer test-token")
        .match_header("content-type", "application/octet-stream")
        .match_body(contents.to_vec())
        .with_status(200)
        .expect(1)
        .create_async()
        .await;

    test_store(&server)
        .put_blob(&BlobUpload {
            digest,
            source: BlobSource::Bytes(contents.to_vec()),
        })
        .await
        .unwrap();

    request.assert_async().await;
}

#[tokio::test]
async fn storing_an_existing_blob_is_not_an_error() {
    let mut server = mockito::Server::new_async().await;
    let contents = b"already exists";
    let digest = CacheDigest::blake3(contents);

    server
        .mock("POST", blob_upload_path(&digest).as_str())
        .with_status(412) // Precondition Failed: generation match 0 failed
        .expect(1)
        .create_async()
        .await;

    test_store(&server)
        .put_blob(&BlobUpload {
            digest,
            source: BlobSource::Bytes(contents.to_vec()),
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn action_result_round_trips() {
    let mut server = mockito::Server::new_async().await;
    let action_digest = CacheDigest::blake3(b"action key");
    let result = RemoteActionResult {
        version: 1,
        action: action_digest.clone(),
        output_root: None,
        metadata: None,
    };
    let body = serde_json::to_vec(&result).unwrap();

    let raw_key = format!(
        "acme/v1/action-results/{}/{}/{}",
        action_digest.algorithm, action_digest.hash, action_digest.size
    );
    let read_path = format!(
        "/storage/v1/b/cache-bucket/o/{}?alt=media",
        percent_encode_key(&raw_key)
    );
    let mut upload_url = Url::parse("http://dummy/upload/storage/v1/b/cache-bucket/o").unwrap();
    let mut pairs = upload_url.query_pairs_mut();
    pairs.append_pair("uploadType", "media");
    pairs.append_pair("name", &raw_key);
    pairs.append_pair("ifGenerationMatch", "0");
    drop(pairs);
    let upload_path = format!("{}?{}", upload_url.path(), upload_url.query().unwrap());

    let put_req = server
        .mock("POST", upload_path.as_str())
        .match_header("content-type", "application/json")
        .match_body(body.clone())
        .with_status(200)
        .expect(1)
        .create_async()
        .await;

    let get_req = server
        .mock("GET", read_path.as_str())
        .with_status(200)
        .with_body(body)
        .expect(1)
        .create_async()
        .await;

    let store = test_store(&server);
    store.put_action_result(&result).await.unwrap();
    let fetched = store
        .get_action_result(&action_digest)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(fetched.action, action_digest);
    put_req.assert_async().await;
    get_req.assert_async().await;
}

#[tokio::test]
async fn action_result_missing_returns_none() {
    let mut server = mockito::Server::new_async().await;
    let action_digest = CacheDigest::blake3(b"missing action");
    let raw_key = format!(
        "acme/v1/action-results/{}/{}/{}",
        action_digest.algorithm, action_digest.hash, action_digest.size
    );
    let read_path = format!(
        "/storage/v1/b/cache-bucket/o/{}?alt=media",
        percent_encode_key(&raw_key)
    );

    server
        .mock("GET", read_path.as_str())
        .with_status(404)
        .expect(1)
        .create_async()
        .await;

    let fetched = test_store(&server)
        .get_action_result(&action_digest)
        .await
        .unwrap();
    assert!(fetched.is_none());
}

#[tokio::test]
async fn action_manifest_creation_and_conditional_update() {
    let mut server = mockito::Server::new_async().await;
    let key = CacheDigest::blake3(b"task manifest");
    let raw_key = format!(
        "acme/v1/action-manifests/{}/{}/{}",
        key.algorithm, key.hash, key.size
    );

    // Initial create (ifGenerationMatch=0)
    let mut create_url = Url::parse("http://dummy/upload/storage/v1/b/cache-bucket/o").unwrap();
    let mut pairs = create_url.query_pairs_mut();
    pairs.append_pair("uploadType", "media");
    pairs.append_pair("name", &raw_key);
    pairs.append_pair("ifGenerationMatch", "0");
    drop(pairs);
    let create_path = format!("{}?{}", create_url.path(), create_url.query().unwrap());

    let create_req = server
        .mock("POST", create_path.as_str())
        .with_status(200)
        .expect(1)
        .create_async()
        .await;

    let store = test_store(&server);
    let outcome = store.put_action_manifest(&key, b"{}", None).await.unwrap();
    assert_eq!(outcome, ManifestPutOutcome::Stored);
    create_req.assert_async().await;

    // GET manifest with ETag / generation
    let read_path = format!(
        "/storage/v1/b/cache-bucket/o/{}?alt=media",
        percent_encode_key(&raw_key)
    );
    let get_req = server
        .mock("GET", read_path.as_str())
        .with_status(200)
        .with_header("etag", "\"etag123\"")
        .with_header("x-goog-generation", "1700000001")
        .with_body(b"{}")
        .expect(1)
        .create_async()
        .await;

    let manifest = store.get_action_manifest(&key).await.unwrap().unwrap();
    assert_eq!(manifest.etag, "etag123");
    get_req.assert_async().await;

    // Conditional update with generation
    let mut update_url = Url::parse("http://dummy/upload/storage/v1/b/cache-bucket/o").unwrap();
    let mut pairs = update_url.query_pairs_mut();
    pairs.append_pair("uploadType", "media");
    pairs.append_pair("name", &raw_key);
    pairs.append_pair("ifGenerationMatch", "1700000001");
    drop(pairs);
    let update_path = format!("{}?{}", update_url.path(), update_url.query().unwrap());

    let update_req = server
        .mock("POST", update_path.as_str())
        .with_status(200)
        .expect(1)
        .create_async()
        .await;

    let outcome = store
        .put_action_manifest(&key, b"{\"updated\":true}", Some("1700000001"))
        .await
        .unwrap();
    assert_eq!(outcome, ManifestPutOutcome::Stored);
    update_req.assert_async().await;

    // Conditional update conflict (412 Precondition Failed)
    let update_conflict_req = server
        .mock("POST", update_path.as_str())
        .with_status(412)
        .expect(1)
        .create_async()
        .await;

    let outcome = store
        .put_action_manifest(&key, b"{\"conflict\":true}", Some("1700000001"))
        .await
        .unwrap();
    assert_eq!(outcome, ManifestPutOutcome::PreconditionFailed);
    update_conflict_req.assert_async().await;
}

#[tokio::test]
async fn connection_probe_passes_on_404() {
    let mut server = mockito::Server::new_async().await;
    let raw_key = format!("acme/v1/{CONNECTIVITY_PROBE_KEY}");
    let probe_path = format!(
        "/storage/v1/b/cache-bucket/o/{}?alt=media",
        percent_encode_key(&raw_key)
    );

    let req = server
        .mock("GET", probe_path.as_str())
        .match_header("authorization", "Bearer test-token")
        .with_status(404)
        .expect(1)
        .create_async()
        .await;

    test_store(&server).check_connection().await.unwrap();
    req.assert_async().await;
}

#[tokio::test]
async fn metadata_server_token_resolution() {
    let mut metadata_server = mockito::Server::new_async().await;
    let token_mock = metadata_server
        .mock(
            "GET",
            "/computeMetadata/v1/instance/service-accounts/default/token",
        )
        .match_header("Metadata-Flavor", "Google")
        .with_status(200)
        .with_body(r#"{"access_token": "metadata-secret-token", "expires_in": 3600}"#)
        .expect(1)
        .create_async()
        .await;

    // Configure GcsRemoteCache without explicit token
    let mut storage_server = mockito::Server::new_async().await;
    let digest = CacheDigest::blake3(b"metadata test");
    let storage_mock = storage_server
        .mock("GET", blob_read_path(&digest).as_str())
        .match_header("authorization", "Bearer metadata-secret-token")
        .with_status(200)
        .with_body(b"metadata test")
        .expect(1)
        .create_async()
        .await;

    unsafe {
        std::env::set_var(
            "GCP_METADATA_SERVER_URL",
            format!(
                "{}/computeMetadata/v1/instance/service-accounts/default/token",
                metadata_server.url()
            ),
        );
    }

    let gcs_config = GcsRemoteCacheConfig {
        bucket: "cache-bucket".into(),
        prefix: String::new(),
        namespace: "acme".into(),
        endpoint: Some(storage_server.url().parse().unwrap()),
        token: None,
        token_file: None,
        connect_timeout: Duration::from_secs(1),
        read_timeout: Duration::from_secs(1),
        download_timeout: Duration::from_secs(1),
        retries: 0,
    };
    let store = GcsRemoteCache::new(gcs_config).unwrap();
    let bytes = store.get_blob(&digest, BLOB_MEDIA_TYPE).await.unwrap();
    assert_eq!(bytes, b"metadata test");

    token_mock.assert_async().await;
    storage_mock.assert_async().await;

    unsafe {
        std::env::remove_var("GCP_METADATA_SERVER_URL");
    }
}

#[tokio::test]
async fn storing_a_mismatched_blob_source_is_rejected_before_post() {
    let server = mockito::Server::new_async().await;
    let expected_digest = CacheDigest::blake3(b"expected contents");
    let actual_contents = b"completely different contents";

    let error = test_store(&server)
        .put_blob(&BlobUpload {
            digest: expected_digest,
            source: BlobSource::Bytes(actual_contents.to_vec()),
        })
        .await
        .unwrap_err();

    assert!(error.to_string().contains("do not match expected digest"));
}
