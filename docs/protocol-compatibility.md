---
description: Compatibility rules for the local agent protocol, remote cache HTTP contract, and published Rust crates.
---
# Protocol compatibility

mbx has two wire protocols and a separately versioned Rust API surface.

| Integration | Compatibility rule |
| --- | --- |
| Local compiler shim and agent | Exact protocol and application-version match |
| Remote cache client and server | Versioned HTTP contract with negotiated extensions |
| Published Rust subcrates | Independent crate versions; embedding APIs remain on `0.x` |

For CLI and JSON output guarantees, see [Stability](/stability). Most mbx users
do not need to interact with these protocols directly.

## Shim/agent protocol

The shim and its task-scoped agent exchange newline-delimited JSON values over a
local socket. The first request and response are always `hello` values carrying
`AGENT_PROTOCOL_VERSION` and the mr-boxington package version.

Both versions must match exactly. A shim and agent from different builds fail
the handshake and do not exchange cache requests. Adding, removing, or changing
a request or response therefore requires incrementing `AGENT_PROTOCOL_VERSION`.

`crates/mbx-cache-core/tests/agent_protocol.rs` exercises every request and
response variant against `tests/fixtures/agent-protocol-v11.jsonl`. Its exhaustive
matches make a newly added variant fail to compile until the fixture and the
protocol-version decision are reviewed together.

### Local protocol history

Every revision still requires exact protocol and application-version equality,
including when different applications embed the client and agent.

| Version | Addition |
| --- | --- |
| v2 | Compiler-duration accounting for hits and real compilations |
| v3 | Crate names on hits; `begin_task` and `commit_task` for per-command manifests |
| v4 | `record_warning` forwards shim diagnostics for the agent to print once |
| v5 | `find_file_digests` and `record_file_digests` share hashes within a session |
| v6 | `join_action_promise` and `complete_action_promise` coordinate remote compilations |
| v7 | `resolve_file_digests` coalesces simultaneous hashing requests |
| v8 | `pins` on `store_executable_identity` validate cached compiler and linker probes |
| v9 | `record_debug` and `debug_recorded` forward routine shim logs to the session logger |
| v10 | Hard-linked output accounting on `record_action_hit` restore statistics |
| v11 | `unit_id` and `dependencies` on `record_wrapper_timing` identify each build unit and what it consumed |

Routine debug logs retain their module target and do not consume the
warning/error diagnostic allowance.

## Remote cache protocol

The dependency-light `mbx-cache-protocol` crate owns the remote wire records,
capability schema, media types, headers, and blob-pack framing constants. Both
the client and server compile against that crate; transport, authentication,
storage, and adapter execution remain implementation details of their
respective packages.

Remote cache endpoints live below `/v{PROTOCOL_VERSION}/` and every request
carries `mbx-cache-protocol` and `mbx-cache-namespace` headers. The v1 protocol
defines these resources; extensions require the capabilities described below:

| Operation | Method and path | Representation |
| --- | --- | --- |
| capabilities | `GET /v1/capabilities` | JSON capability document |
| action result | `GET`/`PUT /v1/action-results/{algorithm}/{hash}/{size}` | `application/vnd.mbx.cache-action-result.v1+json` |
| action manifest | `GET`/`PUT /v1/action-manifests/{algorithm}/{hash}/{size}` | `application/vnd.mbx.cache-task-action-manifest.v1+json` |
| action result batch | `POST /v1/action-results:batch` | `application/vnd.mbx.cache-action-result-batch.v1+json` |
| action promise | `POST`/`PUT /v1/action-promises/{algorithm}/{hash}/{size}` | `application/vnd.mbx.cache-action-promise.v1+json` |
| blob | `GET`/`PUT /v1/blobs/{algorithm}/{hash}/{size}` | media type requested by the caller |
| blob pack | `POST /v1/blobs:pack` | `application/vnd.mbx.cache-blob-pack.v1` |
| blob pack upload | `POST /v1/blobs:pack-upload` | `application/vnd.mbx.cache-blob-pack-receipt.v1+json` |

The batched and packed resources are extensions, each gated on its own
capability (`features.action_batch`, `features.blob_packs`, and
`features.blob_pack_uploads`) and bounded by `limits.max_batch_items` and
`limits.max_pack_bytes`. A client falls back to the single-object resources
when a feature is not advertised, and also when an advertised endpoint answers
`404`, `405`, or `501`, after which it stops asking for the rest of the
session. None of them is required to serve the baseline.

