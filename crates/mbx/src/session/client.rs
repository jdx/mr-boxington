use super::VERSION;
use crate::config::Config;
use eyre::{Context, Result, bail};
use mbx_cache_core::{AGENT_PROTOCOL_VERSION, AgentRequest, AgentResponse, CacheAgent};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::OsString;
#[cfg(unix)]
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

/// Serve a persistent Cargo wrapper directly from the local store.
///
/// There is deliberately no remote access here: the short-lived process has
/// no trusted session policy or opportunity to batch transfers. Prediction
/// manifests are still persisted after every successful record so the many
/// wrapper processes in one Cargo build, and later builds, share what they
/// learn without a daemon.
pub(super) fn request_standalone_agent(requests: &[AgentRequest]) -> Result<Vec<AgentResponse>> {
    thread_local! {
        static STANDALONE_AGENT: RefCell<Option<StandaloneAgent>> = const { RefCell::new(None) };
    }

    STANDALONE_AGENT.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            let config = Config::load()?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            *slot = Some(StandaloneAgent {
                agent: CacheAgent::new(config.store_dir(), VERSION),
                runtime,
                runs: BTreeMap::new(),
            });
        }
        let standalone = slot.as_mut().expect("standalone agent was initialized");
        let StandaloneAgent {
            agent,
            runtime,
            runs,
        } = standalone;
        runtime.block_on(async {
            let mut responses = Vec::with_capacity(requests.len());
            for request in requests.iter().cloned() {
                let (request, run_to_commit) = match request {
                    AgentRequest::FindActionPrediction { task, invocation } => {
                        let run = match runs.get(&task) {
                            Some(run) => run.clone(),
                            None => {
                                let run = agent.begin_task(&task).await?;
                                runs.insert(task.clone(), run.clone());
                                run
                            }
                        };
                        (
                            AgentRequest::FindActionPrediction {
                                task: run,
                                invocation,
                            },
                            None,
                        )
                    }
                    AgentRequest::RecordActionPrediction { task, prediction } => {
                        let run = match runs.remove(&task) {
                            Some(run) => run,
                            None => agent.begin_task(&task).await?,
                        };
                        (
                            AgentRequest::RecordActionPrediction {
                                task: run.clone(),
                                prediction,
                            },
                            Some(run),
                        )
                    }
                    request => (request, None),
                };
                let response = agent
                    .handle_requests(std::iter::once(request))
                    .await
                    .into_iter()
                    .next()
                    .expect("one request returns one response");
                let succeeded = !matches!(response, AgentResponse::Error { .. });
                responses.push(response);
                if succeeded && let Some(run) = run_to_commit {
                    agent.commit_task(&run).await?;
                }
            }
            Ok(responses)
        })
    })
}

struct StandaloneAgent {
    agent: CacheAgent,
    runtime: tokio::runtime::Runtime,
    runs: BTreeMap<String, String>,
}

#[cfg(unix)]
pub(super) fn request_agent_at(
    socket: &OsString,
    requests: &[AgentRequest],
) -> Result<Vec<AgentResponse>> {
    // One compilation asks the agent several separate questions, and paying a
    // connect plus a handshake round trip for each adds up across a warm
    // build. The protocol is strict request-response, so a connection held for
    // the life of this shim process serves them all.
    thread_local! {
        static CONNECTION: RefCell<Option<(OsString, std::os::unix::net::UnixStream)>> =
            const { RefCell::new(None) };
    }
    CONNECTION.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.as_ref().is_none_or(|(known, _)| known != socket) {
            let mut stream = std::os::unix::net::UnixStream::connect(Path::new(socket))
                .wrap_err("failed to connect to the cache session")?;
            sync_handshake(&mut stream)?;
            *slot = Some((socket.clone(), stream));
        }
        let (_, stream) = slot.as_mut().expect("the connection was just established");
        let responses = requests
            .iter()
            .map(|request| sync_request(stream, request))
            .collect::<Result<Vec<_>>>();
        if responses.is_err() {
            // A failed exchange leaves the stream in an unknown protocol
            // state, so the next request starts over on a fresh connection.
            *slot = None;
        }
        responses
    })
}

