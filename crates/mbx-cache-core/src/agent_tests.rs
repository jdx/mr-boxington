use super::*;
use crate::ACTION_RESULT_MEDIA_TYPE;
use std::time::Duration;

#[test]
fn directory_queue_counts_only_unique_unseen_nodes() {
    let shared = CacheDigest::blake3(b"shared");
    let first = CacheDigest::blake3(b"first");
    let second = CacheDigest::blake3(b"second");
    let overflow = CacheDigest::blake3(b"overflow");
    let seen = BTreeMap::from([(shared.clone(), ())]);
    let mut pending = BTreeMap::new();

    assert!(queue_prefetch_directory(&seen, &mut pending, shared, 3));
    assert!(pending.is_empty());
    assert!(queue_prefetch_directory(
        &seen,
        &mut pending,
        first.clone(),
        3
    ));
    assert!(queue_prefetch_directory(&seen, &mut pending, first, 3));
    assert!(queue_prefetch_directory(&seen, &mut pending, second, 3));
    assert!(!queue_prefetch_directory(&seen, &mut pending, overflow, 3));
    assert_eq!(pending.len(), 2);
}

async fn handshake(stream: &mut (impl AsyncRead + AsyncWrite + Unpin), version: &str) {
    let request = AgentRequest::Hello {
        protocol: AGENT_PROTOCOL_VERSION,
        client_version: version.to_string(),
    };
    let mut encoded = serde_json::to_vec(&request).unwrap();
    encoded.push(b'\n');
    stream.write_all(&encoded).await.unwrap();
    stream.flush().await.unwrap();
    let mut response = String::new();
    BufReader::new(stream)
        .read_line(&mut response)
        .await
        .unwrap();
    assert!(matches!(
        serde_json::from_str(&response).unwrap(),
        AgentResponse::Hello { .. }
    ));
}