Action promises are gated by `features.action_promises`. `POST` atomically
returns a claim token, a bounded retry delay while another lease is live, or a
completed `ActionPrediction`. `PUT` presents the claim token and prediction to
complete the promise. A server must authorize both operations as writes, expire
abandoned claims, refuse a completion whose action result is not already
durable, and make the first valid completion immutable. Claim tokens are opaque
bearer values and clients cap them at 256 bytes. As with other extensions,
`404`, `405`, or `501` disables promises for the rest of the client session.

A batched action-result response carries only the records the service holds, in
no order, so each one is bound to its request by the action digest inside it,
not by position: a client refuses a batch naming an action it did not ask for.
An uploaded pack repeats the `MBXPACK1` framing of a downloaded one and declares
its contents in the pack headers. Every blob in it is content-addressed and
immutable, so a rejected pack may leave an accepted prefix stored; the client
then republishes its blobs individually.

The capabilities endpoint is optional: `404`, `405`, or `501` selects the v1
baseline without extensions. Advertised capabilities must report the same
protocol major. Optional response fields may be added only when existing
clients ignore them; the canonical persisted records use
`#[serde(deny_unknown_fields)]`, so changing their shape or meaning requires a
new media type and protocol major.

Content-addressed records are serialized with the JSON Canonicalization Scheme
before hashing. The conformance fixture locks their canonical v1 bytes, the
protocol constants, and the normalized shape of every local-agent message to
prevent accidental drift.

Action manifests are updated conditionally: a `GET` returns a strong `ETag` that
the following `PUT` sends back as `If-Match`, and a first write uses
`If-None-Match: *`. That entity tag is **opaque**. A client must echo it
unchanged and must not read content from it, because a manifest can reach the
client through an intermediary. RFC 9110 section 8.8.3.3 requires a proxy that
compresses a response to vary the strong tag along with the coding, and Caddy
does so by appending the coding name. The body is validated by comparing it
against its canonical JSON and checking the task identity it claims, never by
the tag. A server may therefore choose any strong tag; a weak one leaves the
manifest readable but not updatable.

## Rust API compatibility

Published Rust APIs are versioned independently of the wire protocols. The
subcrates remain on `0.x`, so their APIs may change in a minor release and CI
does not currently enforce API compatibility. Wire format changes still require
the protocol-version steps above.

Release-plz chooses version bumps from conventional commits. Declare breaking
API changes with `!` and a `BREAKING CHANGE:` footer, and isolate pre-1.0
subcrate breaks from changes to `mbx`: breaking markers apply to every crate
touched by a commit, regardless of its scope.
[RELEASING.md](https://github.com/jdx/mr-boxington/blob/main/RELEASING.md)
covers what that means for contributors.

The published crates do not all share a version, because they do not promise
the same things:

| Crate | Version line | What it promises |
| --- | --- | --- |
| `mbx` | independent | The command line: subcommands and versioned JSON output. The library target is internal. |
| `mbx-cache-protocol` | independent | The remote cache wire contract, as described above. Depend on this to speak to an mbx cache. |
| `mbx-cache-core` | shared `0.x` | Unstable session, store, and agent primitives for coordinated embedding. |
| `mbx-cache-rustc` | shared `0.x` | Unstable rustc action modeling for coordinated embedding. |
| `mbx-cache-cc` | shared `0.x` | Unstable C and C++ action modeling for coordinated embedding. |
| `mbx-cache-cargo` | independent `0.x` | Unstable Cargo invocation and shared-cache-root resolution. |
| `mbx-cache-store` | independent `0.x` | Unstable checkout claims and disk-bounded shared-store GC. |

The embedding crates stay on `0.x`, where a minor bump is allowed to break.
`mbx-cache-core`, `mbx-cache-rustc`, and `mbx-cache-cc` move together; the
Cargo and store crates release independently. Pin compatible minors and expect
an upgrade to require source changes.

The Rust types mirror which wire records are open to extension and which are
not. Capability records are `#[non_exhaustive]`, so a newly advertised feature
or limit is a minor release; build them with `Capabilities::new` and assign the
fields a service supports. The canonical persisted records are exhaustive,
because their shape is covered by a digest and a change to it requires a new
media type and protocol major. `BypassReason` is also `#[non_exhaustive]`: the
set of uncacheable invocations shifts as the adapter learns to model more of
them, so aggregate on `BypassReason::kind` instead of matching every variant.

The same rule sorts the statistics types. `AgentStats` and `CompilerStats`
gain counters as sessions measure more, so both are open to extension; build
them from `default` or `CompilerStats::new`. `RestoreStats`
stays constructible, because the shim reports one on every compilation; it is
an input to the crate.
