---
description: Understand compatibility rules for the local agent protocol, remote cache HTTP contract, and published Rust crates.
---
# Protocol compatibility

mbx has two wire protocols and a separately versioned Rust API surface.

| Integration | Compatibility rule |
| --- | --- |
| Local compiler shim and agent | Exact protocol and application-version match |
| Remote cache client and server | Versioned HTTP contract with negotiated extensions |
| Published Rust subcrates | Independent crate versions; embedding APIs remain on `0.x` |

You do not need to work with these protocols to use mbx. This page is for
cache server implementers and for applications that embed the mbx crates. For
CLI and JSON output guarantees, see [Stability](/stability).

## Shim/agent protocol

The compiler shims and the cache agent of one
[build session](/how-it-works#from-command-to-result) exchange
newline-delimited JSON values over a local socket. The first request and
response are always `hello` values. Each carries `AGENT_PROTOCOL_VERSION` and
the application version, which for the mbx CLI is the `mbx` package version.

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

Debug logs forwarded with `record_debug` keep their module target for
filtering and do not count against the agent's allowance of warning and
error diagnostics.

## Remote cache protocol

The dependency-light `mbx-cache-protocol` crate owns the remote wire records,
capability schema, media types, headers, and blob-pack framing constants. Both
the client and server compile against that crate; transport, authentication,
storage, and adapter execution remain implementation details of their
respective packages.

Remote cache endpoints live below `/v{PROTOCOL_VERSION}/`, and every request
carries the `mbx-cache-protocol` and `mbx-cache-namespace` headers.
Action-result batches, blob packs, blob pack uploads, and action promises
are extensions, each gated on its own capability. The v1 protocol defines
these resources:

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

The batched and packed resources are gated on `features.action_batch`,
`features.blob_packs`, and `features.blob_pack_uploads` and bounded by
`limits.max_batch_items` and `limits.max_pack_bytes`. A client falls back to
the single-object resources when a feature is not advertised. It also falls
back when an advertised endpoint answers `404`, `405`, or `501`, and then
stops using that extension for the rest of the client session. A server does
not need any of these extensions to serve the baseline.

Action promises, gated by `features.action_promises`, let one client claim a
compilation so that other clients can wait for its result instead of
repeating it. `POST` atomically returns one of three states:

- a claim token, when this client owns the lease and should compile
- a bounded retry delay, while another client's lease is live
- a completed `ActionPrediction`, once the lease owner has published one

`PUT` presents the claim token and prediction to complete the promise. A
server must:

- authorize both operations as writes
- expire abandoned claims
- refuse a completion whose action result is not already durable
- make the first valid completion immutable

Claim tokens are opaque bearer values, and clients cap them at 256 bytes. As
with the other extensions, `404`, `405`, or `501` disables promises for the
rest of the client session.

A batched action-result response carries only the records the server holds, in
no particular order. The client binds each record to its request by the action
digest inside it, not by position, and refuses a batch that names an action it
did not ask for.

An uploaded pack repeats the `MBXPACK1` framing of a downloaded one and
declares its blob count and payload bytes in the `mbx-cache-pack-blobs` and
`mbx-cache-pack-bytes` headers. A rejected pack may leave an accepted prefix
stored. That is harmless: every blob is content-addressed and immutable, and
the client then republishes the pack's blobs individually.

The capabilities endpoint is optional for builds: `404`, `405`, or
`501` selects the v1 baseline without extensions. `mbx doctor` does
require it. Its remote check negotiates capabilities without that
fallback, so a server without the endpoint fails the check even though
builds still use the baseline.

Advertised capabilities must report the same protocol major as the client.
Optional response fields may be added only when existing clients ignore them.
The canonical persisted records use `#[serde(deny_unknown_fields)]`, so
changing their shape or meaning requires a new media type and protocol major.

Content-addressed records are serialized with the JSON Canonicalization Scheme
before hashing. The conformance test and fixture described in
[Shim/agent protocol](#shim-agent-protocol) also lock their canonical v1 bytes,
the protocol constants, and the normalized shape of every local-agent message
to prevent accidental drift.

An action manifest holds the
[predictions](/how-it-works#prediction-and-dep-info) recorded for one task, and
clients update it conditionally. A `GET` returns a strong `ETag` that the
following `PUT` sends back as `If-Match`, and a first write uses
`If-None-Match: *`. That entity tag is **opaque**. A client must echo it
unchanged and must not read content from it, because a manifest can reach the
client through an intermediary. RFC 9110 section 8.8.3.3 requires a proxy that
compresses a response to vary the strong tag along with the coding, and Caddy
does so by appending the coding name. The client validates the body by
comparing it against its canonical JSON and checking the task identity it
claims, never by the tag. A server may therefore choose any strong tag. mbx
refuses a manifest response whose `ETag` is weak or missing and treats it as a
failed lookup, so such a server can neither serve nor update manifests.

## Rust API compatibility

Published Rust APIs are versioned independently of the wire protocols. The
subcrates remain on `0.x`, so their APIs may change in a minor release, and CI
does not currently enforce API compatibility. Wire format changes still require
the protocol-version steps in [Shim/agent protocol](#shim-agent-protocol) and
[Remote cache protocol](#remote-cache-protocol).

Release-plz chooses version bumps from Conventional Commits. Declare a breaking
API change with `!` and a `BREAKING CHANGE:` footer. Keep pre-1.0 subcrate
breaks out of commits that change `mbx`, because a breaking marker applies to
every crate the commit touches, whatever the commit's scope.
[RELEASING.md](https://github.com/jdx/mr-boxington/blob/main/RELEASING.md)
covers what that means for contributors.

The published crates do not all share a version, because they do not promise
the same things:

| Crate | Version line | What it promises |
| --- | --- | --- |
| `mbx` | independent | The command line: subcommands and versioned JSON output. The library target is internal. |
| `mbx-cache-protocol` | independent | The wire contract in [Remote cache protocol](#remote-cache-protocol). Depend on it to write a cache client or server. |
| `mbx-cache-core` | shared `0.x` | Unstable session, store, and agent primitives for coordinated embedding. |
| `mbx-cache-rustc` | shared `0.x` | Unstable rustc action modeling for coordinated embedding. |
| `mbx-cache-cc` | shared `0.x` | Unstable C and C++ action modeling for coordinated embedding. |
| `mbx-cache-cargo` | independent `0.x` | Unstable Cargo invocation and cache directory resolution. |
| `mbx-cache-store` | independent `0.x` | Unstable checkout claims and disk-bounded collection of the shared store. |

`mbx-cache-core`, `mbx-cache-rustc`, and `mbx-cache-cc` move together; the
Cargo and store crates release independently. Because a minor bump of a
`0.x` crate may break, pin compatible minors and expect an upgrade to
require source changes.

The Rust types mirror which wire records are open to extension and which are
not. Capability records are `#[non_exhaustive]`, so a newly advertised feature
or limit is a minor release. Build them with `Capabilities::new` and assign the
fields your service supports. The canonical persisted records are exhaustive,
because their shape is covered by a digest and a change to it requires a new
media type and protocol major. `BypassReason` is also `#[non_exhaustive]`,
because the set of uncacheable invocations shifts as the adapter learns to
model more of them. Aggregate on `BypassReason::kind` instead of matching
every variant.

The same rule sorts the statistics types. `AgentStats` and `CompilerStats`
gain counters as sessions measure more, so both are open to extension; build
them from `default` or `CompilerStats::new`. `RestoreStats` stays
constructible because the shim reports one on every compilation: it is an
input to the crate.