#[tokio::test]
async fn rejects_a_request_that_never_ends_its_line() {
    let directory = tempfile::tempdir().unwrap();
    let agent = CacheAgent::new(directory.path().join("cache"), "test-version");
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let task = tokio::spawn(async move { agent.handle_connection(server).await });

    handshake(&mut client, "test-version").await;
    // Never send a newline: the agent must give up rather than buffer
    // whatever a peer is willing to write.
    let filler = vec![b'x'; 64 * 1024];
    let mut written = 0usize;
    while written <= MAX_REQUEST_BYTES {
        if client.write_all(&filler).await.is_err() {
            break;
        }
        written += filler.len();
    }

    let error = task.await.unwrap().err().unwrap();
    assert!(
        error.to_string().contains("exceeded"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn counts_compilations_it_could_not_look_up() {
    let directory = tempfile::tempdir().unwrap();
    let agent = CacheAgent::new(directory.path().join("cache"), "test-version");

    for _ in 0..3 {
        agent.respond(AgentRequest::RecordUnconsulted).await;
    }

    let stats = agent.stats();
    assert_eq!(stats.unconsulted, 3);
    // Not a miss: nothing was looked up, and a hit rate computed over these
    // would be a rate over lookups that never happened.
    assert_eq!(stats.lookups, 0);
    assert_eq!(stats.hits, 0);
}

#[tokio::test]
async fn records_compiler_time_by_outcome_and_crate() {
    let directory = tempfile::tempdir().unwrap();
    let agent = CacheAgent::new(directory.path().join("cache"), "test-version");
    for (outcome, crate_name, duration_ns) in [
        ("miss", Some("slow_crate"), 17),
        ("miss", Some("slow_crate"), 5),
        ("bypass", Some("linked_bin"), 11),
        ("verification", Some("not_uncached"), 7),
    ] {
        let response = agent
            .respond(AgentRequest::RecordCompilerInvocation {
                outcome: outcome.into(),
                crate_name: crate_name.map(str::to_string),
                duration_ns,
            })
            .await;
        assert!(matches!(
            response,
            AgentResponse::CompilerInvocationRecorded
        ));
    }

    let stats = agent.stats();
    assert_eq!(stats.compiler["miss"].invocations, 2);
    assert_eq!(stats.compiler["miss"].duration_ns, 22);
    assert_eq!(stats.compiler["bypass"].duration_ns, 11);
    assert_eq!(stats.slow_compilations["slow_crate"], 22);
    assert_eq!(stats.slow_compilations["linked_bin"], 11);
    assert!(!stats.slow_compilations.contains_key("not_uncached"));
}

#[tokio::test]
async fn counts_bypasses_by_reason() {
    let directory = tempfile::tempdir().unwrap();
    let agent = CacheAgent::new(directory.path().join("cache"), "test-version");

    for kind in [
        "unsupported-crate-type",
        "unsupported-crate-type",
        "incremental",
    ] {
        agent
            .respond(AgentRequest::RecordBypass { kind: kind.into() })
            .await;
    }

    let stats = agent.stats();
    assert_eq!(stats.bypasses.get("unsupported-crate-type"), Some(&2));
    assert_eq!(stats.bypasses.get("incremental"), Some(&1));
}

#[tokio::test]
async fn handshake_and_blob_round_trip() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    std::fs::write(&source, b"cached object").unwrap();
    let digest = CacheDigest::blake3(b"cached object");
    let agent = CacheAgent::new(directory.path().join("cache"), "test-version");
    let (mut client, server) = tokio::io::duplex(16 * 1024);
    let server_agent = agent.clone();
    let task = tokio::spawn(async move { server_agent.handle_connection(server).await });

    handshake(&mut client, "test-version").await;
    let request = AgentRequest::StoreBlob {
        digest: digest.clone(),
        source,
    };
    let mut encoded = serde_json::to_vec(&request).unwrap();
    encoded.push(b'\n');
    client.write_all(&encoded).await.unwrap();
    let mut response = String::new();
    BufReader::new(&mut client)
        .read_line(&mut response)
        .await
        .unwrap();
    assert!(matches!(
        serde_json::from_str(&response).unwrap(),
        AgentResponse::Stored { .. }
    ));
    drop(client);
    task.await.unwrap().unwrap();
    assert_eq!(
        agent.stats(),
        AgentStats {
            stores: 1,
            stored_bytes: digest.size,
            ..AgentStats::default()
        }
    );
}

#[test]
fn remembered_blobs_reject_same_size_corruption() {
    let directory = tempfile::tempdir().unwrap();
    let agent = CacheAgent::new(directory.path().join("cache"), "test-version");
    let digest = CacheDigest::blake3(b"cached object");
    let path = agent.cas.store_bytes(&digest, b"cached object").unwrap();
    assert_eq!(
        agent.find_verified_blob(&digest).unwrap(),
        Some(path.clone())
    );

    std::fs::write(&path, b"broken object").unwrap();

    assert!(agent.find_verified_blob(&digest).is_err());
    assert!(!agent.verified_blobs.lock().unwrap().contains_key(&digest));
}

#[derive(Default)]
struct RecordingObserver {
    events: Mutex<Vec<AgentEvent>>,
}

impl AgentEventObserver for RecordingObserver {
    fn event(&self, event: AgentEvent) {
        self.events.lock().unwrap().push(event);
    }
}

#[tokio::test]
async fn reports_each_accounted_decision_to_an_observer() {
    let directory = tempfile::tempdir().unwrap();
    let observer = Arc::new(RecordingObserver::default());
    let agent = CacheAgent::new(directory.path().join("cache"), "test-version")
        .with_observer(observer.clone());

    agent
        .respond(AgentRequest::RecordBypass {
            kind: "incremental".into(),
        })
        .await;
    agent.respond(AgentRequest::RecordUnconsulted).await;
    agent
        .respond(AgentRequest::RecordCompilerInvocation {
            outcome: "miss".into(),
            crate_name: Some("serde".into()),
            duration_ns: 42,
        })
        .await;
    agent
        .respond(AgentRequest::RecordActionVerification {
            matched: false,
            restore: RestoreStats::default(),
        })
        .await;

    let events = observer.events.lock().unwrap();
    assert!(matches!(
        events.as_slice(),
        [
            AgentEvent::Bypass { kind },
            AgentEvent::Unconsulted,
            AgentEvent::CompilerInvocation {
                outcome,
                crate_name: Some(crate_name),
                duration_ns: 42,
            },
            AgentEvent::Verification { matched: false, .. },
        ] if kind == "incremental" && outcome == "miss" && crate_name == "serde"
    ));
}

#[tokio::test]
async fn a_rejected_hit_reports_no_event() {
    let directory = tempfile::tempdir().unwrap();
    let observer = Arc::new(RecordingObserver::default());
    let agent = CacheAgent::new(directory.path().join("cache"), "test-version")
        .with_observer(observer.clone());

    // A hit for an action the store never had is an error, and an error is not
    // an outcome an observer should see.
    let response = agent
        .respond(AgentRequest::RecordActionHit {
            action: CacheDigest::blake3(b"absent"),
            restore: RestoreStats::default(),
            crate_name: Some("serde".into()),
        })
        .await;

    assert!(matches!(response, AgentResponse::Error { .. }));
    assert!(observer.events.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_hit_carrying_an_unusable_crate_name_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let agent = CacheAgent::new(directory.path().join("cache"), "test-version");

    let response = agent
        .respond(AgentRequest::RecordActionHit {
            action: CacheDigest::blake3(b"action"),
            restore: RestoreStats::default(),
            crate_name: Some("serde\nrustc".into()),
        })
        .await;

    assert!(matches!(
        response,
        AgentResponse::Error { message } if message.contains("invalid compiler crate name")
    ));
}

#[tokio::test]
async fn publishes_a_complete_action_result() {
    let directory = tempfile::tempdir().unwrap();
    let agent = CacheAgent::new(directory.path().join("cache"), "test-version");
    let action = CacheDigest::blake3(b"action");
    let metadata = CacheDigest::blake3(b"metadata");
    let output_root = CacheDigest::blake3(b"directory");
    for (digest, contents) in [
        (&action, b"action".as_slice()),
        (&metadata, b"metadata".as_slice()),
        (&output_root, b"directory".as_slice()),
    ] {
        agent.cas.store_bytes(digest, contents).unwrap();
    }
    let response = agent
        .respond(AgentRequest::StoreActionResult {
            result: RemoteActionResult {
                action: action.clone(),
                metadata: Some(metadata),
                output_root: Some(output_root),
                version: 1,
            },
        })
        .await;
    assert!(matches!(response, AgentResponse::ActionStored { .. }));
    let response = agent
        .respond(AgentRequest::FindActionResult {
            action: action.clone(),
        })
        .await;
    assert!(matches!(
        response,
        AgentResponse::ActionResult {
            result: Some(result)
        } if result.action == action
    ));
    assert!(matches!(
        agent
            .respond(AgentRequest::RecordActionHit {
                action: action.clone(),
                restore: RestoreStats {
                    duration_ns: 7,
                    avoided_compiler_duration_ns: 13,
                    output_files: 2,
                    output_bytes: 11,
                    reflinked_output_files: 1,
                    reflinked_output_bytes: 7,
                    copied_output_files: 1,
                    copied_output_bytes: 4,
                },
                crate_name: None,
            })
            .await,
        AgentResponse::ActionHitRecorded
    ));
    assert_eq!(
        agent.stats(),
        AgentStats {
            lookups: 1,
            hits: 1,
            materialization_duration_ns: 7,
            restored_output_files: 2,
            restored_output_bytes: 11,
            reflinked_output_files: 1,
            reflinked_output_bytes: 7,
            copied_output_files: 1,
            copied_output_bytes: 4,
            avoided_compiler_duration_ns: 13,
            ..AgentStats::default()
        }
    );
}

#[tokio::test]
async fn missing_action_result_is_a_cache_miss() {
    let directory = tempfile::tempdir().unwrap();
    let agent = CacheAgent::new(directory.path(), "test-version");
    let action = CacheDigest::blake3(b"missing action");
    let response = agent
        .respond(AgentRequest::FindActionResult {
            action: action.clone(),
        })
        .await;

    assert!(matches!(
        response,
        AgentResponse::ActionResult { result: None }
    ));
    assert!(matches!(
        agent
            .respond(AgentRequest::RecordActionHit {
                action,
                restore: RestoreStats::default(),
                crate_name: None,
            })
            .await,
        AgentResponse::Error { .. }
    ));
    assert_eq!(
        agent.stats(),
        AgentStats {
            lookups: 1,
            ..AgentStats::default()
        }
    );

    assert!(matches!(
        agent
            .respond(AgentRequest::RecordActionVerification {
                matched: false,
                restore: RestoreStats {
                    duration_ns: 7,
                    output_files: 2,
                    output_bytes: 11,
                    ..RestoreStats::default()
                },
            })
            .await,
        AgentResponse::ActionVerificationRecorded
    ));
    assert_eq!(agent.stats().verifications, 1);
    assert_eq!(agent.stats().divergences, 1);
    assert_eq!(agent.stats().materialization_duration_ns, 7);
    assert_eq!(agent.stats().restored_output_files, 0);
    assert_eq!(agent.stats().restored_output_bytes, 0);
}

#[tokio::test]
async fn coalesces_repeated_remote_action_lookups() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = mockito::Server::new_async().await;
    let action = CacheDigest::blake3(b"remote action");
    let result = RemoteActionResult {
        action: action.clone(),
        metadata: None,
        output_root: None,
        version: 1,
    };
    let remote = server
        .mock("GET", action_path(&action).as_str())
        .with_status(200)
        .with_header("content-type", ACTION_RESULT_MEDIA_TYPE)
        .with_body(serde_json::to_vec(&result).unwrap())
        .expect(1)
        .create_async()
        .await;
    let agent = remote_agent(
        &server,
        directory.path().join("reader"),
        RemoteCacheMode::ReadOnly,
    );

    for _ in 0..2 {
        assert!(matches!(
            agent
                .respond(AgentRequest::FindActionResult {
                    action: action.clone(),
                })
                .await,
            AgentResponse::ActionResult {
                result: Some(found)
            } if found == result
        ));
    }
    remote.assert_async().await;
}

#[tokio::test]
async fn publishes_only_successfully_committed_task_action_manifests() {
    let directory = tempfile::tempdir().unwrap();
    let cache = directory.path().join("cache");
    let task = "a".repeat(64);
    let first_invocation = CacheDigest::blake3(b"first invocation");
    let first = ActionPrediction {
        invocation: first_invocation.clone(),
        action: CacheDigest::blake3(b"first action"),
        adapter: "rustc".into(),
        payload: "{}".into(),
    };

    let agent = CacheAgent::new(&cache, "test-version");
    let first_run = agent.begin_task(&task).await.unwrap();
    assert!(matches!(
        agent
            .respond(AgentRequest::RecordActionPrediction {
                task: first_run.clone(),
                prediction: first.clone(),
            })
            .await,
        AgentResponse::ActionPredictionRecorded
    ));
    agent.commit_task(&first_run).await.unwrap();

    let uncommitted = CacheAgent::new(&cache, "test-version");
    let uncommitted_run = uncommitted.begin_task(&task).await.unwrap();
    let second_invocation = CacheDigest::blake3(b"second invocation");
    assert!(matches!(
        uncommitted
            .respond(AgentRequest::RecordActionPrediction {
                task: uncommitted_run,
                prediction: ActionPrediction {
                    invocation: second_invocation.clone(),
                    action: CacheDigest::blake3(b"second action"),
                    adapter: "rustc".into(),
                    payload: "{}".into(),
                },
            })
            .await,
        AgentResponse::ActionPredictionRecorded
    ));

    let next_session = CacheAgent::new(&cache, "test-version");
    let next_run = next_session.begin_task(&task).await.unwrap();
    assert!(matches!(
        next_session
            .respond(AgentRequest::FindActionPrediction {
                task: next_run.clone(),
                invocation: first_invocation,
            })
            .await,
        AgentResponse::ActionPrediction {
            prediction: Some(prediction)
        } if prediction == first
    ));
    assert!(matches!(
        next_session
            .respond(AgentRequest::FindActionPrediction {
                task: next_run,
                invocation: second_invocation,
            })
            .await,
        AgentResponse::ActionPrediction { prediction: None }
    ));

    let corrupt_task = "b".repeat(64);
    fs::create_dir_all(next_session.manifest_dir.as_path()).unwrap();
    fs::write(next_session.task_manifest_path(&corrupt_task), b"not json").unwrap();
    assert!(next_session.begin_task(&corrupt_task).await.is_err());
    let corrupt_run = "c".repeat(64);
    next_session.task_actions.lock().unwrap().insert(
        corrupt_run.clone(),
        TaskActionState {
            manifest: corrupt_task.clone(),
            ..TaskActionState::default()
        },
    );
    assert!(matches!(
        next_session
            .respond(AgentRequest::RecordActionPrediction {
                task: corrupt_run.clone(),
                prediction: first,
            })
            .await,
        AgentResponse::ActionPredictionRecorded
    ));
    assert!(next_session.commit_task(&corrupt_run).await.is_err());
    assert_eq!(
        fs::read(next_session.task_manifest_path(&corrupt_task)).unwrap(),
        b"not json"
    );
}

#[tokio::test]
async fn concurrent_task_commits_do_not_republish_stale_baselines() {
    let directory = tempfile::tempdir().unwrap();
    let cache = directory.path().join("cache");
    let task = "d".repeat(64);
    let first_invocation = CacheDigest::blake3(b"first invocation");
    let old = ActionPrediction {
        invocation: first_invocation.clone(),
        action: CacheDigest::blake3(b"old action"),
        adapter: "rustc".into(),
        payload: "{}".into(),
    };

    let seed = CacheAgent::new(&cache, "test-version");
    let seed_run = seed.begin_task(&task).await.unwrap();
    assert!(matches!(
        seed.respond(AgentRequest::RecordActionPrediction {
            task: seed_run.clone(),
            prediction: old,
        })
        .await,
        AgentResponse::ActionPredictionRecorded
    ));
    seed.commit_task(&seed_run).await.unwrap();

    // Both agents load the old value before either publishes. The second
    // commit must merge only its new entry, not its stale baseline.
    let first = CacheAgent::new(&cache, "test-version");
    let second = CacheAgent::new(&cache, "test-version");
    let first_run = first.begin_task(&task).await.unwrap();
    let second_run = second.begin_task(&task).await.unwrap();
    let updated = ActionPrediction {
        invocation: first_invocation.clone(),
        action: CacheDigest::blake3(b"updated action"),
        adapter: "rustc".into(),
        payload: "{}".into(),
    };
    assert!(matches!(
        first
            .respond(AgentRequest::RecordActionPrediction {
                task: first_run.clone(),
                prediction: updated.clone(),
            })
            .await,
        AgentResponse::ActionPredictionRecorded
    ));
    first.commit_task(&first_run).await.unwrap();

    let second_prediction = ActionPrediction {
        invocation: CacheDigest::blake3(b"second invocation"),
        action: CacheDigest::blake3(b"second action"),
        adapter: "rustc".into(),
        payload: "{}".into(),
    };
    assert!(matches!(
        second
            .respond(AgentRequest::RecordActionPrediction {
                task: second_run.clone(),
                prediction: second_prediction.clone(),
            })
            .await,
        AgentResponse::ActionPredictionRecorded
    ));
    second.commit_task(&second_run).await.unwrap();

    let reader = CacheAgent::new(&cache, "test-version");
    let reader_run = reader.begin_task(&task).await.unwrap();
    assert!(matches!(
        reader
            .respond(AgentRequest::FindActionPrediction {
                task: reader_run.clone(),
                invocation: first_invocation,
            })
            .await,
        AgentResponse::ActionPrediction {
            prediction: Some(prediction)
        } if prediction == updated
    ));
    assert!(matches!(
        reader
            .respond(AgentRequest::FindActionPrediction {
                task: reader_run,
                invocation: second_prediction.invocation.clone(),
            })
            .await,
        AgentResponse::ActionPrediction {
            prediction: Some(prediction)
        } if prediction == second_prediction
    ));
}

#[tokio::test]
async fn round_trips_task_actions_between_fresh_local_caches() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = mockito::Server::new_async().await;
    let task = "e".repeat(64);
    let invocation = CacheDigest::blake3(b"remote invocation");
    let action_bytes = canonical_json(&serde_json::json!({"kind":"rustc"})).unwrap();
    let stdout_bytes = b"cached stdout".to_vec();
    let stderr_bytes = b"cached stderr".to_vec();
    let artifact_bytes = b"cached artifact".to_vec();
    let stdout = CacheDigest::blake3(&stdout_bytes);
    let stderr = CacheDigest::blake3(&stderr_bytes);
    let artifact = CacheDigest::blake3(&artifact_bytes);
    let metadata_bytes = canonical_json(&RustcMetadata {
        version: 1,
        kind: "rustc".into(),
        stdout: stdout.clone(),
        stderr: stderr.clone(),
    })
    .unwrap();
    let directory_bytes = canonical_json(&serde_json::json!({
        "directories":[],
        "files":[{"digest":artifact,"executable":false,"mode":420,"name":"artifact"}],
        "symlinks":[],
        "version":1
    }))
    .unwrap();
    let action = CacheDigest::blake3(&action_bytes);
    let metadata = CacheDigest::blake3(&metadata_bytes);
    let output_root = CacheDigest::blake3(&directory_bytes);
    let result = RemoteActionResult {
        action: action.clone(),
        metadata: Some(metadata.clone()),
        output_root: Some(output_root.clone()),
        version: 1,
    };
    let prediction = ActionPrediction {
        invocation: invocation.clone(),
        action: action.clone(),
        adapter: "rustc".into(),
        payload: "{}".into(),
    };
    let manifest_bytes = canonical_json(&TaskActionManifest {
        version: TASK_ACTION_MANIFEST_VERSION,
        task: task.clone(),
        predictions: vec![prediction.clone()],
    })
    .unwrap();
    let manifest_etag = blake3::hash(&manifest_bytes).to_hex().to_string();
    let (_, selector) = CacheAgent::task_manifest_selector(&task).unwrap();

    let mut mocks = Vec::new();
    for (digest, bytes) in [
        (&action, action_bytes.as_slice()),
        (&metadata, metadata_bytes.as_slice()),
        (&output_root, directory_bytes.as_slice()),
        (&stdout, stdout_bytes.as_slice()),
        (&stderr, stderr_bytes.as_slice()),
        (&artifact, artifact_bytes.as_slice()),
    ] {
        mocks.push(
            server
                .mock("PUT", blob_path(digest).as_str())
                .match_header("mbx-cache-namespace", "test")
                .match_body(bytes.to_vec())
                .with_status(200)
                .expect(1)
                .create_async()
                .await,
        );
    }
    mocks.push(
        server
            .mock("PUT", action_path(&result.action).as_str())
            .match_header("mbx-cache-namespace", "test")
            .with_status(200)
            .expect(1)
            .create_async()
            .await,
    );
    mocks.push(
        server
            .mock("PUT", action_manifest_path(&selector).as_str())
            .match_header("mbx-cache-namespace", "test")
            .match_header("if-none-match", "*")
            .match_body(manifest_bytes.clone())
            .with_status(201)
            .expect(1)
            .create_async()
            .await,
    );
    mocks.push(
        server
            .mock("GET", action_manifest_path(&selector).as_str())
            .with_status(200)
            .with_header("etag", &format!("\"{manifest_etag}\""))
            .with_body(manifest_bytes.clone())
            .expect(1)
            .create_async()
            .await,
    );
    mocks.push(
        server
            .mock("GET", action_path(&action).as_str())
            .with_status(200)
            .with_header("content-type", ACTION_RESULT_MEDIA_TYPE)
            .with_body(serde_json::to_vec(&result).unwrap())
            .expect(1)
            .create_async()
            .await,
    );
    for (digest, bytes) in [
        (&action, action_bytes.as_slice()),
        (&metadata, metadata_bytes.as_slice()),
        (&output_root, directory_bytes.as_slice()),
        (&stdout, stdout_bytes.as_slice()),
        (&stderr, stderr_bytes.as_slice()),
        (&artifact, artifact_bytes.as_slice()),
    ] {
        mocks.push(
            server
                .mock("GET", blob_path(digest).as_str())
                .with_status(200)
                .with_body(bytes)
                .expect(1)
                .create_async()
                .await,
        );
    }

    let writer = remote_agent(
        &server,
        directory.path().join("writer"),
        RemoteCacheMode::WriteOnly,
    );
    for (index, (digest, bytes)) in [
        (&action, action_bytes.as_slice()),
        (&metadata, metadata_bytes.as_slice()),
        (&output_root, directory_bytes.as_slice()),
        (&stdout, stdout_bytes.as_slice()),
        (&stderr, stderr_bytes.as_slice()),
        (&artifact, artifact_bytes.as_slice()),
    ]
    .into_iter()
    .enumerate()
    {
        let source = directory.path().join(format!("source-{index}"));
        fs::write(&source, bytes).unwrap();
        assert!(matches!(
            writer
                .respond(AgentRequest::StoreBlob {
                    digest: digest.clone(),
                    source,
                })
                .await,
            AgentResponse::Stored { .. }
        ));
    }
    assert!(matches!(
        writer
            .respond(AgentRequest::StoreActionResult {
                result: result.clone(),
            })
            .await,
        AgentResponse::ActionStored { .. }
    ));
    let run = writer.begin_task(&task).await.unwrap();
    assert!(matches!(
        writer
            .respond(AgentRequest::RecordActionPrediction {
                task: run.clone(),
                prediction: prediction.clone(),
            })
            .await,
        AgentResponse::ActionPredictionRecorded
    ));
    writer.commit_task(&run).await.unwrap();

    let reader = remote_agent(
        &server,
        directory.path().join("reader"),
        RemoteCacheMode::ReadOnly,
    );
    let run = reader.begin_task(&task).await.unwrap();
    reader.wait_for_prefetches().await;
    assert!(matches!(
        reader
            .respond(AgentRequest::FindActionPrediction {
                task: run,
                invocation,
            })
            .await,
        AgentResponse::ActionPrediction {
            prediction: Some(found)
        } if found == prediction
    ));
    assert!(matches!(
        reader
            .respond(AgentRequest::FindActionResult {
                action: action.clone(),
            })
            .await,
        AgentResponse::ActionResult {
            result: Some(found)
        } if found == result
    ));
    for digest in [&action, &metadata, &output_root] {
        assert!(matches!(
            reader
                .respond(AgentRequest::FindBlob {
                    digest: digest.clone(),
                })
                .await,
            AgentResponse::Blob { path: Some(_) }
        ));
    }
    assert!(matches!(
        reader
            .respond(AgentRequest::RecordActionHit {
                action,
                restore: RestoreStats::default(),
                crate_name: None,
            })
            .await,
        AgentResponse::ActionHitRecorded
    ));
    for mock in mocks {
        mock.assert_async().await;
    }
    let stats = reader.stats();
    assert_eq!(stats.prefetch_runs, 1);
    assert_eq!(stats.prefetched_actions, 1);
    assert!(stats.remote_manifest_lookups > 0);
    assert!(stats.remote_action_lookups > 0);
    assert!(stats.remote_blob_requests > 0);
    assert!(stats.remote_manifest_lookup_duration_ns > 0);
    assert!(stats.remote_action_lookup_duration_ns > 0);
    assert!(stats.remote_blob_transfer_duration_ns > 0);
    assert!(stats.local_cas_write_duration_ns > 0);
    assert!(stats.prefetch_duration_ns > 0);
}

#[tokio::test]
async fn keeps_newer_local_predictions_when_remote_manifest_is_stale() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = mockito::Server::new_async().await;
    let task = "f".repeat(64);
    let invocation = CacheDigest::blake3(b"shared invocation");
    let local_prediction = ActionPrediction {
        invocation: invocation.clone(),
        action: CacheDigest::blake3(b"new local action"),
        adapter: "rustc".into(),
        payload: "{}".into(),
    };
    let remote_prediction = ActionPrediction {
        invocation: invocation.clone(),
        action: CacheDigest::blake3(b"stale remote action"),
        adapter: "rustc".into(),
        payload: "{}".into(),
    };
    let remote_manifest = TaskActionManifest {
        version: TASK_ACTION_MANIFEST_VERSION,
        task: task.clone(),
        predictions: vec![remote_prediction],
    };
    let remote_bytes = canonical_json(&remote_manifest).unwrap();
    let remote_etag = blake3::hash(&remote_bytes).to_hex().to_string();
    let (_, selector) = CacheAgent::task_manifest_selector(&task).unwrap();
    let remote = server
        .mock("GET", action_manifest_path(&selector).as_str())
        .with_status(200)
        .with_header("etag", &format!("\"{remote_etag}\""))
        .with_body(remote_bytes)
        .expect(1)
        .create_async()
        .await;

    let agent = remote_agent(
        &server,
        directory.path().join("reader"),
        RemoteCacheMode::ReadOnly,
    );
    agent
        .persist_task_manifest(&TaskActionManifest {
            version: TASK_ACTION_MANIFEST_VERSION,
            task: task.clone(),
            predictions: vec![local_prediction.clone()],
        })
        .unwrap();

    let run = agent.begin_task(&task).await.unwrap();
    assert!(matches!(
        agent
            .respond(AgentRequest::FindActionPrediction {
                task: run,
                invocation,
            })
            .await,
        AgentResponse::ActionPrediction {
            prediction: Some(found)
        } if found == local_prediction
    ));
    let persisted = agent.load_task_manifest(&task).unwrap().unwrap();
    assert_eq!(persisted.predictions, vec![local_prediction]);
    remote.assert_async().await;
}

