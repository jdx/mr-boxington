use mbx_cache_core::{
    ACTION_RESULT_MEDIA_TYPE, AGENT_PROTOCOL_VERSION, ActionPrediction, AgentRequest,
    AgentResponse, BLOB_MEDIA_TYPE, BLOB_PACK_MEDIA_TYPE, CLIENT_METADATA_MEDIA_TYPE, CacheDigest,
    CacheDirectory, CacheFileNode, DIRECTORY_MEDIA_TYPE, PROTOCOL_VERSION, RemoteActionResult,
    RestoreStats, RustcMetadata, TASK_ACTION_MANIFEST_MEDIA_TYPE, canonical_json,
};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

const V1_FIXTURE: &str = include_str!("fixtures/agent-protocol-v1.jsonl");

fn digest() -> CacheDigest {
    CacheDigest {
        algorithm: "blake3".into(),
        hash: "a".repeat(64),
        size: 7,
    }
}

fn restore() -> RestoreStats {
    RestoreStats {
        copied_output_bytes: 17,
        copied_output_files: 16,
        duration_ns: 11,
        output_files: 12,
        output_bytes: 13,
        reflinked_output_bytes: 15,
        reflinked_output_files: 14,
    }
}

fn result() -> RemoteActionResult {
    RemoteActionResult {
        action: digest(),
        metadata: None,
        output_root: Some(digest()),
        version: 1,
    }
}

fn prediction() -> ActionPrediction {
    ActionPrediction {
        invocation: digest(),
        action: digest(),
        adapter: "rustc".into(),
        payload: "{}".into(),
    }
}

fn environment() -> BTreeMap<String, Option<String>> {
    BTreeMap::from([
        ("RUSTFLAGS".into(), Some("-Cdebuginfo=1".into())),
        ("UNSET".into(), None),
    ])
}

fn requests() -> Vec<(&'static str, AgentRequest)> {
    vec![
        (
            "request.hello",
            AgentRequest::Hello {
                protocol: 1,
                client_version: "0.3.0".into(),
            },
        ),
        (
            "request.find_blob",
            AgentRequest::FindBlob { digest: digest() },
        ),
        (
            "request.find_blobs",
            AgentRequest::FindBlobs {
                digests: vec![digest()],
            },
        ),
        (
            "request.store_blob",
            AgentRequest::StoreBlob {
                digest: digest(),
                source: PathBuf::from("cache/blob"),
            },
        ),
        (
            "request.find_action_result",
            AgentRequest::FindActionResult { action: digest() },
        ),
        (
            "request.record_action_hit",
            AgentRequest::RecordActionHit {
                action: digest(),
                restore: restore(),
            },
        ),
        (
            "request.record_bypass",
            AgentRequest::RecordBypass {
                kind: "compiler-query".into(),
            },
        ),
        (
            "request.record_unconsulted",
            AgentRequest::RecordUnconsulted,
        ),
        (
            "request.record_action_verification",
            AgentRequest::RecordActionVerification {
                matched: true,
                restore: restore(),
            },
        ),
        (
            "request.store_action_result",
            AgentRequest::StoreActionResult { result: result() },
        ),
        (
            "request.find_action_prediction",
            AgentRequest::FindActionPrediction {
                task: "task-id".into(),
                invocation: digest(),
            },
        ),
        (
            "request.record_action_prediction",
            AgentRequest::RecordActionPrediction {
                task: "task-id".into(),
                prediction: prediction(),
            },
        ),
        (
            "request.find_executable_identity",
            AgentRequest::FindExecutableIdentity {
                executable: PathBuf::from("bin/rustc"),
                environment: environment(),
            },
        ),
        (
            "request.store_executable_identity",
            AgentRequest::StoreExecutableIdentity {
                executable: PathBuf::from("bin/rustc"),
                environment: environment(),
                stdout: b"rustc".to_vec(),
            },
        ),
    ]
}

fn responses() -> Vec<(&'static str, AgentResponse)> {
    vec![
        (
            "response.hello",
            AgentResponse::Hello {
                protocol: 1,
                agent_version: "0.3.0".into(),
            },
        ),
        (
            "response.blob",
            AgentResponse::Blob {
                path: Some(PathBuf::from("cache/blob")),
            },
        ),
        (
            "response.blobs",
            AgentResponse::Blobs {
                paths: vec![Some(PathBuf::from("cache/blob")), None],
            },
        ),
        (
            "response.stored",
            AgentResponse::Stored {
                path: PathBuf::from("cache/blob"),
            },
        ),
        (
            "response.action_result",
            AgentResponse::ActionResult {
                result: Some(result()),
            },
        ),
        (
            "response.action_hit_recorded",
            AgentResponse::ActionHitRecorded,
        ),
        (
            "response.action_verification_recorded",
            AgentResponse::ActionVerificationRecorded,
        ),
        ("response.bypass_recorded", AgentResponse::BypassRecorded),
        (
            "response.unconsulted_recorded",
            AgentResponse::UnconsultedRecorded,
        ),
        (
            "response.action_stored",
            AgentResponse::ActionStored {
                path: PathBuf::from("cache/action"),
            },
        ),
        (
            "response.action_prediction",
            AgentResponse::ActionPrediction {
                prediction: Some(prediction()),
            },
        ),
        (
            "response.action_prediction_recorded",
            AgentResponse::ActionPredictionRecorded,
        ),
        (
            "response.executable_identity",
            AgentResponse::ExecutableIdentity {
                stdout: Some(b"rustc".to_vec()),
            },
        ),
        (
            "response.error",
            AgentResponse::Error {
                message: "incompatible protocol".into(),
            },
        ),
    ]
}