#[cfg(windows)]
pub(super) fn request_agent_at(
    socket: &OsString,
    requests: &[AgentRequest],
) -> Result<Vec<AgentResponse>> {
    let endpoint = socket.to_string_lossy().into_owned();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async move {
            use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
            let mut stream = connect_pipe(&endpoint).await?;
            let request = AgentRequest::Hello {
                protocol: AGENT_PROTOCOL_VERSION,
                client_version: VERSION.into(),
            };
            let mut encoded = serde_json::to_vec(&request)?;
            encoded.push(b'\n');
            stream.write_all(&encoded).await?;
            stream.flush().await?;
            let mut response = String::new();
            tokio::io::BufReader::new(&mut stream)
                .read_line(&mut response)
                .await?;
            validate_handshake_response(&response)?;
            let mut responses = Vec::with_capacity(requests.len());
            for request in requests {
                let mut encoded = serde_json::to_vec(request)?;
                encoded.push(b'\n');
                stream.write_all(&encoded).await?;
                stream.flush().await?;
                let mut response = String::new();
                tokio::io::BufReader::new(&mut stream)
                    .read_line(&mut response)
                    .await?;
                responses.push(serde_json::from_str(&response)?);
            }
            Ok(responses)
        })
}

/// Open the agent's pipe, waiting out the instances that are already busy.
///
/// The agent keeps one spare instance and creates the next only after accepting,
/// so the rustc processes cargo runs in parallel routinely arrive to a busy
/// pipe. Without this they would fall back to uncached compiles.
#[cfg(windows)]
async fn connect_pipe(endpoint: &str) -> Result<tokio::net::windows::named_pipe::NamedPipeClient> {
    use std::time::Instant;
    use windows_sys::Win32::Foundation::ERROR_PIPE_BUSY;

    const RETRY_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);
    const RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(5);

    let deadline = Instant::now() + RETRY_DEADLINE;
    loop {
        match tokio::net::windows::named_pipe::ClientOptions::new().open(endpoint) {
            Ok(client) => return Ok(client),
            Err(error)
                if error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32)
                    && Instant::now() < deadline => {}
            Err(error) => {
                return Err(error).wrap_err("failed to connect to the cache session");
            }
        }
        tokio::time::sleep(RETRY_DELAY).await;
    }
}

#[cfg(unix)]
fn sync_handshake(stream: &mut (impl std::io::Read + Write)) -> Result<()> {
    let request = AgentRequest::Hello {
        protocol: AGENT_PROTOCOL_VERSION,
        client_version: VERSION.into(),
    };
    write_request_line(stream, &request)?;
    let mut response = String::new();
    BufReader::new(&mut *stream).read_line(&mut response)?;
    validate_handshake_response(&response)
}

#[cfg(unix)]
fn sync_request(
    stream: &mut (impl std::io::Read + Write),
    request: &AgentRequest,
) -> Result<AgentResponse> {
    write_request_line(stream, request)?;
    let mut response = String::new();
    BufReader::new(&mut *stream).read_line(&mut response)?;
    Ok(serde_json::from_str(&response)?)
}

/// Send one request as a single write.
///
/// Serializing straight onto the socket emits a syscall per JSON fragment,
/// and a prediction payload has thousands of them, each waking the agent's
/// reader. The encoded line is assembled first so the kernel sees it once.
#[cfg(unix)]
fn write_request_line(stream: &mut impl Write, request: &AgentRequest) -> Result<()> {
    let mut encoded = serde_json::to_vec(request)?;
    encoded.push(b'\n');
    stream.write_all(&encoded)?;
    stream.flush()?;
    Ok(())
}

pub(super) fn validate_handshake_response(response: &str) -> Result<()> {
    match serde_json::from_str(response)? {
        AgentResponse::Hello {
            protocol,
            agent_version,
        } if protocol == AGENT_PROTOCOL_VERSION && agent_version == VERSION => Ok(()),
        AgentResponse::Error { message } => bail!(message),
        _ => bail!("the cache agent returned an incompatible handshake"),
    }
}