#[tokio::test]
async fn prefetch_does_not_block_task_initialization() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = mockito::Server::new_async().await;
    let task = "9".repeat(64);
    let invocation = CacheDigest::blake3(b"prefetched invocation");
    let action_bytes = b"prefetched action";
    let action = CacheDigest::blake3(action_bytes);
    let result = RemoteActionResult {
        action: action.clone(),
        metadata: None,
        output_root: None,
        version: 1,
    };
    let manifest_bytes = canonical_json(&TaskActionManifest {
        version: TASK_ACTION_MANIFEST_VERSION,
        task: task.clone(),
        predictions: vec![ActionPrediction {
            invocation,
            action: action.clone(),
            adapter: "rustc".into(),
            payload: "{}".into(),
        }],
    })
    .unwrap();
    let manifest_etag = blake3::hash(&manifest_bytes).to_hex().to_string();
    let (_, selector) = CacheAgent::task_manifest_selector(&task).unwrap();
    let manifest = server
        .mock("GET", action_manifest_path(&selector).as_str())
        .with_status(200)
        .with_header("etag", &format!("\"{manifest_etag}\""))
        .with_body(manifest_bytes)
        .expect(1)
        .create_async()
        .await;
    let release = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let response_release = release.clone();
    let result_bytes = serde_json::to_vec(&result).unwrap();
    let action_result = server
        .mock("GET", action_path(&action).as_str())
        .with_status(200)
        .with_header("content-type", ACTION_RESULT_MEDIA_TYPE)
        .with_chunked_body(move |writer| {
            let (released, condition) = &*response_release;
            let mut released = released.lock().unwrap();
            while !*released {
                released = condition.wait(released).unwrap();
            }
            std::io::Write::write_all(writer, &result_bytes)
        })
        .expect(1)
        .create_async()
        .await;
    let action_blob = server
        .mock("GET", blob_path(&action).as_str())
        .with_status(200)
        .with_body(action_bytes)
        .expect(1)
        .create_async()
        .await;
    let agent = remote_agent(
        &server,
        directory.path().join("reader"),
        RemoteCacheMode::ReadOnly,
    );

    let begin = tokio::time::timeout(Duration::from_secs(2), agent.begin_task(&task)).await;
    let (released, condition) = &*release;
    *released.lock().unwrap() = true;
    condition.notify_all();
    let run = begin
        .expect("task initialization waited for prefetch")
        .unwrap();
    assert_eq!(
        agent
            .task_actions
            .lock()
            .unwrap()
            .get(&run)
            .unwrap()
            .predictions
            .len(),
        1
    );
    agent.wait_for_prefetches().await;
    manifest.assert_async().await;
    action_result.assert_async().await;
    action_blob.assert_async().await;
    assert!(agent.actions.find(&action).unwrap().is_some());
}