fn fixture() -> BTreeMap<&'static str, &'static str> {
    V1_FIXTURE
        .lines()
        .map(|line| {
            line.split_once('\t')
                .expect("fixture line must contain a tab")
        })
        .collect()
}

fn assert_fixture<T: Serialize>(expected: &mut BTreeMap<&str, &str>, name: &str, value: &T) {
    let actual = String::from_utf8(canonical_json(value).unwrap()).unwrap();
    assert_eq!(
        actual,
        expected.remove(name).expect("fixture entry is missing"),
        "{name}"
    );
}

#[test]
fn v1_protocol_shapes_match_the_conformance_fixture() {
    let mut expected = fixture();
    for line in V1_FIXTURE.lines() {
        let (name, json) = line
            .split_once('\t')
            .expect("fixture line must contain a tab");
        if name.starts_with("request.") {
            request_variants_are_exhaustive(serde_json::from_str(json).unwrap());
        } else if name.starts_with("response.") {
            response_variants_are_exhaustive(serde_json::from_str(json).unwrap());
        }
    }
    for (name, request) in requests() {
        assert_fixture(&mut expected, name, &request);
    }
    for (name, response) in responses() {
        assert_fixture(&mut expected, name, &response);
    }
    assert_fixture(&mut expected, "record.action_result", &result());
    assert_fixture(
        &mut expected,
        "record.directory",
        &CacheDirectory {
            directories: vec![],
            files: vec![CacheFileNode {
                digest: digest(),
                executable: false,
                mode: 0o644,
                name: "lib.rlib".into(),
            }],
            symlinks: vec![],
            version: 1,
        },
    );
    assert_fixture(
        &mut expected,
        "record.rustc_metadata",
        &RustcMetadata {
            version: 1,
            kind: "rustc".into(),
            stdout: digest(),
            stderr: digest(),
        },
    );
    assert!(
        expected.is_empty(),
        "unexercised fixture entries: {expected:?}"
    );
}

#[test]
fn v1_protocol_constants_match_the_contract() {
    assert_eq!(AGENT_PROTOCOL_VERSION, 1);
    assert_eq!(PROTOCOL_VERSION, 1);
    assert_eq!(
        ACTION_RESULT_MEDIA_TYPE,
        "application/vnd.mbx.cache-action-result.v1+json"
    );
    assert_eq!(
        DIRECTORY_MEDIA_TYPE,
        "application/vnd.mbx.cache-directory.v1+json"
    );
    assert_eq!(
        CLIENT_METADATA_MEDIA_TYPE,
        "application/vnd.mbx.cache-client-metadata.v1+json"
    );
    assert_eq!(
        TASK_ACTION_MANIFEST_MEDIA_TYPE,
        "application/vnd.mbx.cache-task-action-manifest.v1+json"
    );
    assert_eq!(BLOB_MEDIA_TYPE, "application/octet-stream");
    assert_eq!(
        BLOB_PACK_MEDIA_TYPE,
        "application/vnd.mbx.cache-blob-pack.v1"
    );
}

fn request_variants_are_exhaustive(request: AgentRequest) {
    match request {
        AgentRequest::Hello { .. }
        | AgentRequest::FindBlob { .. }
        | AgentRequest::FindBlobs { .. }
        | AgentRequest::StoreBlob { .. }
        | AgentRequest::FindActionResult { .. }
        | AgentRequest::RecordActionHit { .. }
        | AgentRequest::RecordBypass { .. }
        | AgentRequest::RecordUnconsulted
        | AgentRequest::RecordActionVerification { .. }
        | AgentRequest::StoreActionResult { .. }
        | AgentRequest::FindActionPrediction { .. }
        | AgentRequest::RecordActionPrediction { .. }
        | AgentRequest::FindExecutableIdentity { .. }
        | AgentRequest::StoreExecutableIdentity { .. } => {}
    }
}

fn response_variants_are_exhaustive(response: AgentResponse) {
    match response {
        AgentResponse::Hello { .. }
        | AgentResponse::Blob { .. }
        | AgentResponse::Blobs { .. }
        | AgentResponse::Stored { .. }
        | AgentResponse::ActionResult { .. }
        | AgentResponse::ActionHitRecorded
        | AgentResponse::ActionVerificationRecorded
        | AgentResponse::BypassRecorded
        | AgentResponse::UnconsultedRecorded
        | AgentResponse::ActionStored { .. }
        | AgentResponse::ActionPrediction { .. }
        | AgentResponse::ActionPredictionRecorded
        | AgentResponse::ExecutableIdentity { .. }
        | AgentResponse::Error { .. } => {}
    }
}
