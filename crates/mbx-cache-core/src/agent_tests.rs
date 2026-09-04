use super::*;
use crate::{
    ACTION_PROMISE_MEDIA_TYPE, ACTION_RESULT_BATCH_MEDIA_TYPE, ACTION_RESULT_MEDIA_TYPE,
    BLOB_PACK_BLOBS_HEADER, MAX_ACTION_PREDICTION_PAYLOAD,
};
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
async fn surfaces_each_shim_warning_once() {
    struct Collected(Mutex<Vec<String>>);
    impl AgentEventObserver for Collected {
        fn event(&self, event: AgentEvent) {
            if let AgentEvent::Warning { message } = event {
                self.0.lock().unwrap().push(message);
            }
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let observer = Arc::new(Collected(Mutex::new(Vec::new())));
    let agent = CacheAgent::new(directory.path().join("cache"), "test-version")
        .with_observer(observer.clone());

    for _ in 0..3 {
        let response = agent
            .respond(AgentRequest::RecordWarning {
                message: "cc result was not restored: blob missing".into(),
            })
            .await;
        assert!(matches!(response, AgentResponse::WarningRecorded));
    }
    // A different message is its own diagnostic, not a repeat.
    agent
        .respond(AgentRequest::RecordWarning {
            message: "verification was not recorded".into(),
        })
        .await;

    assert_eq!(
        *observer.0.lock().unwrap(),
        vec![
            "cc result was not restored: blob missing".to_string(),
            "verification was not recorded".to_string(),
        ]
    );
}

#[tokio::test]
async fn rejects_malformed_shim_warnings() {
    let directory = tempfile::tempdir().unwrap();
    let agent = CacheAgent::new(directory.path().join("cache"), "test-version");
    for message in [
        String::new(),
        "two\nlines".into(),
        "wide".repeat(MAX_WARNING_BYTES),
    ] {
        let response = agent.respond(AgentRequest::RecordWarning { message }).await;
        assert!(matches!(response, AgentResponse::Error { .. }));
    }
}

#[tokio::test]
async fn stops_surfacing_shim_warnings_at_the_cap() {
    struct Counter(AtomicU64);
    impl AgentEventObserver for Counter {
        fn event(&self, event: AgentEvent) {
            if matches!(event, AgentEvent::Warning { .. }) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let observer = Arc::new(Counter(AtomicU64::new(0)));
    let agent = CacheAgent::new(directory.path().join("cache"), "test-version")
        .with_observer(observer.clone());

    for index in 0..MAX_WARNINGS + 10 {
        agent
            .respond(AgentRequest::RecordWarning {
                message: format!("unique failure {index}"),
            })
            .await;
    }

    assert_eq!(
        observer.0.load(Ordering::Relaxed),
        u64::try_from(MAX_WARNINGS).unwrap()
    );
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

/// Outcomes are a closed set because they name the categories a build summary
/// adds up; an unrecognized one would appear as its own line and count as
/// nothing.
#[tokio::test]
async fn compiler_invocations_are_counted_by_known_outcomes() {
    let directory = tempfile::tempdir().unwrap();
    let agent = CacheAgent::new(directory.path().join("cache"), "test-version");

    for outcome in ["miss", "incremental", "incremental"] {
        agent
            .respond(AgentRequest::RecordCompilerInvocation {
                outcome: outcome.into(),
                crate_name: Some("demo".into()),
                duration_ns: 10,
            })
            .await;
    }
    let rejected = agent
        .respond(AgentRequest::RecordCompilerInvocation {
            outcome: "invented".into(),
            crate_name: None,
            duration_ns: 0,
        })
        .await;

    let stats = agent.stats();
    assert_eq!(
        stats.compiler.get("incremental").map(|it| it.invocations),
        Some(2)
    );
    assert_eq!(stats.compiler.get("miss").map(|it| it.invocations), Some(1));
    assert!(matches!(rejected, AgentResponse::Error { .. }));
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

/// A remembered blob is revalidated by file identity rather than by rehashing
/// it, so an overwrite that preserves the length is still caught: writing the
/// file moves its modification time. The modification time is set explicitly
/// here so the test does not depend on the filesystem's timestamp resolution.
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

    let corrupted = b"broken object";
    assert_eq!(corrupted.len() as u64, digest.size);
    let mut file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    std::io::Write::write_all(&mut file, corrupted).unwrap();
    file.set_modified(std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1))
        .unwrap();
    drop(file);

    assert!(agent.find_verified_blob(&digest).is_err());
    assert!(!agent.verified_blobs.lock().unwrap().contains_key(&digest));
}

#[test]
fn remembered_blobs_reject_truncation() {
    let directory = tempfile::tempdir().unwrap();
    let agent = CacheAgent::new(directory.path().join("cache"), "test-version");
    let digest = CacheDigest::blake3(b"cached object");
    let path = agent.cas.store_bytes(&digest, b"cached object").unwrap();
    assert_eq!(
        agent.find_verified_blob(&digest).unwrap(),
        Some(path.clone())
    );

    std::fs::write(&path, b"torn").unwrap();

    assert!(agent.find_verified_blob(&digest).is_err());
    assert!(!agent.verified_blobs.lock().unwrap().contains_key(&digest));
}

#[test]
fn remembered_blobs_notice_eviction() {
    let directory = tempfile::tempdir().unwrap();
    let agent = CacheAgent::new(directory.path().join("cache"), "test-version");
    let digest = CacheDigest::blake3(b"cached object");
    let path = agent.cas.store_bytes(&digest, b"cached object").unwrap();
    assert_eq!(
        agent.find_verified_blob(&digest).unwrap(),
        Some(path.clone())
    );

    std::fs::remove_file(&path).unwrap();

    assert_eq!(agent.find_verified_blob(&digest).unwrap(), None);
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

#[test]
fn diagnostic_and_action_events_are_emitted_as_one_pair() {
    let directory = tempfile::tempdir().unwrap();
    let observer = Arc::new(RecordingObserver::default());
    let agent = CacheAgent::new(directory.path().join("cache"), "test-version")
        .with_observer(observer.clone());
    let barrier = Arc::new(std::sync::Barrier::new(9));
    let threads = (0..8)
        .map(|index| {
            let agent = agent.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let crate_name = format!("unit-{index}");
                barrier.wait();
                agent.emit_action(
                    Some(AgentEvent::ActionDiagnostic {
                        outcome: "miss".into(),
                        crate_name: Some(crate_name.clone()),
                        diagnostic: ActionDiagnostic {
                            action: CacheDigest::blake3(crate_name.as_bytes()),
                            components: BTreeMap::new(),
                            inputs: BTreeMap::new(),
                        },
                    }),
                    AgentEvent::CompilerInvocation {
                        outcome: "miss".into(),
                        crate_name: Some(crate_name),
                        duration_ns: 1,
                    },
                );
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    for thread in threads {
        thread.join().unwrap();
    }

    let events = observer.events.lock().unwrap();
    assert_eq!(events.len(), 16);
    for pair in events.chunks_exact(2) {
        assert!(matches!(
            pair,
            [
                AgentEvent::ActionDiagnostic {
                    crate_name: Some(diagnostic_crate),
                    ..
                },
                AgentEvent::CompilerInvocation {
                    crate_name: Some(action_crate),
                    ..
                }
            ] if diagnostic_crate == action_crate
        ));
    }
}

#[tokio::test]
async fn reports_each_accounted_decision_to_an_observer() {
    let directory = tempfile::tempdir().unwrap();
    let observer = Arc::new(RecordingObserver::default());
    let agent = CacheAgent::new(directory.path().join("cache"), "test-version")
        .with_observer(observer.clone());
    let diagnostic = ActionDiagnostic {
        action: CacheDigest::blake3(b"action"),
        components: BTreeMap::new(),
        inputs: BTreeMap::new(),
    };

    agent
        .respond(AgentRequest::RecordBypass {
            kind: "incremental".into(),
        })
        .await;
    agent.respond(AgentRequest::RecordUnconsulted).await;
    agent
        .handle_requests([
            AgentRequest::RecordWarning {
                message: format!(
                    "{ACTION_DIAGNOSTIC_PREFIX}{}",
                    serde_json::json!({
                        "outcome": "miss",
                        "crate_name": "serde",
                        "diagnostic": diagnostic,
                    })
                ),
            },
            AgentRequest::RecordCompilerInvocation {
                outcome: "miss".into(),
                crate_name: Some("serde".into()),
                duration_ns: 42,
            },
        ])
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
            AgentEvent::ActionDiagnostic {
                outcome: diagnostic_outcome,
                crate_name: Some(diagnostic_crate),
                diagnostic: observed,
            },
            AgentEvent::CompilerInvocation {
                outcome,
                crate_name: Some(crate_name),
                duration_ns: 42,
            },
            AgentEvent::Verification { matched: false, .. },
        ] if kind == "incremental"
            && diagnostic_outcome == "miss"
            && diagnostic_crate == "serde"
            && observed == &diagnostic
            && outcome == "miss"
            && crate_name == "serde"
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
    let diagnostic = ActionDiagnostic {
        action: CacheDigest::blake3(b"absent"),
        components: BTreeMap::new(),
        inputs: BTreeMap::new(),
    };
    let responses = agent
        .handle_requests([
            AgentRequest::RecordWarning {
                message: format!(
                    "{ACTION_DIAGNOSTIC_PREFIX}{}",
                    serde_json::json!({
                        "outcome": "hit",
                        "crate_name": "serde",
                        "diagnostic": diagnostic,
                    })
                ),
            },
            AgentRequest::RecordActionHit {
                action: CacheDigest::blake3(b"absent"),
                restore: RestoreStats::default(),
                crate_name: Some("serde".into()),
            },
        ])
        .await;

    assert!(matches!(responses[1], AgentResponse::Error { .. }));
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
                    reused_output_files: 1,
                    reused_output_bytes: 2,
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
            reused_output_files: 1,
            reused_output_bytes: 2,
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
async fn begin_task_reports_how_many_predictions_were_loaded() {
    let directory = tempfile::tempdir().unwrap();
    let cache = directory.path().join("cache");
    let task = "b".repeat(64);

    let seed = CacheAgent::new(&cache, "test-version");
    let run = seed.begin_task(&task).await.unwrap();
    assert_eq!(seed.stats().predictions_loaded, 0);
    for name in ["first", "second"] {
        assert!(matches!(
            seed.respond(AgentRequest::RecordActionPrediction {
                task: run.clone(),
                prediction: ActionPrediction {
                    invocation: CacheDigest::blake3(name.as_bytes()),
                    action: CacheDigest::blake3(name.as_bytes()),
                    adapter: "rustc".into(),
                    payload: "{}".into(),
                },
            })
            .await,
            AgentResponse::ActionPredictionRecorded
        ));
    }
    seed.commit_task(&run).await.unwrap();

    // What was loaded, not what was recorded: this is the baseline a session
    // had available to match against.
    assert_eq!(seed.stats().predictions_loaded, 0);
    let reader = CacheAgent::new(&cache, "test-version");
    reader.begin_task(&task).await.unwrap();
    assert_eq!(reader.stats().predictions_loaded, 2);
}

#[test]
fn a_matching_prediction_activates_only_its_adapter_once() {
    let rustc_first = ActionPrediction {
        invocation: CacheDigest::blake3(b"first rustc invocation"),
        action: CacheDigest::blake3(b"first rustc action"),
        adapter: "rustc".into(),
        payload: "{}".into(),
    };
    let rustc_second = ActionPrediction {
        invocation: CacheDigest::blake3(b"second rustc invocation"),
        action: CacheDigest::blake3(b"second rustc action"),
        adapter: "rustc".into(),
        payload: "{}".into(),
    };
    let cc = ActionPrediction {
        invocation: CacheDigest::blake3(b"cc invocation"),
        action: CacheDigest::blake3(b"cc action"),
        adapter: "cc".into(),
        payload: "{}".into(),
    };
    let mut state = TaskActionState::default();
    for prediction in [&rustc_first, &rustc_second, &cc] {
        state
            .predictions
            .insert(prediction.invocation.clone(), prediction.clone());
    }

    let (prediction, prefetch) =
        activate_prediction_adapter(&mut state, &CacheDigest::blake3(b"stale invocation"));
    assert_eq!(prediction, None);
    assert_eq!(prefetch, None);
    assert!(state.prefetched_adapters.is_empty());

    let (prediction, prefetch) = activate_prediction_adapter(&mut state, &rustc_first.invocation);
    assert_eq!(prediction, Some(rustc_first.clone()));
    let prefetch = prefetch.unwrap();
    assert_eq!(prefetch.len(), 2);
    assert!(
        prefetch
            .iter()
            .all(|prediction| prediction.adapter == "rustc")
    );

    let (prediction, prefetch) = activate_prediction_adapter(&mut state, &rustc_second.invocation);
    assert_eq!(prediction, Some(rustc_second));
    assert_eq!(prefetch, None, "one adapter starts only one prefetch wave");

    let (prediction, prefetch) = activate_prediction_adapter(&mut state, &cc.invocation);
    assert_eq!(prediction, Some(cc.clone()));
    assert_eq!(prefetch, Some(vec![cc]));
}

fn seeded_predictions(names: &[&str]) -> Vec<ActionPrediction> {
    names
        .iter()
        .map(|name| ActionPrediction {
            invocation: CacheDigest::blake3(name.as_bytes()),
            action: CacheDigest::blake3(name.as_bytes()),
            adapter: "rustc".into(),
            payload: "{}".into(),
        })
        .collect()
}

#[tokio::test]
async fn a_task_without_a_manifest_inherits_the_first_fallback_that_has_one() {
    let directory = tempfile::tempdir().unwrap();
    let cache = directory.path().join("cache");
    let previous = "a".repeat(64);
    let current = "b".repeat(64);
    let unbuilt = "c".repeat(64);
    let seed = CacheAgent::new(&cache, "test-version");
    seed.persist_task_manifest(&TaskActionManifest {
        version: TASK_ACTION_MANIFEST_VERSION,
        task: previous.clone(),
        predictions: seeded_predictions(&["first", "second"]),
    })
    .unwrap();

    let agent = CacheAgent::new(&cache, "test-version");
    let consulted = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let fallbacks = {
        let consulted = consulted.clone();
        let identities = vec![unbuilt.clone(), previous.clone()];
        move || {
            consulted.store(true, Ordering::Relaxed);
            identities.clone()
        }
    };
    agent.register_task_fallbacks(&current, fallbacks).unwrap();
    let run = agent.begin_task(&current).await.unwrap();
    assert!(consulted.load(Ordering::Relaxed));
    assert_eq!(agent.stats().predictions_loaded, 2);
    assert!(matches!(
        agent
            .respond(AgentRequest::FindActionPrediction {
                task: run.clone(),
                invocation: CacheDigest::blake3(b"first"),
            })
            .await,
        AgentResponse::ActionPrediction {
            prediction: Some(_)
        }
    ));

    // Adopted under the current identity as a whole, and the source is left
    // as it was.
    let adopted = agent.load_task_manifest(&current).unwrap().unwrap();
    assert_eq!(adopted.task, current);
    assert_eq!(adopted.predictions.len(), 2);
    assert!(agent.load_task_manifest(&unbuilt).unwrap().is_none());
    assert_eq!(
        agent.load_task_manifest(&previous).unwrap().unwrap().task,
        previous
    );

    // A command that uses only part of it commits the whole: the next
    // command under this identity, a test run after a build, starts from the
    // units the first one never compiled rather than from a partial record.
    assert!(matches!(
        agent
            .respond(AgentRequest::RecordActionPrediction {
                task: run.clone(),
                prediction: seeded_predictions(&["first"]).remove(0),
            })
            .await,
        AgentResponse::ActionPredictionRecorded
    ));
    agent.commit_task(&run).await.unwrap();
    let later = CacheAgent::new(&cache, "test-version");
    later.begin_task(&current).await.unwrap();
    assert_eq!(later.stats().predictions_loaded, 2);
}

#[tokio::test]
async fn the_newest_manifest_in_the_store_is_the_last_resort() {
    let directory = tempfile::tempdir().unwrap();
    let cache = directory.path().join("cache");
    let older = "5".repeat(64);
    let newer = "6".repeat(64);
    let current = "7".repeat(64);
    let huge = "9".repeat(64);
    let seed = CacheAgent::new(&cache, "test-version");
    // The newest manifest belongs to some other, enormous workspace: adopting
    // it would leave this one's recordings no room under the limit.
    let oversized: Vec<String> = (0..=MAX_TASK_ACTION_PREDICTIONS / 2)
        .map(|index| format!("huge {index}"))
        .collect();
    let oversized: Vec<&str> = oversized.iter().map(String::as_str).collect();
    for (task, names, age) in [
        (&older, vec!["old"], 120),
        (&newer, vec!["new"], 60),
        (&huge, oversized, 30),
    ] {
        seed.persist_task_manifest(&TaskActionManifest {
            version: TASK_ACTION_MANIFEST_VERSION,
            task: task.clone(),
            predictions: seeded_predictions(&names),
        })
        .unwrap();
        let file = std::fs::File::options()
            .write(true)
            .open(seed.task_manifest_path(task))
            .unwrap();
        file.set_modified(std::time::SystemTime::now() - Duration::from_secs(age))
            .unwrap();
    }

    // Without registered fallbacks the store is not searched.
    let plain = CacheAgent::new(&cache, "test-version");
    plain.begin_task(&current).await.unwrap();
    assert_eq!(plain.stats().predictions_loaded, 0);

    let agent = CacheAgent::new(&cache, "test-version");
    agent
        .register_task_fallbacks(&current, || vec!["8".repeat(64)])
        .unwrap();
    let run = agent.begin_task(&current).await.unwrap();
    assert_eq!(agent.stats().predictions_loaded, 1);
    assert!(matches!(
        agent
            .respond(AgentRequest::FindActionPrediction {
                task: run,
                invocation: CacheDigest::blake3(b"new"),
            })
            .await,
        AgentResponse::ActionPrediction {
            prediction: Some(_)
        }
    ));
}

#[tokio::test]
async fn fallbacks_are_not_consulted_for_a_task_that_has_a_manifest() {
    let directory = tempfile::tempdir().unwrap();
    let cache = directory.path().join("cache");
    let task = "d".repeat(64);
    let agent = CacheAgent::new(&cache, "test-version");
    agent
        .persist_task_manifest(&TaskActionManifest {
            version: TASK_ACTION_MANIFEST_VERSION,
            task: task.clone(),
            predictions: seeded_predictions(&["own"]),
        })
        .unwrap();
    let consulted = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let fallbacks = {
        let consulted = consulted.clone();
        move || {
            consulted.store(true, Ordering::Relaxed);
            vec!["e".repeat(64)]
        }
    };
    agent.register_task_fallbacks(&task, fallbacks).unwrap();
    agent.begin_task(&task).await.unwrap();
    assert!(!consulted.load(Ordering::Relaxed));
    assert_eq!(agent.stats().predictions_loaded, 1);
}

#[tokio::test]
async fn a_fallback_manifest_is_fetched_from_the_remote_when_absent_locally() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = mockito::Server::new_async().await;
    let previous = "1".repeat(64);
    let current = "2".repeat(64);
    let remote_manifest = TaskActionManifest {
        version: TASK_ACTION_MANIFEST_VERSION,
        task: previous.clone(),
        predictions: seeded_predictions(&["remote"]),
    };
    let remote_bytes = canonical_json(&remote_manifest).unwrap();
    let remote_etag = blake3::hash(&remote_bytes).to_hex().to_string();
    let (_, current_selector) = CacheAgent::task_manifest_selector(&current).unwrap();
    let (_, previous_selector) = CacheAgent::task_manifest_selector(&previous).unwrap();
    let missing = server
        .mock("GET", action_manifest_path(&current_selector).as_str())
        .with_status(404)
        .expect(1)
        .create_async()
        .await;
    let found = server
        .mock("GET", action_manifest_path(&previous_selector).as_str())
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
    let fallbacks = {
        let previous = previous.clone();
        move || vec![previous.clone()]
    };
    agent.register_task_fallbacks(&current, fallbacks).unwrap();
    let run = agent.begin_task(&current).await.unwrap();
    assert_eq!(agent.stats().predictions_loaded, 1);
    let adopted = agent.load_task_manifest(&current).unwrap().unwrap();
    assert_eq!(adopted.task, current);
    assert_eq!(adopted.predictions, remote_manifest.predictions);
    assert!(matches!(
        agent
            .respond(AgentRequest::FindActionPrediction {
                task: run,
                invocation: CacheDigest::blake3(b"remote"),
            })
            .await,
        AgentResponse::ActionPrediction {
            prediction: Some(_)
        }
    ));
    missing.assert_async().await;
    found.assert_async().await;
}

#[tokio::test]
async fn commit_receipt_contains_this_runs_predictions_not_its_baseline() {
    let directory = tempfile::tempdir().unwrap();
    let cache = directory.path().join("cache");
    let task = "9".repeat(64);
    let first = ActionPrediction {
        invocation: CacheDigest::blake3(b"first invocation"),
        action: CacheDigest::blake3(b"first action"),
        adapter: "rustc".into(),
        payload: "{}".into(),
    };
    let seed = CacheAgent::new(&cache, "test-version");
    let seed_run = seed.begin_task(&task).await.unwrap();
    assert!(matches!(
        seed.respond(AgentRequest::RecordActionPrediction {
            task: seed_run.clone(),
            prediction: first,
        })
        .await,
        AgentResponse::ActionPredictionRecorded
    ));
    seed.commit_task(&seed_run).await.unwrap();

    let second = ActionPrediction {
        invocation: CacheDigest::blake3(b"second invocation"),
        action: CacheDigest::blake3(b"second action"),
        adapter: "rustc".into(),
        payload: "{}".into(),
    };
    let agent = CacheAgent::new(&cache, "test-version");
    let run = agent.begin_task(&task).await.unwrap();
    assert!(matches!(
        agent
            .respond(AgentRequest::RecordActionPrediction {
                task: run.clone(),
                prediction: second.clone(),
            })
            .await,
        AgentResponse::ActionPredictionRecorded
    ));

    let completed = agent.commit_task_actions(&run).await.unwrap();

    assert_eq!(completed, vec![second]);
    assert_eq!(task_manifest_actions(&cache, &task).unwrap().len(), 2);
}

#[tokio::test]
async fn a_refused_prediction_reports_the_constraint_the_shim_should_print() {
    // The shim prints whatever this error says. "invalid action prediction"
    // on its own left an oversized payload from one crate looking identical
    // to a malformed one, in a build log with nothing else to go on.
    let directory = tempfile::tempdir().unwrap();
    let agent = CacheAgent::new(directory.path().join("cache"), "test-version");
    let task = agent.begin_task(&"c".repeat(64)).await.unwrap();
    let digest = CacheDigest::blake3(b"invocation");
    let payload = format!("\"{}\"", "p".repeat(MAX_ACTION_PREDICTION_PAYLOAD));
    let payload_len = payload.len();

    let response = agent
        .respond(AgentRequest::RecordActionPrediction {
            task,
            prediction: ActionPrediction {
                invocation: digest.clone(),
                action: digest,
                adapter: "rustc".into(),
                payload,
            },
        })
        .await;
    let AgentResponse::Error { message } = response else {
        panic!("an oversized payload must be refused, got {response:?}");
    };
    assert_eq!(
        message,
        format!(
            "invalid action prediction: payload is {payload_len} bytes, \
             over the {MAX_ACTION_PREDICTION_PAYLOAD} byte limit"
        )
    );
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
async fn local_predictions_start_prefetching_while_the_remote_manifest_loads() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = mockito::Server::new_async().await;
    mock_agent_capabilities(&mut server, serde_json::json!({})).await;
    let task = "5".repeat(64);
    let action_bytes = b"locally predicted action";
    let action = CacheDigest::blake3(action_bytes);
    let prediction = ActionPrediction {
        invocation: CacheDigest::blake3(b"local invocation"),
        action: action.clone(),
        adapter: "rustc".into(),
        payload: "{}".into(),
    };
    let manifest_value = TaskActionManifest {
        version: TASK_ACTION_MANIFEST_VERSION,
        task: task.clone(),
        predictions: vec![prediction],
    };
    let manifest_bytes = canonical_json(&manifest_value).unwrap();
    let manifest_etag = blake3::hash(&manifest_bytes).to_hex().to_string();
    let (_, selector) = CacheAgent::task_manifest_selector(&task).unwrap();
    let release = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let response_release = release.clone();
    let remote_manifest = server
        .mock("GET", action_manifest_path(&selector).as_str())
        .with_status(200)
        .with_header("etag", &format!("\"{manifest_etag}\""))
        .with_chunked_body(move |writer| {
            while !response_release.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(10));
            }
            std::io::Write::write_all(writer, &manifest_bytes)
        })
        .expect(1)
        .create_async()
        .await;
    let started = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let response_started = started.clone();
    let result = RemoteActionResult {
        action: action.clone(),
        metadata: None,
        output_root: None,
        version: 1,
    };
    let action_result = server
        .mock("GET", action_path(&action).as_str())
        .with_status(200)
        .with_header("content-type", ACTION_RESULT_MEDIA_TYPE)
        .with_chunked_body(move |writer| {
            response_started.store(true, Ordering::Release);
            std::io::Write::write_all(writer, &serde_json::to_vec(&result).unwrap())
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
    agent.persist_task_manifest(&manifest_value).unwrap();

    let begin_agent = agent.clone();
    let begin_task = tokio::spawn(async move { begin_agent.begin_task(&task).await });
    let prefetched_before_manifest = tokio::time::timeout(Duration::from_secs(2), async {
        while !started.load(Ordering::Acquire) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    release.store(true, Ordering::Release);
    prefetched_before_manifest.expect("local prefetch waited for the remote manifest");
    begin_task.await.unwrap().unwrap();
    agent.wait_for_prefetches().await;

    remote_manifest.assert_async().await;
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
async fn downloads_independent_blob_packs_concurrently() {
    let directory = tempfile::tempdir().unwrap();
    let entries = BTreeMap::from([
        (CacheDigest::blake3(b"first pack"), b"first pack".to_vec()),
        (CacheDigest::blake3(b"second pack"), b"second pack".to_vec()),
    ]);
    let (base_url, maximum_in_flight, server) =
        delayed_pack_server(entries.clone(), Duration::from_millis(50)).await;
    let agent = remote_agent_url(
        base_url,
        directory.path().join("reader"),
        RemoteCacheMode::ReadOnly,
    );
    let remote = agent.remote.as_deref().unwrap();

    let verified = agent
        .fetch_remote_blobs(remote, entries.keys().cloned().collect(), None)
        .await;
    server.await.unwrap();

    assert_eq!(verified.len(), entries.len());
    for (digest, bytes) in entries {
        assert_eq!(fs::read(&verified[&digest]).unwrap(), bytes);
    }
    assert!(maximum_in_flight.load(Ordering::Relaxed) > 1);
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
async fn prefetch_resolves_predicted_actions_in_one_lookup() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = mockito::Server::new_async().await;
    mock_agent_capabilities(&mut server, serde_json::json!({ "action_batch": true })).await;
    let task = "7".repeat(64);
    let mut predictions = Vec::new();
    let mut results = Vec::new();
    let mut action_blobs = Vec::new();
    let mut skipped = Vec::new();
    for index in 0..3 {
        let action_bytes = format!("predicted action {index}").into_bytes();
        let action = CacheDigest::blake3(&action_bytes);
        predictions.push(ActionPrediction {
            invocation: CacheDigest::blake3(format!("invocation {index}").as_bytes()),
            action: action.clone(),
            adapter: "rustc".into(),
            payload: "{}".into(),
        });
        results.push(RemoteActionResult {
            action: action.clone(),
            metadata: None,
            output_root: None,
            version: 1,
        });
        action_blobs.push(
            server
                .mock("GET", blob_path(&action).as_str())
                .with_status(200)
                .with_body(action_bytes)
                .expect(1)
                .create_async()
                .await,
        );
        // One batched lookup replaces a request per predicted action.
        skipped.push(
            server
                .mock("GET", action_path(&action).as_str())
                .expect(0)
                .create_async()
                .await,
        );
    }
    let manifest_bytes = canonical_json(&TaskActionManifest {
        version: TASK_ACTION_MANIFEST_VERSION,
        task: task.clone(),
        predictions: predictions.clone(),
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
    let batch = server
        .mock("POST", "/v1/action-results:batch")
        .with_status(200)
        .with_header("content-type", ACTION_RESULT_BATCH_MEDIA_TYPE)
        .with_body(serde_json::json!({ "results": results }).to_string())
        .expect(1)
        .create_async()
        .await;
    let agent = remote_agent(
        &server,
        directory.path().join("reader"),
        RemoteCacheMode::ReadOnly,
    );

    agent.begin_task(&task).await.unwrap();
    agent.wait_for_prefetches().await;

    manifest.assert_async().await;
    batch.assert_async().await;
    for mock in action_blobs.into_iter().chain(skipped) {
        mock.assert_async().await;
    }
    for result in &results {
        assert!(agent.actions.find(&result.action).unwrap().is_some());
    }
    assert_eq!(agent.stats().remote_action_lookups, 1);
}

#[tokio::test]
async fn prefetch_skips_remote_negotiation_when_every_action_is_local() {
    let directory = tempfile::tempdir().unwrap();
    let server = mockito::Server::new_async().await;
    let action = CacheDigest::blake3(b"already local action");
    let result = RemoteActionResult {
        action: action.clone(),
        metadata: None,
        output_root: None,
        version: 1,
    };
    let agent = remote_agent(
        &server,
        directory.path().join("reader"),
        RemoteCacheMode::ReadOnly,
    );
    let action_source = directory.path().join("already-local-action");
    fs::write(&action_source, b"already local action").unwrap();
    agent.cas.store_file(&action, &action_source).unwrap();
    agent.actions.store(&result).unwrap();
    let prediction = ActionPrediction {
        invocation: CacheDigest::blake3(b"already local invocation"),
        action,
        adapter: "rustc".into(),
        payload: "{}".into(),
    };
    let actions = select_prefetch_actions(std::iter::once(&prediction));

    assert!(agent.prefetch_action_batches(&actions).await.unwrap());
    assert_eq!(agent.stats().remote_action_lookups, 0);
}

#[tokio::test]
async fn deferred_prefetch_waits_for_a_matching_adapter() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = mockito::Server::new_async().await;
    let task = "5".repeat(64);
    let rustc = ActionPrediction {
        invocation: CacheDigest::blake3(b"matching rustc invocation"),
        action: CacheDigest::blake3(b"matching rustc action"),
        adapter: "rustc".into(),
        payload: "{}".into(),
    };
    let cc = ActionPrediction {
        invocation: CacheDigest::blake3(b"unmatched cc invocation"),
        action: CacheDigest::blake3(b"unmatched cc action"),
        adapter: "cc".into(),
        payload: "{}".into(),
    };
    let manifest_bytes = canonical_json(&TaskActionManifest {
        version: TASK_ACTION_MANIFEST_VERSION,
        task: task.clone(),
        predictions: vec![rustc.clone(), cc],
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
    let capabilities = server
        .mock("GET", "/v1/capabilities")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            serde_json::json!({
                "protocol":{"major":1},
                "features":{"action_batch":true},
                "limits":{"max_batch_items":100,"max_pack_bytes":1048576}
            })
            .to_string(),
        )
        .expect(1)
        .create_async()
        .await;
    let batch = server
        .mock("POST", "/v1/action-results:batch")
        .match_body(mockito::Matcher::Json(serde_json::json!({
            "digests": [rustc.action.clone()]
        })))
        .with_status(200)
        .with_header("content-type", ACTION_RESULT_BATCH_MEDIA_TYPE)
        .with_body(serde_json::json!({ "results": [] }).to_string())
        .expect(1)
        .create_async()
        .await;
    let agent = remote_agent(
        &server,
        directory.path().join("reader"),
        RemoteCacheMode::ReadOnly,
    );

    let run = agent.begin_task_on_prediction(&task).await.unwrap();
    agent.wait_for_prefetches().await;
    assert_eq!(agent.stats().prefetch_runs, 0);
    assert!(matches!(
        agent
            .respond(AgentRequest::FindActionPrediction {
                task: run.clone(),
                invocation: CacheDigest::blake3(b"stale invocation"),
            })
            .await,
        AgentResponse::ActionPrediction { prediction: None }
    ));
    agent.wait_for_prefetches().await;
    assert_eq!(agent.stats().prefetch_runs, 0);

    assert!(matches!(
        agent
            .respond(AgentRequest::FindActionPrediction {
                task: run,
                invocation: rustc.invocation.clone(),
            })
            .await,
        AgentResponse::ActionPrediction {
            prediction: Some(found)
        } if found == rustc
    ));
    agent.wait_for_prefetches().await;

    manifest.assert_async().await;
    capabilities.assert_async().await;
    batch.assert_async().await;
    assert_eq!(agent.stats().prefetch_runs, 1);
    assert_eq!(agent.stats().remote_action_lookups, 1);
}

#[tokio::test]
async fn prefetch_falls_back_when_batched_lookups_are_unavailable() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = mockito::Server::new_async().await;
    // Advertised, then absent: the client must still resolve the prediction.
    mock_agent_capabilities(&mut server, serde_json::json!({ "action_batch": true })).await;
    let task = "6".repeat(64);
    let action_bytes = b"fallback action".to_vec();
    let action = CacheDigest::blake3(&action_bytes);
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
            invocation: CacheDigest::blake3(b"fallback invocation"),
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
    let batch = server
        .mock("POST", "/v1/action-results:batch")
        .with_status(404)
        .expect(1)
        .create_async()
        .await;
    let single = server
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
    let agent = remote_agent(
        &server,
        directory.path().join("reader"),
        RemoteCacheMode::ReadOnly,
    );

    agent.begin_task(&task).await.unwrap();
    agent.wait_for_prefetches().await;

    manifest.assert_async().await;
    batch.assert_async().await;
    single.assert_async().await;
    action_blob.assert_async().await;
    assert!(agent.actions.find(&action).unwrap().is_some());
}

async fn mock_agent_capabilities(server: &mut mockito::ServerGuard, features: serde_json::Value) {
    server
        .mock("GET", "/v1/capabilities")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            serde_json::json!({
                "protocol":{"major":1},
                "features":features,
                "limits":{"max_batch_items":100,"max_pack_bytes":1048576}
            })
            .to_string(),
        )
        .create_async()
        .await;
}

#[tokio::test]
async fn queued_blobs_are_published_in_one_pack() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = mockito::Server::new_async().await;
    mock_agent_capabilities(
        &mut server,
        serde_json::json!({ "blob_pack_uploads": true }),
    )
    .await;
    let contents = [
        b"first output".to_vec(),
        b"second output".to_vec(),
        b"third output".to_vec(),
    ];
    let digests: Vec<CacheDigest> = contents
        .iter()
        .map(|bytes| CacheDigest::blake3(bytes))
        .collect();
    let pack = server
        .mock("POST", "/v1/blobs:pack-upload")
        .match_header(BLOB_PACK_BLOBS_HEADER, "3")
        .with_status(200)
        .with_body(serde_json::json!({"created":3,"existing":0}).to_string())
        .expect(1)
        .create_async()
        .await;
    // One request replaces three, so none of the individual endpoints are used.
    let mut singles = Vec::new();
    for digest in &digests {
        singles.push(
            server
                .mock("PUT", blob_path(digest).as_str())
                .expect(0)
                .create_async()
                .await,
        );
    }
    let agent = remote_agent(
        &server,
        directory.path().join("writer"),
        RemoteCacheMode::WriteOnly,
    );
    let mut requests = Vec::new();
    for (index, (digest, bytes)) in digests.iter().zip(&contents).enumerate() {
        let source = directory.path().join(format!("source-{index}"));
        fs::write(&source, bytes).unwrap();
        requests.push(AgentRequest::StoreBlob {
            digest: digest.clone(),
            source,
        });
    }
    agent.handle_requests(requests).await;
    agent.wait_for_uploads().await;

    pack.assert_async().await;
    for single in singles {
        single.assert_async().await;
    }
    let stats = agent.stats();
    assert_eq!(stats.remote_blob_pack_uploads, 1);
    assert_eq!(stats.remote_blob_pack_upload_blobs, 3);
    assert_eq!(stats.background_uploads, 3);
}

#[tokio::test]
async fn a_rejected_pack_falls_back_to_individual_uploads() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = mockito::Server::new_async().await;
    mock_agent_capabilities(
        &mut server,
        serde_json::json!({ "blob_pack_uploads": true }),
    )
    .await;
    // A pack's worth of members, not a pair: the fallback republishes the whole
    // group, and doing that one round trip at a time is worst exactly when the
    // server has just refused a request.
    let contents: Vec<Vec<u8>> = (0..8)
        .map(|index| format!("output {index}").into_bytes())
        .collect();
    let digests: Vec<CacheDigest> = contents
        .iter()
        .map(|bytes| CacheDigest::blake3(bytes))
        .collect();
    let pack = server
        .mock("POST", "/v1/blobs:pack-upload")
        .with_status(500)
        .expect(1)
        .create_async()
        .await;
    let mut singles = Vec::new();
    for digest in &digests {
        singles.push(
            server
                .mock("PUT", blob_path(digest).as_str())
                .with_status(200)
                .expect(1)
                .create_async()
                .await,
        );
    }
    let agent = remote_agent(
        &server,
        directory.path().join("writer"),
        RemoteCacheMode::WriteOnly,
    );
    let mut requests = Vec::new();
    for (index, (digest, bytes)) in digests.iter().zip(&contents).enumerate() {
        let source = directory.path().join(format!("source-{index}"));
        fs::write(&source, bytes).unwrap();
        requests.push(AgentRequest::StoreBlob {
            digest: digest.clone(),
            source,
        });
    }
    agent.handle_requests(requests).await;
    agent.wait_for_uploads().await;

    pack.assert_async().await;
    for single in singles {
        single.assert_async().await;
    }
    // Everything still published, so nothing downstream is withheld.
    assert_eq!(agent.stats().background_uploads, digests.len() as u64);
    assert_eq!(agent.stats().remote_blob_pack_uploads, 0);
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

async fn delayed_pack_server(
    entries: BTreeMap<CacheDigest, Vec<u8>>,
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
    let entries = Arc::new(entries);
    let in_flight = Arc::new(AtomicUsize::new(0));
    let maximum_in_flight = Arc::new(AtomicUsize::new(0));
    let observed_maximum = maximum_in_flight.clone();
    let server = tokio::spawn(async move {
        let mut requests = tokio::task::JoinSet::new();
        for _ in 0..3 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let entries = entries.clone();
            let in_flight = in_flight.clone();
            let maximum_in_flight = maximum_in_flight.clone();
            requests.spawn(async move {
                let mut request = Vec::new();
                let (header_end, content_length) = loop {
                    let mut chunk = [0; 1024];
                    let size = socket.read(&mut chunk).await.unwrap();
                    assert!(size > 0, "client closed before sending request headers");
                    request.extend_from_slice(&chunk[..size]);
                    let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
                        continue;
                    };
                    let header_end = header_end + 4;
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    break (header_end, content_length);
                };
                while request.len() < header_end + content_length {
                    let mut chunk = [0; 1024];
                    let size = socket.read(&mut chunk).await.unwrap();
                    assert!(size > 0, "client closed before sending request body");
                    request.extend_from_slice(&chunk[..size]);
                }
                let first_line = String::from_utf8_lossy(&request[..header_end])
                    .lines()
                    .next()
                    .unwrap()
                    .to_owned();
                let path = first_line.split_whitespace().nth(1).unwrap();
                if path == "/v1/capabilities" {
                    let body = serde_json::json!({
                        "protocol":{"major":1},
                        "features":{"blob_packs":true},
                        "limits":{"max_batch_items":1,"max_pack_bytes":1048576}
                    })
                    .to_string();
                    socket
                        .write_all(
                            format!(
                                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                                body.len()
                            )
                            .as_bytes(),
                        )
                        .await
                        .unwrap();
                    return;
                }
                assert_eq!(path, "/v1/blobs:pack");
                let body: serde_json::Value =
                    serde_json::from_slice(&request[header_end..header_end + content_length])
                        .unwrap();
                let digest: CacheDigest =
                    serde_json::from_value(body["digests"][0].clone()).unwrap();
                let bytes = entries.get(&digest).unwrap();
                let body = blob_pack_body(&[(digest, bytes.as_slice())]);
                let active = in_flight.fetch_add(1, Ordering::Relaxed) + 1;
                maximum_in_flight.fetch_max(active, Ordering::Relaxed);
                tokio::time::sleep(latency).await;
                socket
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\ncontent-type: {}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                            crate::BLOB_PACK_MEDIA_TYPE,
                            body.len()
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
                socket.write_all(&body).await.unwrap();
                in_flight.fetch_sub(1, Ordering::Relaxed);
            });
        }
        while let Some(result) = requests.join_next().await {
            result.unwrap();
        }
    });
    (
        url::Url::parse(&format!("http://{address}/")).unwrap(),
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

#[tokio::test]
async fn a_read_write_agent_claims_an_advertised_action_promise() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = mockito::Server::new_async().await;
    let invocation = CacheDigest::blake3(b"fleet invocation");
    let capabilities = server
        .mock("GET", "/v1/capabilities")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            serde_json::json!({
                "protocol":{"major":1},
                "features":{"action_promises":true}
            })
            .to_string(),
        )
        .create_async()
        .await;
    let promise_path = format!(
        "/v1/action-promises/blake3/{}/{}",
        invocation.hash, invocation.size
    );
    let claim = server
        .mock("POST", promise_path.as_str())
        .with_status(200)
        .with_header("content-type", ACTION_PROMISE_MEDIA_TYPE)
        .with_body(r#"{"state":"claimed","claim":"fleet-lease"}"#)
        .create_async()
        .await;
    let agent = remote_agent(
        &server,
        directory.path().to_path_buf(),
        RemoteCacheMode::ReadWrite,
    );

    assert!(matches!(
        agent
            .respond(AgentRequest::JoinActionPromise {
                adapter: "rustc".into(),
                invocation,
            })
            .await,
        AgentResponse::ActionPromise {
            claim: Some(claim),
            prediction: None,
        } if claim == "fleet-lease"
    ));
    capabilities.assert_async().await;
    claim.assert_async().await;
}

#[tokio::test]
async fn a_read_only_agent_never_claims_fleet_work() {
    let directory = tempfile::tempdir().unwrap();
    let server = mockito::Server::new_async().await;
    let agent = remote_agent(
        &server,
        directory.path().to_path_buf(),
        RemoteCacheMode::ReadOnly,
    );

    assert!(matches!(
        agent
            .respond(AgentRequest::JoinActionPromise {
                adapter: "rustc".into(),
                invocation: CacheDigest::blake3(b"read-only invocation"),
            })
            .await,
        AgentResponse::ActionPromise {
            claim: None,
            prediction: None,
        }
    ));
}

#[tokio::test]
async fn completing_a_fleet_promise_waits_for_its_action_upload() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = mockito::Server::new_async().await;
    let invocation = CacheDigest::blake3(b"promised invocation");
    let action = CacheDigest::blake3(b"promised action");
    let output_root = CacheDigest::blake3(b"promised output root");
    let result = RemoteActionResult {
        version: 1,
        action: action.clone(),
        output_root: Some(output_root.clone()),
        metadata: None,
    };
    let prediction = ActionPrediction {
        invocation: invocation.clone(),
        action: action.clone(),
        adapter: "rustc".into(),
        payload: "{}".into(),
    };
    let capabilities = server
        .mock("GET", "/v1/capabilities")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            serde_json::json!({
                "protocol":{"major":1},
                "features":{"action_promises":true}
            })
            .to_string(),
        )
        .create_async()
        .await;
    let upload = server
        .mock("PUT", action_path(&action).as_str())
        .with_status(201)
        .expect(1)
        .create_async()
        .await;
    let action_blob_upload = server
        .mock("PUT", blob_path(&action).as_str())
        .with_status(201)
        .expect(1)
        .create_async()
        .await;
    let output_blob_upload = server
        .mock("PUT", blob_path(&output_root).as_str())
        .with_status(201)
        .expect(1)
        .create_async()
        .await;
    let promise_path = format!(
        "/v1/action-promises/blake3/{}/{}",
        invocation.hash, invocation.size
    );
    let complete = server
        .mock("PUT", promise_path.as_str())
        .match_body(mockito::Matcher::Json(
            serde_json::to_value(ActionPromiseCompletion {
                claim: "fleet-lease".into(),
                prediction: prediction.clone(),
            })
            .unwrap(),
        ))
        .with_status(204)
        .expect(1)
        .create_async()
        .await;
    let agent = remote_agent(
        &server,
        directory.path().to_path_buf(),
        RemoteCacheMode::ReadWrite,
    );

    let action_source = directory.path().join("action-source");
    let output_source = directory.path().join("output-source");
    std::fs::write(&action_source, b"promised action").unwrap();
    std::fs::write(&output_source, b"promised output root").unwrap();
    assert!(matches!(
        agent
            .respond(AgentRequest::StoreBlob {
                digest: action.clone(),
                source: action_source,
            })
            .await,
        AgentResponse::Stored { .. }
    ));
    assert!(matches!(
        agent
            .respond(AgentRequest::StoreBlob {
                digest: output_root,
                source: output_source,
            })
            .await,
        AgentResponse::Stored { .. }
    ));

    assert!(matches!(
        agent
            .respond(AgentRequest::StoreActionResult { result })
            .await,
        AgentResponse::ActionStored { .. }
    ));
    assert!(matches!(
        agent
            .respond(AgentRequest::CompleteActionPromise {
                claim: "fleet-lease".into(),
                prediction,
            })
            .await,
        AgentResponse::ActionPromiseCompleted
    ));
    capabilities.assert_async().await;
    action_blob_upload.assert_async().await;
    output_blob_upload.assert_async().await;
    upload.assert_async().await;
    complete.assert_async().await;
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

/// A linker driver's identity depends on the SDK and toolchain environment it
/// builds against, so those names key an identity too. Everything else stays
/// refused: a name that does not select what the probe reports would let one
/// key stand for two answers.
#[tokio::test]
async fn identity_keys_admit_only_names_that_select_the_answer() {
    let directory = tempfile::tempdir().unwrap();
    let agent = CacheAgent::new(directory.path(), "test-version");
    let executable = directory.path().join("cc");

    for name in [
        "SDKROOT",
        "MACOSX_DEPLOYMENT_TARGET",
        "LIB",
        "UCRTVersion",
        "UniversalCRTSdkDir",
        "VCToolsInstallDir",
        "VCToolsVersion",
        "WindowsSdkDir",
        "WindowsSDKVersion",
    ] {
        let response = agent
            .respond(AgentRequest::StoreExecutableIdentity {
                executable: executable.clone(),
                environment: BTreeMap::from([(name.into(), Some("value".into()))]),
                stdout: b"cc identity".to_vec(),
            })
            .await;
        assert!(
            matches!(response, AgentResponse::ExecutableIdentity { .. }),
            "{name} should key an identity"
        );
    }

    let response = agent
        .respond(AgentRequest::StoreExecutableIdentity {
            executable,
            environment: BTreeMap::from([("PATH".into(), Some("/usr/bin".into()))]),
            stdout: b"cc identity".to_vec(),
        })
        .await;
    assert!(matches!(response, AgentResponse::Error { .. }));
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

fn ledger_identity(path: &str, len: u64, nanos: u32) -> FileIdentity {
    // Rooted per platform so `is_absolute` holds on both, and timed in
    // multiples of 100ns so Windows file-time resolution keeps two distinct
    // nanos values distinct.
    let root = if cfg!(windows) { "C:\\" } else { "/" };
    FileIdentity {
        path: PathBuf::from(format!("{root}{path}")),
        len,
        modified: SystemTime::UNIX_EPOCH + Duration::new(1_700_000_000, nanos * 100),
        changed: Some((1_700_000_000, nanos.into())),
        object: None,
    }
}

fn ledger_digest(len: u64) -> CacheDigest {
    CacheDigest {
        algorithm: "blake3".into(),
        hash: "b".repeat(64),
        size: len,
    }
}

#[tokio::test]
async fn file_digest_ledger_answers_only_matching_identities() {
    let directory = tempfile::tempdir().unwrap();
    let agent = CacheAgent::new(directory.path(), "test-version");
    let file = ledger_identity("work/target/libserde.rlib", 7, 21);

    let response = agent
        .respond(AgentRequest::RecordFileDigests {
            scope: FileDigestScope::Content,
            entries: vec![RecordedFileDigest {
                file: file.clone(),
                digest: ledger_digest(7),
            }],
        })
        .await;
    assert!(matches!(response, AgentResponse::FileDigestsRecorded));

    // The exact identity answers; a touched or truncated file does not.
    let response = agent
        .respond(AgentRequest::FindFileDigests {
            scope: FileDigestScope::Content,
            files: vec![
                file.clone(),
                ledger_identity("work/target/libserde.rlib", 7, 22),
                ledger_identity("work/target/libserde.rlib", 8, 21),
                ledger_identity("work/target/absent.rlib", 7, 21),
            ],
        })
        .await;
    let AgentResponse::FileDigests { digests } = response else {
        panic!("expected file digests");
    };
    assert_eq!(
        digests,
        vec![Some(ledger_digest(7)), None, None, None],
        "only the recorded identity may answer"
    );
}

#[tokio::test]
async fn file_digest_ledger_scopes_do_not_answer_for_each_other() {
    let directory = tempfile::tempdir().unwrap();
    let agent = CacheAgent::new(directory.path(), "test-version");
    let file = ledger_identity("work/vendor/header.h", 7, 21);

    agent
        .respond(AgentRequest::RecordFileDigests {
            scope: FileDigestScope::Content,
            entries: vec![RecordedFileDigest {
                file: file.clone(),
                digest: ledger_digest(7),
            }],
        })
        .await;

    // A content digest never vouches for the cc input scan.
    let response = agent
        .respond(AgentRequest::FindFileDigests {
            scope: FileDigestScope::CcInput,
            files: vec![file.clone()],
        })
        .await;
    assert!(
        matches!(response, AgentResponse::FileDigests { digests } if digests == vec![None]),
        "scopes must not answer for each other"
    );

    let response = agent
        .respond(AgentRequest::FindFileDigests {
            scope: FileDigestScope::Content,
            files: vec![file],
        })
        .await;
    assert!(
        matches!(response, AgentResponse::FileDigests { digests } if digests == vec![Some(ledger_digest(7))])
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_file_digest_misses_share_one_large_read() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("large-input.rlib");
    let bytes = vec![b'x'; 256 * 1024];
    std::fs::write(&path, &bytes).unwrap();
    let metadata = std::fs::metadata(&path).unwrap();
    let identity = FileIdentity::for_digest_cache(&path, &metadata)
        .unwrap()
        .unwrap();
    let expected = CacheDigest::blake3(&bytes);
    let agent = CacheAgent::new(directory.path().join("cache"), "test-version");
    let barrier = Arc::new(tokio::sync::Barrier::new(65));
    let mut tasks = Vec::new();
    for _ in 0..64 {
        let agent = agent.clone();
        let identity = identity.clone();
        let barrier = Arc::clone(&barrier);
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            agent
                .resolve_file_digest(FileDigestScope::Content, identity)
                .await
        }));
    }
    barrier.wait().await;
    for task in tasks {
        assert_eq!(
            task.await.unwrap(),
            FileDigestResolution::Digest(expected.clone())
        );
    }
    assert_eq!(agent.file_digest_reads.load(Ordering::Relaxed), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_timestamp_macro_resolutions_share_one_read() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("timestamp-input.c");
    std::fs::write(&path, b"const char *built = __DATE__;\n").unwrap();
    let metadata = std::fs::metadata(&path).unwrap();
    let identity = FileIdentity::for_digest_cache(&path, &metadata)
        .unwrap()
        .unwrap();
    let key = (FileDigestScope::CcInput, identity.clone());
    let flight = Arc::new(FileDigestFlight {
        lock: tokio::sync::Mutex::new(()),
        resolution: Mutex::new(None),
    });
    let owner = flight.lock.lock().await;
    let agent = CacheAgent::new(directory.path().join("cache"), "test-version");
    agent
        .file_digest_locks
        .lock()
        .unwrap()
        .insert(key, Arc::downgrade(&flight));

    let mut tasks = Vec::new();
    for _ in 0..64 {
        let agent = agent.clone();
        let identity = identity.clone();
        tasks.push(tokio::spawn(async move {
            agent
                .resolve_file_digest(FileDigestScope::CcInput, identity)
                .await
        }));
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        while Arc::strong_count(&flight) != 65 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("waiters did not join the same digest flight");
    drop(owner);

    for task in tasks {
        assert_eq!(
            task.await.unwrap(),
            FileDigestResolution::EmbeddedTimestampMacro
        );
    }
    assert_eq!(agent.file_digest_reads.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn abandoned_file_digest_owner_wakes_the_next_waiter() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("input.rlib");
    std::fs::write(&path, b"dependency bytes").unwrap();
    let metadata = std::fs::metadata(&path).unwrap();
    let identity = FileIdentity::for_digest_cache(&path, &metadata)
        .unwrap()
        .unwrap();
    let key = (FileDigestScope::Content, identity.clone());
    let lock = Arc::new(FileDigestFlight {
        lock: tokio::sync::Mutex::new(()),
        resolution: Mutex::new(None),
    });

    let acquired = Arc::new(tokio::sync::Notify::new());
    let owner_lock = Arc::clone(&lock);
    let owner_acquired = Arc::clone(&acquired);
    let owner = tokio::spawn(async move {
        let _guard = owner_lock.lock.lock().await;
        owner_acquired.notify_one();
        std::future::pending::<()>().await;
    });
    acquired.notified().await;

    let agent = CacheAgent::new(directory.path().join("waiter-cache"), "test-version");
    agent
        .file_digest_locks
        .lock()
        .unwrap()
        .insert(key, Arc::downgrade(&lock));
    let waiter_agent = agent.clone();
    let waiter = tokio::spawn(async move {
        waiter_agent
            .resolve_file_digest(FileDigestScope::Content, identity)
            .await
    });
    tokio::task::yield_now().await;
    owner.abort();
    let resolution = tokio::time::timeout(Duration::from_secs(5), waiter)
        .await
        .expect("waiter stayed blocked after its owner was aborted")
        .unwrap();
    assert!(matches!(resolution, FileDigestResolution::Digest(_)));
}

#[tokio::test]
async fn file_digest_records_are_validated() {
    let directory = tempfile::tempdir().unwrap();
    let agent = CacheAgent::new(directory.path(), "test-version");

    let response = agent
        .respond(AgentRequest::RecordFileDigests {
            scope: FileDigestScope::Content,
            entries: vec![RecordedFileDigest {
                file: FileIdentity {
                    path: PathBuf::from("relative/libserde.rlib"),
                    ..ledger_identity("work/libserde.rlib", 7, 21)
                },
                digest: ledger_digest(7),
            }],
        })
        .await;
    assert!(matches!(response, AgentResponse::Error { .. }));

    let response = agent
        .respond(AgentRequest::RecordFileDigests {
            scope: FileDigestScope::Content,
            entries: vec![RecordedFileDigest {
                file: ledger_identity("work/libserde.rlib", 8, 21),
                digest: ledger_digest(7),
            }],
        })
        .await;
    assert!(
        matches!(response, AgentResponse::Error { .. }),
        "a length that disagrees with the digest must be refused"
    );
}

/// A seeded ledger answers like one this session filled, drops what it cannot
/// vouch for, and never overrides what this session recorded itself.
#[test]
fn seeded_file_digests_answer_lookups_and_yield_to_this_sessions_records() {
    let directory = tempfile::tempdir().unwrap();
    let agent = CacheAgent::new(directory.path().join("cache"), "test-version");
    let file = directory.path().join("libdep.rlib");
    std::fs::write(&file, b"rlib bytes").unwrap();
    let identity = FileIdentity::describe(&file, &std::fs::metadata(&file).unwrap()).unwrap();
    let digest = CacheDigest::blake3(b"rlib bytes");
    // A digest hashing could never produce, sized like the file so the ledger
    // accepts it: whichever entry answers is then unambiguous.
    let own = RecordedFileDigest {
        file: identity.clone(),
        digest: CacheDigest {
            algorithm: "blake3".into(),
            hash: "a".repeat(64),
            size: identity.len,
        },
    };
    let wrong_length = RecordedFileDigest {
        file: FileIdentity {
            len: identity.len + 1,
            ..identity.clone()
        },
        digest: digest.clone(),
    };
    let relative = RecordedFileDigest {
        file: FileIdentity {
            path: "relative/libdep.rlib".into(),
            ..identity.clone()
        },
        digest: digest.clone(),
    };

    // Recorded by a shim first: the seed must not replace it.
    agent
        .record_file_digests(FileDigestScope::Content, vec![own.clone()])
        .unwrap();
    let seeded = agent.seed_file_digests(vec![
        (
            FileDigestScope::Content,
            RecordedFileDigest {
                file: identity.clone(),
                digest: digest.clone(),
            },
        ),
        (
            FileDigestScope::CcInput,
            RecordedFileDigest {
                file: identity.clone(),
                digest: digest.clone(),
            },
        ),
        (FileDigestScope::Content, wrong_length),
        (FileDigestScope::Content, relative),
    ]);
    assert_eq!(seeded, 1, "only the cc entry was new and valid");

    let AgentResponse::FileDigests { digests } = agent
        .find_file_digests(FileDigestScope::Content, vec![identity.clone()])
        .unwrap()
    else {
        panic!("expected digests");
    };
    assert_eq!(digests, vec![Some(own.digest.clone())]);
    let AgentResponse::FileDigests { digests } = agent
        .find_file_digests(FileDigestScope::CcInput, vec![identity.clone()])
        .unwrap()
    else {
        panic!("expected digests");
    };
    assert_eq!(digests, vec![Some(digest.clone())]);

    let mut exported = agent.file_digests();
    exported.sort_by_key(|(scope, _)| *scope);
    assert_eq!(
        exported,
        vec![
            (FileDigestScope::Content, own),
            (
                FileDigestScope::CcInput,
                RecordedFileDigest {
                    file: identity,
                    digest
                }
            ),
        ]
    );
}