#[tokio::test]
async fn prefetches_complete_actions_in_directory_wave_blob_packs() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = mockito::Server::new_async().await;
    let action_bytes = b"packed action descriptor";
    let stdout_bytes = b"packed stdout";
    let stderr_bytes = b"packed stderr";
    let artifact_bytes = b"packed artifact";
    let action = CacheDigest::blake3(action_bytes);
    let stdout = CacheDigest::blake3(stdout_bytes);
    let stderr = CacheDigest::blake3(stderr_bytes);
    let artifact = CacheDigest::blake3(artifact_bytes);
    let metadata_bytes = canonical_json(&RustcMetadata {
        version: 1,
        kind: "rustc".into(),
        stdout: stdout.clone(),
        stderr: stderr.clone(),
    })
    .unwrap();
    let metadata = CacheDigest::blake3(&metadata_bytes);
    let directory_bytes = canonical_json(&serde_json::json!({
        "directories": [],
        "files": [{
            "digest": artifact,
            "executable": false,
            "mode": 420,
            "name": "artifact",
        }],
        "symlinks": [],
        "version": 1,
    }))
    .unwrap();
    let output_root = CacheDigest::blake3(&directory_bytes);
    let result = RemoteActionResult {
        action: action.clone(),
        metadata: Some(metadata.clone()),
        output_root: Some(output_root.clone()),
        version: 1,
    };
    let action_result = server
        .mock("GET", action_path(&action).as_str())
        .with_status(200)
        .with_header("content-type", ACTION_RESULT_MEDIA_TYPE)
        .with_body(serde_json::to_vec(&result).unwrap())
        .expect(1)
        .create_async()
        .await;
    let capabilities = server
        .mock("GET", "/v1/capabilities")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            serde_json::json!({
                "protocol":{"major":1},
                "features":{"blob_packs":true},
                "limits":{"max_batch_items":100,"max_pack_bytes":1048576}
            })
            .to_string(),
        )
        .expect(1)
        .create_async()
        .await;
    let mut top = vec![
        (action.clone(), action_bytes.as_slice()),
        (metadata.clone(), metadata_bytes.as_slice()),
        (output_root.clone(), directory_bytes.as_slice()),
    ];
    top.sort_by(|left, right| left.0.cmp(&right.0));
    let first_pack = server
        .mock("POST", "/v1/blobs:pack")
        .match_body(mockito::Matcher::Json(serde_json::json!({
            "digests": top.iter().map(|(digest, _)| digest).collect::<Vec<_>>()
        })))
        .with_status(200)
        .with_header("content-type", crate::BLOB_PACK_MEDIA_TYPE)
        .with_body(blob_pack_body(&top))
        .expect(1)
        .create_async()
        .await;
    let mut leaves = vec![
        (stdout.clone(), stdout_bytes.as_slice()),
        (stderr.clone(), stderr_bytes.as_slice()),
        (artifact.clone(), artifact_bytes.as_slice()),
    ];
    leaves.sort_by(|left, right| left.0.cmp(&right.0));
    let second_pack = server
        .mock("POST", "/v1/blobs:pack")
        .match_body(mockito::Matcher::Json(serde_json::json!({
            "digests": leaves.iter().map(|(digest, _)| digest).collect::<Vec<_>>()
        })))
        .with_status(200)
        .with_header("content-type", crate::BLOB_PACK_MEDIA_TYPE)
        .with_body(blob_pack_body(&leaves))
        .expect(1)
        .create_async()
        .await;
    let agent = remote_agent(
        &server,
        directory.path().join("reader"),
        RemoteCacheMode::ReadOnly,
    );

    agent
        .prefetch_action(action.clone(), "rustc".into())
        .await
        .unwrap();

    assert_eq!(agent.actions.find(&action).unwrap(), Some(result));
    let stats = agent.stats();
    assert_eq!(stats.prefetched_actions, 1);
    assert_eq!(stats.remote_blob_requests, 0);
    assert_eq!(stats.remote_blob_pack_requests, 2);
    assert_eq!(stats.remote_blob_pack_blobs, 6);
    action_result.assert_async().await;
    capabilities.assert_async().await;
    first_pack.assert_async().await;
    second_pack.assert_async().await;
}

#[tokio::test]
async fn foreground_blob_batches_use_blob_packs() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = mockito::Server::new_async().await;
    let mut entries = [
        (CacheDigest::blake3(b"first"), b"first".as_slice()),
        (CacheDigest::blake3(b"second"), b"second".as_slice()),
    ];
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    let requested = entries
        .iter()
        .map(|(digest, _)| digest.clone())
        .collect::<Vec<_>>();
    let response_requested = vec![
        entries[0].0.clone(),
        entries[1].0.clone(),
        entries[0].0.clone(),
    ];
    let capabilities = server
        .mock("GET", "/v1/capabilities")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            serde_json::json!({
                "protocol":{"major":1},
                "features":{"blob_packs":true},
                "limits":{"max_batch_items":100,"max_pack_bytes":1048576}
            })
            .to_string(),
        )
        .expect(1)
        .create_async()
        .await;
    let pack = server
        .mock("POST", "/v1/blobs:pack")
        .match_body(mockito::Matcher::Json(serde_json::json!({
            "digests": requested.clone()
        })))
        .with_status(200)
        .with_header("content-type", crate::BLOB_PACK_MEDIA_TYPE)
        .with_body(blob_pack_body(&entries))
        .expect(1)
        .create_async()
        .await;
    let agent = remote_agent(
        &server,
        directory.path().join("reader"),
        RemoteCacheMode::ReadOnly,
    );
    let response = agent
        .respond(AgentRequest::FindBlobs {
            digests: response_requested,
        })
        .await;

    let AgentResponse::Blobs { paths } = response else {
        panic!("unexpected blob lookup response");
    };
    assert_eq!(paths.len(), 3);
    for (expected, path) in [entries[0].1, entries[1].1, entries[0].1]
        .into_iter()
        .zip(paths)
    {
        assert_eq!(fs::read(path.unwrap()).unwrap(), expected);
    }
    let stats = agent.stats();
    assert_eq!(stats.remote_blob_requests, 0);
    assert_eq!(stats.remote_blob_pack_requests, 1);
    assert_eq!(stats.remote_blob_pack_blobs, 2);
    capabilities.assert_async().await;
    pack.assert_async().await;
}

#[tokio::test]
async fn pack_and_individual_fetches_share_a_digest_lock() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = mockito::Server::new_async().await;
    let bytes = b"one remote transfer";
    let digest = CacheDigest::blake3(bytes);
    let capabilities = server
        .mock("GET", "/v1/capabilities")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            serde_json::json!({
                "protocol":{"major":1},
                "features":{"blob_packs":true},
                "limits":{"max_batch_items":100,"max_pack_bytes":1048576}
            })
            .to_string(),
        )
        .expect(1)
        .create_async()
        .await;
    let started = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let release = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let response_started = started.clone();
    let response_release = release.clone();
    let pack_body = blob_pack_body(&[(digest.clone(), bytes.as_slice())]);
    let pack = server
        .mock("POST", "/v1/blobs:pack")
        .with_status(200)
        .with_header("content-type", crate::BLOB_PACK_MEDIA_TYPE)
        .with_chunked_body(move |writer| {
            response_started.store(true, Ordering::Release);
            while !response_release.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(10));
            }
            std::io::Write::write_all(writer, &pack_body)
        })
        .expect(1)
        .create_async()
        .await;
    let individual = server
        .mock("GET", blob_path(&digest).as_str())
        .with_status(200)
        .with_body(bytes)
        .expect(0)
        .create_async()
        .await;
    let agent = remote_agent(
        &server,
        directory.path().join("reader"),
        RemoteCacheMode::ReadOnly,
    );

    let pack_agent = agent.clone();
    let pack_digest = digest.clone();
    let pack_fetch = tokio::spawn(async move {
        let remote = pack_agent.remote.as_deref().unwrap();
        pack_agent
            .fetch_remote_blobs(remote, vec![pack_digest], None)
            .await
    });
    tokio::time::timeout(Duration::from_secs(30), async {
        while !started.load(Ordering::Acquire) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("pack request did not start");

    let foreground_agent = agent.clone();
    let foreground_digest = digest.clone();
    let foreground_fetch = tokio::spawn(async move {
        let remote = foreground_agent.remote.as_deref().unwrap();
        foreground_agent
            .fetch_remote_blob(remote, &foreground_digest)
            .await
    });
    release.store(true, Ordering::Release);

    let packed = pack_fetch.await.unwrap();
    let foreground = foreground_fetch.await.unwrap().unwrap();
    assert_eq!(fs::read(&packed[&digest]).unwrap(), bytes);
    assert_eq!(fs::read(foreground).unwrap(), bytes);
    assert_eq!(
        agent.remote_download_bytes.load(Ordering::Relaxed),
        digest.size
    );
    capabilities.assert_async().await;
    pack.assert_async().await;
    individual.assert_async().await;
}

#[tokio::test]
async fn a_smaller_server_pack_cap_does_not_block_later_candidates() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = mockito::Server::new_async().await;
    let oversized = CacheDigest {
        algorithm: "blake3".into(),
        hash: "00".repeat(32),
        size: MAX_STAGED_BLOB_PACK_BYTES,
    };
    let packed_bytes = b"fit";
    let packed = CacheDigest::blake3(packed_bytes);
    let capabilities = server
        .mock("GET", "/v1/capabilities")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            serde_json::json!({
                "protocol":{"major":1},
                "features":{"blob_packs":true},
                "limits":{"max_batch_items":100,"max_pack_bytes":4}
            })
            .to_string(),
        )
        .expect(1)
        .create_async()
        .await;
    let pack = server
        .mock("POST", "/v1/blobs:pack")
        .match_body(mockito::Matcher::Json(serde_json::json!({
            "digests": [&packed]
        })))
        .with_status(200)
        .with_header("content-type", crate::BLOB_PACK_MEDIA_TYPE)
        .with_body(blob_pack_body(&[(packed.clone(), packed_bytes.as_slice())]))
        .expect(1)
        .create_async()
        .await;
    let fallback = server
        .mock("GET", blob_path(&oversized).as_str())
        .with_status(404)
        .expect(1)
        .create_async()
        .await;
    let agent = remote_agent(
        &server,
        directory.path().join("reader"),
        RemoteCacheMode::ReadOnly,
    );
    let remote = agent.remote.as_deref().unwrap();

    let verified = agent
        .fetch_remote_blobs(remote, vec![oversized, packed.clone()], None)
        .await;

    assert_eq!(verified.len(), 1);
    assert_eq!(fs::read(&verified[&packed]).unwrap(), packed_bytes);
    capabilities.assert_async().await;
    pack.assert_async().await;
    fallback.assert_async().await;
}

