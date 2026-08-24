# Protocol compatibility

mr-boxington has two versioned protocols. They have deliberately different
compatibility rules.

## Shim/agent protocol

The shim and its task-scoped agent exchange newline-delimited JSON values over a
local socket. The first request and response are always `hello` values carrying
`AGENT_PROTOCOL_VERSION` and the mr-boxington package version.

Both versions must match exactly. A shim and agent from different builds fail
the handshake and do not exchange cache requests. Adding, removing, or changing
a request or response therefore requires incrementing `AGENT_PROTOCOL_VERSION`.

`crates/mbx-cache-core/tests/agent_protocol.rs` exercises every request and
response variant against `tests/fixtures/agent-protocol-v1.jsonl`. Its exhaustive
matches make a newly added variant fail to compile until the fixture and the
protocol-version decision are reviewed together.

## Remote cache protocol

Remote cache endpoints live below `/v{PROTOCOL_VERSION}/` and every request
carries `mbx-cache-protocol` and `mbx-cache-namespace` headers. The v1 baseline
uses these resources:

| Operation | Method and path | Representation |
| --- | --- | --- |
| capabilities | `GET /v1/capabilities` | JSON capability document |
| action result | `GET`/`PUT /v1/action-results/{algorithm}/{hash}/{size}` | `application/vnd.mbx.cache-action-result.v1+json` |
| action manifest | `GET`/`PUT /v1/action-manifests/{algorithm}/{hash}/{size}` | `application/vnd.mbx.cache-task-action-manifest.v1+json` |
| blob | `GET`/`PUT /v1/blobs/{algorithm}/{hash}/{size}` | media type requested by the caller |
| blob pack | `POST /v1/blobs:pack` | `application/vnd.mbx.cache-blob-pack.v1` |

The capabilities endpoint is optional: `404`, `405`, or `501` selects the v1
baseline without extensions. Advertised capabilities must report the same
protocol major. Optional response fields may be added only when existing
clients ignore them; the canonical persisted records use
`#[serde(deny_unknown_fields)]`, so their shape or meaning requires a new media
type and protocol major.

Content-addressed records are serialized with the JSON Canonicalization Scheme
before hashing. The conformance fixture locks their canonical v1 bytes, the
protocol constants, and the normalized shape of every local-agent message to
prevent accidental drift.

## Rust API compatibility

Published Rust APIs follow semantic versioning independently of the wire
protocols. Pull requests that touch workspace crates run `cargo-semver-checks`
against the pull request's base commit with all features enabled. An intentional
breaking API change must include the corresponding major version change; wire
format changes still require the protocol-version steps above.