#[tokio::test]
async fn preserves_successful_pack_metrics_when_a_later_chunk_falls_back() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = mockito::Server::new_async().await;
    let mut entries = [
        (CacheDigest::blake3(b"first"), b"first".as_slice()),
        (CacheDigest::blake3(b"second"), b"second".as_slice()),
    ];
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    let (first_digest, first_bytes) = entries[0].clone();
    let (second_digest, second_bytes) = entries[1].clone();
    let capabilities = server
        .mock("GET", "/v1/capabilities")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            serde_json::json!({
                "protocol":{"major":1},
                "features":{"blob_packs":true},
                "limits":{"max_batch_items":1,"max_pack_bytes":1048576}
            })
            .to_string(),
        )
        .expect(1)
        .create_async()
        .await;
    let first_pack = server
        .mock("POST", "/v1/blobs:pack")
        .match_body(mockito::Matcher::Json(serde_json::json!({
            "digests": [&first_digest]
        })))
        .with_status(200)
        .with_header("content-type", crate::BLOB_PACK_MEDIA_TYPE)
        .with_body(blob_pack_body(&[(first_digest.clone(), first_bytes)]))
        .expect(1)
        .create_async()
        .await;
    let failed_pack = server
        .mock("POST", "/v1/blobs:pack")
        .match_body(mockito::Matcher::Json(serde_json::json!({
            "digests": [&second_digest]
        })))
        .with_status(500)
        .expect(1)
        .create_async()
        .await;
    let fallback = server
        .mock("GET", blob_path(&second_digest).as_str())
        .with_status(200)
        .with_body(second_bytes)
        .expect(1)
        .create_async()
        .await;
    let agent = remote_agent(
        &server,
        directory.path().join("reader"),
        RemoteCacheMode::ReadOnly,
    );
    let remote = agent.remote.as_deref().unwrap();

    let verified = agent
        .fetch_remote_blobs(
            remote,
            vec![first_digest.clone(), second_digest.clone()],
            Some(&agent.prefetch_transfers),
        )
        .await;

    assert_eq!(verified.len(), 2);
    assert_eq!(fs::read(&verified[&first_digest]).unwrap(), first_bytes);
    assert_eq!(fs::read(&verified[&second_digest]).unwrap(), second_bytes);
    let stats = agent.stats();
    assert_eq!(stats.remote_blob_pack_requests, 1);
    assert_eq!(stats.remote_blob_pack_blobs, 1);
    assert_eq!(stats.remote_blob_requests, 1);
    capabilities.assert_async().await;
    first_pack.assert_async().await;
    failed_pack.assert_async().await;
    fallback.assert_async().await;
}

#[tokio::test]
async fn malformed_blob_pack_metadata_falls_back_to_individual_blobs() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = mockito::Server::new_async().await;
    let bytes = b"fallback blob";
    let digest = CacheDigest::blake3(bytes);
    server
        .mock("GET", "/v1/capabilities")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            serde_json::json!({
                "protocol":{"major":1},
                "features":{"blob_packs":true},
                "limits":{"max_batch_items":100,"max_pack_bytes":1048576}
            })
            .to_string(),
        )
        .expect(1)
        .create_async()
        .await;
    let pack = server
        .mock("POST", "/v1/blobs:pack")
        .with_status(200)
        .with_header("content-type", crate::BLOB_PACK_MEDIA_TYPE)
        .with_header(crate::BLOB_PACK_BYTES_HEADER, "not-a-number")
        .with_body(blob_pack_body(&[(digest.clone(), bytes.as_slice())]))
        .expect(1)
        .create_async()
        .await;
    let fallback = server
        .mock("GET", blob_path(&digest).as_str())
        .with_status(200)
        .with_body(bytes)
        .expect(1)
        .create_async()
        .await;
    let agent = remote_agent(
        &server,
        directory.path().join("reader"),
        RemoteCacheMode::ReadOnly,
    );
    let remote = agent.remote.as_deref().unwrap();

    let verified = agent
        .fetch_remote_blobs(remote, vec![digest.clone()], None)
        .await;

    assert_eq!(fs::read(&verified[&digest]).unwrap(), bytes);
    let stats = agent.stats();
    assert_eq!(stats.remote_blob_pack_requests, 0);
    assert_eq!(stats.remote_blob_pack_blobs, 0);
    assert_eq!(stats.remote_blob_requests, 1);
    pack.assert_async().await;
    fallback.assert_async().await;
}

#[tokio::test]
async fn foreground_action_lookup_does_not_wait_for_prefetch_output() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = mockito::Server::new_async().await;
    let action_bytes = b"prefetched action";
    let artifact_bytes = b"prefetched artifact";
    let action = CacheDigest::blake3(action_bytes);
    let artifact = CacheDigest::blake3(artifact_bytes);
    let directory_bytes = canonical_json(&serde_json::json!({
        "directories": [],
        "files": [{
            "digest": artifact,
            "executable": false,
            "mode": 420,
            "name": "artifact",
        }],
        "symlinks": [],
        "version": 1,
    }))
    .unwrap();
    let output_root = CacheDigest::blake3(&directory_bytes);
    let result = RemoteActionResult {
        action: action.clone(),
        metadata: None,
        output_root: Some(output_root.clone()),
        version: 1,
    };
    let action_result = server
        .mock("GET", action_path(&action).as_str())
        .with_status(200)
        .with_header("content-type", ACTION_RESULT_MEDIA_TYPE)
        .with_body(serde_json::to_vec(&result).unwrap())
        .expect(1)
        .create_async()
        .await;
    let action_blob = server
        .mock("GET", blob_path(&action).as_str())
        .with_status(200)
        .with_body(action_bytes)
        .expect(1)
        .create_async()
        .await;
    let output_directory = server
        .mock("GET", blob_path(&output_root).as_str())
        .with_status(200)
        .with_body(directory_bytes)
        .expect(1)
        .create_async()
        .await;
    let started = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let release = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let response_started = started.clone();
    let response_release = release.clone();
    let artifact_blob = server
        .mock("GET", blob_path(&artifact).as_str())
        .with_status(200)
        .with_chunked_body(move |writer| {
            response_started.store(true, Ordering::Release);
            while !response_release.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(10));
            }
            std::io::Write::write_all(writer, artifact_bytes)
        })
        .expect(1)
        .create_async()
        .await;
    let agent = remote_agent(
        &server,
        directory.path().join("reader"),
        RemoteCacheMode::ReadOnly,
    );
    let prefetch_agent = agent.clone();
    let prefetch_action = action.clone();
    let prefetch = tokio::spawn(async move {
        prefetch_agent
            .prefetch_action(prefetch_action, "rustc".into())
            .await
    });
    // The prefetch reaches the output blob only after three round trips
    // against the mock server, and the foreground lookup below makes three of
    // its own. Both budgets are only ever spent when something hangs, so they
    // are generous: what this test proves is that the foreground lookup does
    // not block on the held-open artifact response, not that either side is
    // fast. A tight bound instead measures the runner, and a contended
    // windows-latest agent loses that race while behaving correctly.
    tokio::time::timeout(Duration::from_secs(30), async {
        while !started.load(Ordering::Acquire) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("prefetch did not request the output blob");

    let foreground =
        tokio::time::timeout(Duration::from_secs(30), agent.find_action_result(&action)).await;
    release.store(true, Ordering::Release);
    prefetch.await.unwrap().unwrap();
    let foreground = foreground.expect("foreground action lookup waited for output prefetch");

    assert!(matches!(
        foreground.unwrap(),
        AgentResponse::ActionResult {
            result: Some(found)
        } if found == result
    ));
    action_result.assert_async().await;
    action_blob.assert_async().await;
    output_directory.assert_async().await;
    artifact_blob.assert_async().await;
}

#[tokio::test]
async fn session_completion_cancels_outstanding_prefetches() {
    let directory = tempfile::tempdir().unwrap();
    let agent = CacheAgent::new(directory.path(), "test-version");
    let task = tokio::spawn(std::future::pending::<()>());
    agent.prefetch_tasks.lock().unwrap().push(task);

    tokio::time::timeout(Duration::from_secs(1), agent.cancel_prefetches())
        .await
        .expect("prefetch cancellation blocked session completion");
    assert!(agent.prefetch_tasks.lock().unwrap().is_empty());
}

/// A blocking response is what makes this deterministic: were the store waiting
/// for its upload, it could not return while the server is still holding the
/// response open.
#[tokio::test]
async fn storing_a_blob_returns_before_its_remote_upload() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = mockito::Server::new_async().await;
    let bytes = b"deferred blob".to_vec();
    let digest = CacheDigest::blake3(&bytes);
    let release = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let response_release = release.clone();
    let upload = server
        .mock("PUT", blob_path(&digest).as_str())
        .with_status(200)
        .with_chunked_body(move |writer| {
            let (released, condition) = &*response_release;
            let mut released = released.lock().unwrap();
            while !*released {
                released = condition.wait(released).unwrap();
            }
            std::io::Write::write_all(writer, b"")
        })
        .expect(1)
        .create_async()
        .await;
    let agent = remote_agent(
        &server,
        directory.path().join("writer"),
        RemoteCacheMode::WriteOnly,
    );
    let source = directory.path().join("source");
    fs::write(&source, &bytes).unwrap();

    let stored = tokio::time::timeout(
        Duration::from_secs(2),
        agent.respond(AgentRequest::StoreBlob {
            digest: digest.clone(),
            source,
        }),
    )
    .await
    .expect("storing a blob waited for its remote upload");
    assert!(matches!(stored, AgentResponse::Stored { .. }));
    assert!(agent.cas.find(&digest).unwrap().is_some());

    let (released, condition) = &*release;
    *released.lock().unwrap() = true;
    condition.notify_all();
    agent.wait_for_uploads().await;
    upload.assert_async().await;
    let stats = agent.stats();
    assert_eq!(stats.background_uploads, 1);
    assert_eq!(stats.background_upload_failures, 0);
    assert_eq!(stats.uploaded_bytes, digest.size);
}

#[tokio::test]
async fn an_action_result_is_published_after_the_blobs_it_references() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = mockito::Server::new_async().await;
    let action_bytes = b"ordered action".to_vec();
    let action = CacheDigest::blake3(&action_bytes);
    let result = RemoteActionResult {
        action: action.clone(),
        metadata: None,
        output_root: None,
        version: 1,
    };
    // Recorded as each request arrives rather than as each response is written:
    // an upload finishes without reading the response body, so the body is no
    // signal for when the client moved on. Each matcher guards on its own path,
    // because mockito may consult a matcher for a request another mock answers.
    let arrivals = Arc::new(std::sync::Mutex::new(Vec::new()));
    let blob_arrivals = arrivals.clone();
    let blob_route = blob_path(&action);
    let matched_blob_route = blob_route.clone();
    let blob = server
        .mock("PUT", blob_route.as_str())
        .match_request(move |request| {
            if request.path() == matched_blob_route {
                blob_arrivals.lock().unwrap().push("blob");
            }
            true
        })
        .with_status(200)
        .expect(1)
        .create_async()
        .await;
    let result_arrivals = arrivals.clone();
    let action_route = action_path(&action);
    let matched_action_route = action_route.clone();
    let action_result = server
        .mock("PUT", action_route.as_str())
        .match_request(move |request| {
            if request.path() == matched_action_route {
                result_arrivals.lock().unwrap().push("action result");
            }
            true
        })
        .with_status(200)
        .expect(1)
        .create_async()
        .await;
    let agent = remote_agent(
        &server,
        directory.path().join("writer"),
        RemoteCacheMode::WriteOnly,
    );
    let source = directory.path().join("source");
    fs::write(&source, &action_bytes).unwrap();

    let responses = agent
        .handle_requests([
            AgentRequest::StoreBlob {
                digest: action.clone(),
                source,
            },
            AgentRequest::StoreActionResult {
                result: result.clone(),
            },
        ])
        .await;
    assert!(matches!(responses[0], AgentResponse::Stored { .. }));
    assert!(matches!(responses[1], AgentResponse::ActionStored { .. }));
    agent.wait_for_uploads().await;

    blob.assert_async().await;
    action_result.assert_async().await;
    assert_eq!(
        *arrivals.lock().unwrap(),
        vec!["blob", "action result"],
        "an action result reached the server before a blob it references"
    );
}

/// A failed re-upload must not retract what an earlier session advertised.
///
/// The conditional manifest write replaces the remote manifest when its entity
/// tag still matches, so dropping an inherited prediction would un-advertise a
/// result that is plausibly still on the server.
#[tokio::test]
async fn a_task_manifest_keeps_inherited_predictions_whose_upload_fails() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = mockito::Server::new_async().await;
    let task = "4".repeat(64);
    let action_bytes = b"inherited action".to_vec();
    let action = CacheDigest::blake3(&action_bytes);
    let prediction = ActionPrediction {
        invocation: CacheDigest::blake3(b"inherited invocation"),
        action: action.clone(),
        adapter: "rustc".into(),
        payload: "{}".into(),
    };
    let manifest_bytes = canonical_json(&TaskActionManifest {
        version: TASK_ACTION_MANIFEST_VERSION,
        task: task.clone(),
        predictions: vec![prediction.clone()],
    })
    .unwrap();
    let manifest_etag = blake3::hash(&manifest_bytes).to_hex().to_string();
    let (_, selector) = CacheAgent::task_manifest_selector(&task).unwrap();
    // The prediction is already advertised remotely when this session starts.
    server
        .mock("GET", action_manifest_path(&selector).as_str())
        .with_status(200)
        .with_header("etag", &format!("\"{manifest_etag}\""))
        .with_body(manifest_bytes.clone())
        .create_async()
        .await;
    server
        .mock("GET", action_path(&action).as_str())
        .with_status(404)
        .create_async()
        .await;
    server
        .mock("PUT", blob_path(&action).as_str())
        .with_status(500)
        .create_async()
        .await;
    let manifest = server
        .mock("PUT", action_manifest_path(&selector).as_str())
        .match_body(manifest_bytes)
        .with_status(201)
        .expect(1)
        .create_async()
        .await;
    let agent = remote_agent(
        &server,
        directory.path().join("writer"),
        RemoteCacheMode::ReadWrite,
    );
    let run = agent.begin_task(&task).await.unwrap();
    agent.wait_for_prefetches().await;
    let source = directory.path().join("source");
    fs::write(&source, &action_bytes).unwrap();
    agent
        .handle_requests([
            AgentRequest::StoreBlob {
                digest: action.clone(),
                source,
            },
            AgentRequest::StoreActionResult {
                result: RemoteActionResult {
                    action: action.clone(),
                    metadata: None,
                    output_root: None,
                    version: 1,
                },
            },
            AgentRequest::RecordActionPrediction {
                task: run.clone(),
                prediction,
            },
        ])
        .await;
    agent.commit_task(&run).await.unwrap();
    agent.wait_for_uploads().await;

    manifest.assert_async().await;
}

#[tokio::test]
async fn a_task_manifest_omits_predictions_that_were_not_published() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = mockito::Server::new_async().await;
    let task = "5".repeat(64);
    let published_bytes = b"published action".to_vec();
    let withheld_bytes = b"withheld action".to_vec();
    let published = CacheDigest::blake3(&published_bytes);
    let withheld = CacheDigest::blake3(&withheld_bytes);
    let predictions: Vec<ActionPrediction> = [&published, &withheld]
        .into_iter()
        .enumerate()
        .map(|(index, action)| ActionPrediction {
            invocation: CacheDigest::blake3(format!("invocation {index}").as_bytes()),
            action: action.clone(),
            adapter: "rustc".into(),
            payload: "{}".into(),
        })
        .collect();
    server
        .mock("PUT", blob_path(&published).as_str())
        .with_status(200)
        .create_async()
        .await;
    // This blob never lands, so the action result naming it is withheld, and a
    // manifest advertising that action would send readers after nothing.
    server
        .mock("PUT", blob_path(&withheld).as_str())
        .with_status(500)
        .create_async()
        .await;
    server
        .mock("PUT", action_path(&published).as_str())
        .with_status(200)
        .create_async()
        .await;
    let (_, selector) = CacheAgent::task_manifest_selector(&task).unwrap();
    let expected = canonical_json(&TaskActionManifest {
        version: TASK_ACTION_MANIFEST_VERSION,
        task: task.clone(),
        predictions: vec![predictions[0].clone()],
    })
    .unwrap();
    let manifest = server
        .mock("PUT", action_manifest_path(&selector).as_str())
        .match_body(expected)
        .with_status(201)
        .expect(1)
        .create_async()
        .await;
    let agent = remote_agent(
        &server,
        directory.path().join("writer"),
        RemoteCacheMode::WriteOnly,
    );
    let run = agent.begin_task(&task).await.unwrap();
    for (index, (action, bytes)) in [(&published, &published_bytes), (&withheld, &withheld_bytes)]
        .into_iter()
        .enumerate()
    {
        let source = directory.path().join(format!("source-{index}"));
        fs::write(&source, bytes).unwrap();
        agent
            .handle_requests([
                AgentRequest::StoreBlob {
                    digest: action.clone(),
                    source,
                },
                AgentRequest::StoreActionResult {
                    result: RemoteActionResult {
                        action: action.clone(),
                        metadata: None,
                        output_root: None,
                        version: 1,
                    },
                },
                AgentRequest::RecordActionPrediction {
                    task: run.clone(),
                    prediction: predictions[index].clone(),
                },
            ])
            .await;
    }
    agent.commit_task(&run).await.unwrap();
    agent.wait_for_uploads().await;

    manifest.assert_async().await;
    // The local manifest keeps both: this checkout can still use what it built.
    let local = agent.load_task_manifest(&task).unwrap().unwrap();
    assert_eq!(local.predictions.len(), 2);
}

#[tokio::test]
async fn a_blob_that_fails_to_upload_withholds_its_action_result() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = mockito::Server::new_async().await;
    let action_bytes = b"unpublishable action".to_vec();
    let action = CacheDigest::blake3(&action_bytes);
    let result = RemoteActionResult {
        action: action.clone(),
        metadata: None,
        output_root: None,
        version: 1,
    };
    let blob = server
        .mock("PUT", blob_path(&action).as_str())
        .with_status(500)
        .expect(1)
        .create_async()
        .await;
    // A server validates an action result against the blobs it references, so
    // sending this one would be rejected anyway.
    let action_result = server
        .mock("PUT", action_path(&action).as_str())
        .with_status(200)
        .expect(0)
        .create_async()
        .await;
    let agent = remote_agent(
        &server,
        directory.path().join("writer"),
        RemoteCacheMode::WriteOnly,
    );
    let source = directory.path().join("source");
    fs::write(&source, &action_bytes).unwrap();

    agent
        .handle_requests([
            AgentRequest::StoreBlob {
                digest: action.clone(),
                source,
            },
            AgentRequest::StoreActionResult { result },
        ])
        .await;
    agent.wait_for_uploads().await;

    blob.assert_async().await;
    action_result.assert_async().await;
    // The build keeps its local result: a failed upload costs hit rate, not
    // correctness.
    assert!(agent.actions.find(&action).unwrap().is_some());
    let stats = agent.stats();
    assert_eq!(stats.background_uploads, 0);
    assert!(stats.remote_failures > 0);
}

/// A blob that failed once must not withhold everything that follows.
///
/// Compilations share blobs -- every empty stdout is the same object -- so a
/// settled failure reused for the rest of the session would let one transient
/// error suppress every action result after it.
#[tokio::test]
async fn a_blob_that_failed_is_uploaded_again_for_a_later_result() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = mockito::Server::new_async().await;
    let shared_bytes = b"shared across compilations".to_vec();
    let shared = CacheDigest::blake3(&shared_bytes);
    let second_action = CacheDigest::blake3(b"the compilation after the failure");
    let failed = server
        .mock("PUT", blob_path(&shared).as_str())
        .with_status(500)
        .expect(1)
        .create_async()
        .await;
    let agent = remote_agent(
        &server,
        directory.path().join("writer"),
        RemoteCacheMode::WriteOnly,
    );
    let store_shared = |index: usize| {
        let source = directory.path().join(format!("source-{index}"));
        fs::write(&source, &shared_bytes).unwrap();
        AgentRequest::StoreBlob {
            digest: shared.clone(),
            source,
        }
    };

    // The first compilation's upload of the shared blob fails, so its result is
    // withheld.
    agent
        .handle_requests([
            store_shared(0),
            AgentRequest::StoreActionResult {
                result: RemoteActionResult {
                    action: shared.clone(),
                    metadata: None,
                    output_root: None,
                    version: 1,
                },
            },
        ])
        .await;
    agent.wait_for_uploads().await;
    failed.assert_async().await;

    // A later compilation stores the same bytes and must get its own attempt.
    let retried = server
        .mock("PUT", blob_path(&shared).as_str())
        .with_status(200)
        .expect(1)
        .create_async()
        .await;
    let second_blob = server
        .mock("PUT", blob_path(&second_action).as_str())
        .with_status(200)
        .expect(1)
        .create_async()
        .await;
    let second_result = server
        .mock("PUT", action_path(&second_action).as_str())
        .with_status(200)
        .expect(1)
        .create_async()
        .await;
    let source = directory.path().join("second-action");
    fs::write(&source, b"the compilation after the failure").unwrap();
    agent
        .handle_requests([
            store_shared(1),
            AgentRequest::StoreBlob {
                digest: second_action.clone(),
                source,
            },
            AgentRequest::StoreActionResult {
                result: RemoteActionResult {
                    action: second_action.clone(),
                    metadata: None,
                    output_root: None,
                    version: 1,
                },
            },
        ])
        .await;
    agent.wait_for_uploads().await;

    retried.assert_async().await;
    second_blob.assert_async().await;
    second_result.assert_async().await;
}

#[tokio::test]
async fn a_repeated_blob_is_uploaded_once() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = mockito::Server::new_async().await;
    let bytes = b"shared blob".to_vec();
    let digest = CacheDigest::blake3(&bytes);
    let upload = server
        .mock("PUT", blob_path(&digest).as_str())
        .with_status(200)
        .expect(1)
        .create_async()
        .await;
    let agent = remote_agent(
        &server,
        directory.path().join("writer"),
        RemoteCacheMode::WriteOnly,
    );
    for index in 0..3 {
        let source = directory.path().join(format!("source-{index}"));
        fs::write(&source, &bytes).unwrap();
        agent
            .respond(AgentRequest::StoreBlob {
                digest: digest.clone(),
                source,
            })
            .await;
    }
    agent.wait_for_uploads().await;

    upload.assert_async().await;
    assert_eq!(agent.stats().background_uploads, 1);
}

#[tokio::test]
async fn a_session_that_cannot_write_queues_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let server = mockito::Server::new_async().await;
    let agent = remote_agent(
        &server,
        directory.path().join("reader"),
        RemoteCacheMode::ReadOnly,
    );
    assert!(agent.uploads.is_none());

    let bytes = b"local only".to_vec();
    let digest = CacheDigest::blake3(&bytes);
    let source = directory.path().join("source");
    fs::write(&source, &bytes).unwrap();
    agent
        .respond(AgentRequest::StoreBlob {
            digest: digest.clone(),
            source,
        })
        .await;
    // No mock is registered, so an upload attempt would fail the request.
    tokio::time::timeout(Duration::from_secs(2), agent.wait_for_uploads())
        .await
        .expect("draining a read-only session blocked");
    assert_eq!(agent.stats().background_uploads, 0);
    assert_eq!(agent.stats().remote_failures, 0);
}

#[tokio::test]
async fn a_collected_blob_is_skipped_rather_than_retried() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = mockito::Server::new_async().await;
    let bytes = b"collected blob".to_vec();
    let digest = CacheDigest::blake3(&bytes);
    let upload = server
        .mock("PUT", blob_path(&digest).as_str())
        .with_status(200)
        .expect(0)
        .create_async()
        .await;
    let agent = remote_agent(
        &server,
        directory.path().join("writer"),
        RemoteCacheMode::WriteOnly,
    );
    let source = directory.path().join("source");
    fs::write(&source, &bytes).unwrap();
    agent
        .respond(AgentRequest::StoreBlob {
            digest: digest.clone(),
            source,
        })
        .await;
    // Stand in for a collection between the store and the upload.
    fs::remove_file(agent.cas.find(&digest).unwrap().unwrap()).unwrap();
    agent.wait_for_uploads().await;

    upload.assert_async().await;
    let stats = agent.stats();
    assert_eq!(stats.background_uploads, 0);
    assert_eq!(stats.background_upload_failures, 1);
}

#[tokio::test]
async fn prefetch_reserves_capacity_for_foreground_transfers() {
    let transfers = tokio::sync::Semaphore::new(MAX_REMOTE_TRANSFERS);
    let _prefetch = transfers
        .acquire_many(MAX_PREFETCH_TRANSFERS as u32)
        .await
        .unwrap();
    assert!(transfers.available_permits() > 0);
}

#[tokio::test]
async fn prefetches_output_files_concurrently() {
    let directory = tempfile::tempdir().unwrap();
    let (responses, output_root) = output_tree_responses(8);
    let (base_url, maximum_in_flight, server) =
        delayed_blob_server(responses, Duration::from_millis(50)).await;
    let agent = remote_agent_url(
        base_url,
        directory.path().join("reader"),
        RemoteCacheMode::ReadOnly,
    );

    agent
        .prefetch_output_tree(agent.remote.as_deref().unwrap(), &output_root)
        .await
        .unwrap();
    server.await.unwrap();

    assert!(maximum_in_flight.load(Ordering::Relaxed) > 1);
}

#[tokio::test]
#[ignore = "local remote-cache throughput benchmark"]
async fn benchmark_prefetch_output_tree_latency() {
    let files = std::env::var("MBX_BENCH_FILES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(96);
    let latency = Duration::from_millis(
        std::env::var("MBX_BENCH_LATENCY_MS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(100),
    );

    let directory = tempfile::tempdir().unwrap();
    let (responses, output_root) = output_tree_responses(files);
    let (base_url, maximum_in_flight, server) = delayed_blob_server(responses, latency).await;
    let agent = remote_agent_url(
        base_url,
        directory.path().join("reader"),
        RemoteCacheMode::ReadOnly,
    );
    let remote = agent.remote.as_deref().unwrap();

    let started = std::time::Instant::now();
    agent
        .prefetch_output_tree(remote, &output_root)
        .await
        .unwrap();
    let elapsed = started.elapsed();

    eprintln!(
        "prefetched {files} blobs with {} ms latency in {elapsed:?}",
        latency.as_millis()
    );
    server.await.unwrap();
    eprintln!(
        "maximum concurrent requests: {}",
        maximum_in_flight.load(Ordering::Relaxed)
    );
}

fn output_tree_responses(files: usize) -> (BTreeMap<String, Vec<u8>>, CacheDigest) {
    let mut entries = Vec::with_capacity(files);
    let mut responses = BTreeMap::new();
    for index in 0..files {
        let body = format!("cached artifact {index}").into_bytes();
        let digest = CacheDigest::blake3(&body);
        entries.push(serde_json::json!({
            "digest": digest,
            "executable": false,
            "mode": 420,
            "name": format!("artifact-{index}"),
        }));
        responses.insert(blob_path(&digest), body);
    }
    let directory = canonical_json(&serde_json::json!({
        "directories": [],
        "files": entries,
        "symlinks": [],
        "version": 1,
    }))
    .unwrap();
    let output_root = CacheDigest::blake3(&directory);
    responses.insert(blob_path(&output_root), directory);
    (responses, output_root)
}

fn blob_pack_body(entries: &[(CacheDigest, &[u8])]) -> Vec<u8> {
    let mut pack = crate::BLOB_PACK_MAGIC.to_vec();
    for (digest, bytes) in entries {
        pack.push(match digest.algorithm.as_str() {
            "blake3" => 1,
            "sha256" => 2,
            algorithm => panic!("unexpected test digest algorithm {algorithm}"),
        });
        pack.extend(hex::decode(&digest.hash).unwrap());
        pack.extend(digest.size.to_be_bytes());
        pack.extend_from_slice(bytes);
    }
    pack
}

async fn delayed_blob_server(
    responses: BTreeMap<String, Vec<u8>>,
    latency: Duration,
) -> (
    url::Url,
    Arc<std::sync::atomic::AtomicUsize>,
    tokio::task::JoinHandle<()>,
) {
    use std::sync::atomic::AtomicUsize;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let responses = Arc::new(responses);
    let request_count = responses.len();
    let in_flight = Arc::new(AtomicUsize::new(0));
    let maximum_in_flight = Arc::new(AtomicUsize::new(0));
    let observed_maximum = maximum_in_flight.clone();
    let server = tokio::spawn(async move {
        let mut requests = tokio::task::JoinSet::new();
        for _ in 0..request_count {
            let (mut socket, _) = listener.accept().await.unwrap();
            let responses = responses.clone();
            let in_flight = in_flight.clone();
            let maximum_in_flight = maximum_in_flight.clone();
            requests.spawn(async move {
                let mut request = Vec::new();
                loop {
                    let mut chunk = [0; 1024];
                    let size = socket.read(&mut chunk).await.unwrap();
                    assert!(size > 0, "client closed before sending request headers");
                    request.extend_from_slice(&chunk[..size]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&request);
                let path = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap();
                let body = responses.get(path).unwrap();
                let active = in_flight.fetch_add(1, Ordering::Relaxed) + 1;
                maximum_in_flight.fetch_max(active, Ordering::Relaxed);
                tokio::time::sleep(latency).await;
                socket
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                            body.len()
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
                socket.write_all(body).await.unwrap();
                in_flight.fetch_sub(1, Ordering::Relaxed);
            });
        }
        while requests.join_next().await.is_some() {}
    });
    (
        format!("http://{address}").parse().unwrap(),
        observed_maximum,
        server,
    )
}

fn remote_agent(
    server: &mockito::ServerGuard,
    cache_dir: PathBuf,
    mode: RemoteCacheMode,
) -> CacheAgent {
    remote_agent_url(server.url().parse().unwrap(), cache_dir, mode)
}

fn remote_agent_url(base_url: url::Url, cache_dir: PathBuf, mode: RemoteCacheMode) -> CacheAgent {
    remote_agent_url_with_limit(base_url, cache_dir, mode, DEFAULT_MAX_REMOTE_DOWNLOAD_BYTES)
}

fn remote_agent_url_with_limit(
    base_url: url::Url,
    cache_dir: PathBuf,
    mode: RemoteCacheMode,
    max_remote_download_bytes: u64,
) -> CacheAgent {
    let client = RemoteCacheClient::new(crate::RemoteCacheConfig {
        base_url,
        namespace: "test".into(),
        token: None,
        token_file: None,
        oidc_audience: None,
        connect_timeout: Duration::from_secs(1),
        read_timeout: Duration::from_secs(1),
        download_timeout: Duration::from_secs(1),
        retries: 0,
    })
    .unwrap();
    CacheAgent::new_remote_with_download_limit(
        &cache_dir,
        "test-version",
        AgentRemoteCache {
            client,
            mode,
            staging_dir: cache_dir.join("remote"),
        },
        max_remote_download_bytes,
    )
}

fn blob_path(digest: &CacheDigest) -> String {
    format!(
        "/v1/blobs/{}/{}/{}",
        digest.algorithm, digest.hash, digest.size
    )
}

fn action_path(digest: &CacheDigest) -> String {
    format!(
        "/v1/action-results/{}/{}/{}",
        digest.algorithm, digest.hash, digest.size
    )
}

fn action_manifest_path(digest: &CacheDigest) -> String {
    format!(
        "/v1/action-manifests/{}/{}/{}",
        digest.algorithm, digest.hash, digest.size
    )
}

#[tokio::test]
async fn bounds_cumulative_remote_downloads_for_a_session() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = mockito::Server::new_async().await;
    let first = CacheDigest::blake3(b"first");
    let second = CacheDigest::blake3(b"second");
    let first_download = server
        .mock("GET", blob_path(&first).as_str())
        .with_status(200)
        .with_body("first")
        .expect(1)
        .create_async()
        .await;
    let second_download = server
        .mock("GET", blob_path(&second).as_str())
        .with_status(200)
        .with_body("second")
        .expect(0)
        .create_async()
        .await;
    let agent = remote_agent_url_with_limit(
        server.url().parse().unwrap(),
        directory.path().join("reader"),
        RemoteCacheMode::ReadOnly,
        first.size,
    );

    assert!(matches!(
        agent
            .respond(AgentRequest::FindBlob {
                digest: first.clone()
            })
            .await,
        AgentResponse::Blob { path: Some(_) }
    ));
    assert!(matches!(
        agent
            .respond(AgentRequest::FindBlob { digest: second })
            .await,
        AgentResponse::Blob { path: None }
    ));
    assert_eq!(
        agent.remote_download_bytes.load(Ordering::Relaxed),
        first.size
    );
    first_download.assert_async().await;
    second_download.assert_async().await;
}

#[tokio::test]
async fn merges_overlapping_runs_into_one_task_manifest() {
    let directory = tempfile::tempdir().unwrap();
    let cache = directory.path().join("cache");
    let task = "d".repeat(64);
    let agent = CacheAgent::new(&cache, "test-version");
    let first_run = agent.begin_task(&task).await.unwrap();
    let second_run = agent.begin_task(&task).await.unwrap();
    assert_ne!(first_run, second_run);
    let first_invocation = CacheDigest::blake3(b"overlap one");
    let second_invocation = CacheDigest::blake3(b"overlap two");
    for (run, invocation) in [
        (&first_run, &first_invocation),
        (&second_run, &second_invocation),
    ] {
        assert!(matches!(
            agent
                .respond(AgentRequest::RecordActionPrediction {
                    task: run.clone(),
                    prediction: ActionPrediction {
                        invocation: invocation.clone(),
                        action: CacheDigest::blake3(invocation.hash.as_bytes()),
                        adapter: "rustc".into(),
                        payload: "{}".into(),
                    },
                })
                .await,
            AgentResponse::ActionPredictionRecorded
        ));
    }
    agent.commit_task(&first_run).await.unwrap();
    agent.commit_task(&second_run).await.unwrap();

    let next = CacheAgent::new(cache, "test-version");
    let run = next.begin_task(&task).await.unwrap();
    for invocation in [first_invocation, second_invocation] {
        assert!(matches!(
            next.respond(AgentRequest::FindActionPrediction {
                task: run.clone(),
                invocation,
            })
            .await,
            AgentResponse::ActionPrediction {
                prediction: Some(_)
            }
        ));
    }
}

#[test]
fn keeps_local_manifest_when_remote_merge_exceeds_prediction_limit() {
    let task = "7".repeat(64);
    let prediction = |index: usize| {
        let digest = CacheDigest::blake3(&index.to_le_bytes());
        ActionPrediction {
            invocation: digest.clone(),
            action: digest,
            adapter: "rustc".into(),
            payload: "{}".into(),
        }
    };
    let local = TaskActionManifest {
        version: TASK_ACTION_MANIFEST_VERSION,
        task: task.clone(),
        predictions: (0..MAX_TASK_ACTION_PREDICTIONS).map(prediction).collect(),
    };
    let expected_first = local.predictions[0].clone();
    let remote = TaskActionManifest {
        version: TASK_ACTION_MANIFEST_VERSION,
        task: task.clone(),
        predictions: vec![prediction(MAX_TASK_ACTION_PREDICTIONS)],
    };

    let (manifest, merged) = merge_remote_task_manifest(&task, remote, local);
    assert!(!merged);
    assert_eq!(manifest.predictions.len(), MAX_TASK_ACTION_PREDICTIONS);
    assert_eq!(manifest.predictions[0], expected_first);
}

#[test]
fn task_manifest_lock_is_shared_across_agents() {
    let directory = tempfile::tempdir().unwrap();
    let cache = directory.path().join("cache");
    let first = CacheAgent::new(&cache, "test-version");
    let second = CacheAgent::new(&cache, "test-version");
    let task = "8".repeat(64);

    let first_lock = first.lock_task_manifest(&task).unwrap();
    let mut contender = fslock::LockFile::open(&second.task_manifest_lock_path(&task)).unwrap();
    assert!(!contender.try_lock().unwrap());
    drop(first_lock);
    assert!(contender.try_lock().unwrap());
}

#[tokio::test]
async fn memoizes_client_observed_executable_identities() {
    let directory = tempfile::tempdir().unwrap();
    let agent = CacheAgent::new(directory.path(), "test-version");
    let executable = directory.path().join("rustc");
    let environment = BTreeMap::from([("RUSTUP_TOOLCHAIN".into(), Some("stable".into()))]);

    let response = agent
        .respond(AgentRequest::FindExecutableIdentity {
            executable: executable.clone(),
            environment: environment.clone(),
        })
        .await;
    assert!(matches!(
        response,
        AgentResponse::ExecutableIdentity { stdout: None }
    ));

    let response = agent
        .respond(AgentRequest::StoreExecutableIdentity {
            executable: executable.clone(),
            environment: environment.clone(),
            stdout: b"rustc identity".to_vec(),
        })
        .await;
    assert!(matches!(
        response,
        AgentResponse::ExecutableIdentity {
            stdout: Some(stdout)
        } if stdout == b"rustc identity"
    ));

    let response = agent
        .respond(AgentRequest::FindExecutableIdentity {
            executable,
            environment,
        })
        .await;
    assert!(matches!(
        response,
        AgentResponse::ExecutableIdentity {
            stdout: Some(stdout)
        } if stdout == b"rustc identity"
    ));
}

#[test]
fn bounds_executable_identity_entry_count() {
    let directory = tempfile::tempdir().unwrap();
    let agent = CacheAgent::new(directory.path(), "test-version");
    for index in 0..MAX_EXECUTABLE_IDENTITIES {
        agent
            .store_executable_identity(
                directory.path().join(format!("rustc-{index}")),
                BTreeMap::new(),
                vec![b'x'],
            )
            .unwrap();
    }

    assert!(
        agent
            .store_executable_identity(
                directory.path().join("one-too-many"),
                BTreeMap::new(),
                vec![b'x'],
            )
            .is_err()
    );
}

#[test]
fn bounds_executable_identity_retained_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let agent = CacheAgent::new(directory.path(), "test-version");
    for index in 0..MAX_EXECUTABLE_IDENTITY_BYTES / MAX_EXECUTABLE_IDENTITY_SIZE {
        agent
            .store_executable_identity(
                directory.path().join(format!("rustc-{index}")),
                BTreeMap::new(),
                vec![b'x'; MAX_EXECUTABLE_IDENTITY_SIZE],
            )
            .unwrap();
    }

    assert!(
        agent
            .store_executable_identity(
                directory.path().join("one-byte-too-many"),
                BTreeMap::new(),
                vec![b'x'],
            )
            .is_err()
    );
}

#[tokio::test]
async fn version_skew_is_a_handshake_miss() {
    let directory = tempfile::tempdir().unwrap();
    let agent = CacheAgent::new(directory.path(), "agent-version");
    let (mut client, server) = tokio::io::duplex(1024);
    let task = tokio::spawn(async move { agent.handle_connection(server).await });
    let request = AgentRequest::Hello {
        protocol: AGENT_PROTOCOL_VERSION,
        client_version: "other-version".into(),
    };
    let mut encoded = serde_json::to_vec(&request).unwrap();
    encoded.push(b'\n');
    client.write_all(&encoded).await.unwrap();
    let mut response = String::new();
    BufReader::new(&mut client)
        .read_line(&mut response)
        .await
        .unwrap();

    assert!(matches!(
        serde_json::from_str(&response).unwrap(),
        AgentResponse::Error { .. }
    ));
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn reports_the_action_digests_a_task_manifest_recorded() {
    let directory = tempfile::tempdir().unwrap();
    let cache = directory.path().join("cache");
    let task = "b".repeat(64);
    let action = CacheDigest::blake3(b"recorded action");

    let agent = CacheAgent::new(&cache, "test-version");
    let run = agent.begin_task(&task).await.unwrap();
    assert!(matches!(
        agent
            .respond(AgentRequest::RecordActionPrediction {
                task: run.clone(),
                prediction: ActionPrediction {
                    invocation: CacheDigest::blake3(b"an invocation"),
                    action: action.clone(),
                    adapter: "rustc".into(),
                    payload: "{}".into(),
                },
            })
            .await,
        AgentResponse::ActionPredictionRecorded
    ));
    agent.commit_task(&run).await.unwrap();

    assert_eq!(task_manifest_actions(&cache, &task).unwrap(), vec![action]);
}

#[test]
fn reports_no_actions_for_a_task_without_a_usable_manifest() {
    let directory = tempfile::tempdir().unwrap();
    let cache = directory.path();
    let task = "c".repeat(64);
    assert!(task_manifest_actions(cache, &task).unwrap().is_empty());

    // A manifest the current code can no longer parse is worth exactly as
    // much as a missing one, and neither is worth failing a sweep over.
    let path = task_manifest_dir(cache).join(format!("{task}.json"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"not json").unwrap();
    assert!(task_manifest_actions(cache, &task).unwrap().is_empty());

    // A manifest naming a different task is somebody else's; claiming its
    // actions would root objects this task never used.
    fs::write(
        &path,
        serde_json::to_vec(&TaskActionManifest {
            version: TASK_ACTION_MANIFEST_VERSION,
            task: "d".repeat(64),
            predictions: Vec::new(),
        })
        .unwrap(),
    )
    .unwrap();
    assert!(task_manifest_actions(cache, &task).unwrap().is_empty());

    assert!(task_manifest_actions(cache, "not-an-identity").is_err());
}

#[test]
fn recognizes_only_well_formed_task_identities() {
    assert!(is_task_identity(&"a".repeat(64)));
    assert!(is_task_identity(&"0123456789abcdef".repeat(4)));
    assert!(!is_task_identity(&"A".repeat(64)));
    assert!(!is_task_identity(&"g".repeat(64)));
    assert!(!is_task_identity(&"a".repeat(63)));
    assert!(!is_task_identity(""));
}
