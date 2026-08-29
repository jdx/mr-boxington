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
response variant against `tests/fixtures/agent-protocol-v5.jsonl`. Its exhaustive
matches make a newly added variant fail to compile until the fixture and the
protocol-version decision are reviewed together.

Agent protocol v2 adds compiler-duration accounting to hits and real compiler
invocations. v3 adds the crate name to a recorded hit plus `begin_task` and
`commit_task`, allowing an embedded Cargo shim to create one prediction manifest
for each real Cargo invocation. v4 adds `record_warning`, which is how a shim
reports a diagnostic at all: a C or C++ shim stands in for a compiler whose
stderr its caller reads as an answer, so it cannot write there itself, and the
agent prints each distinct message once from the process that owns the build.
v5 adds `find_file_digests` and `record_file_digests`, which let a shim reuse
the digest of a file the session already read in full instead of rehashing it.
The client and agent still require exact protocol and application-version
equality, including when different applications ship them.

## Remote cache protocol

The dependency-light `mbx-cache-protocol` crate owns the remote wire records,
capability schema, media types, headers, and blob-pack framing constants. Both
the client and server compile against that crate; transport, authentication,
storage, and adapter execution remain implementation details of their
respective packages.

Remote cache endpoints live below `/v{PROTOCOL_VERSION}/` and every request
carries `mbx-cache-protocol` and `mbx-cache-namespace` headers. The v1 baseline
uses these resources:

| Operation | Method and path | Representation |
| --- | --- | --- |
| capabilities | `GET /v1/capabilities` | JSON capability document |
| action result | `GET`/`PUT /v1/action-results/{algorithm}/{hash}/{size}` | `application/vnd.mbx.cache-action-result.v1+json` |
| action manifest | `GET`/`PUT /v1/action-manifests/{algorithm}/{hash}/{size}` | `application/vnd.mbx.cache-task-action-manifest.v1+json` |
| action result batch | `POST /v1/action-results:batch` | `application/vnd.mbx.cache-action-result-batch.v1+json` |
| blob | `GET`/`PUT /v1/blobs/{algorithm}/{hash}/{size}` | media type requested by the caller |
| blob pack | `POST /v1/blobs:pack` | `application/vnd.mbx.cache-blob-pack.v1` |
| blob pack upload | `POST /v1/blobs:pack-upload` | `application/vnd.mbx.cache-blob-pack-receipt.v1+json` |

Both batched resources are extensions, gated on their own capability
(`features.action_batch` and `features.blob_pack_uploads`) and bounded by
`limits.max_batch_items` and `limits.max_pack_bytes`. A client falls back to the
single-object resources when a feature is not advertised, and also when an
advertised endpoint answers `404`, `405`, or `501` — after which it stops asking
for the rest of the session. Neither is required to serve the baseline.

A batched action-result response carries only the records the service holds, in
no order, so each one is bound to its request by the action digest inside it
rather than by position: a client refuses a batch naming an action it did not
ask for. An uploaded pack repeats the `MBXPACK1` framing of a downloaded one and
declares its contents in the pack headers; because every blob in it is
content-addressed and immutable, a rejected pack may leave an accepted prefix
stored, and the client republishes its blobs individually rather than relying on
that.

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

Action manifests are updated conditionally: a `GET` returns a strong `ETag` that
the following `PUT` sends back as `If-Match`, and a first write uses
`If-None-Match: *`. That entity tag is **opaque**. A client must echo it
unchanged and must not read content from it, because a manifest can reach the
client through an intermediary — RFC 9110 section 8.8.3.3 requires a proxy that
compresses a response to vary the strong tag along with the coding, and Caddy
does so by appending the coding name. What the body is gets established by
comparing it against its canonical JSON and validating the task identity it
claims, never by the tag. A server may therefore choose any strong tag; a weak
one leaves the manifest readable but not updatable.

## Rust API compatibility

Published Rust APIs follow semantic versioning independently of the wire
protocols. Pull requests that touch workspace crates run `cargo-semver-checks`
against the pull request's base commit with all features enabled. Wire format
changes still require the protocol-version steps above.

A breaking API change is *declared*, not numbered by hand: the commit carries
`feat!:` or a `BREAKING CHANGE:` footer, and release-plz prices it into the
version when it opens the release PR. What that means for contributors —
including why a deliberate break leaves the semver check red until the release
PR exists — is covered in
[RELEASING.md](https://github.com/jdx/mr-boxington/blob/main/RELEASING.md).

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
smaller Cargo and store crates release independently. Pin compatible minors and expect an
upgrade to require source changes.

The Rust types mirror which wire records are open to extension and which are
not. Capability records are `#[non_exhaustive]`, so a newly advertised feature
or limit is a minor release rather than a major one; build them with
`Capabilities::new` and assign the fields a service actually supports. The
canonical persisted records are deliberately left exhaustive, because their
shape is covered by a digest and a change to it requires a new media type and
protocol major rather than an added field. `BypassReason` is also
`#[non_exhaustive]`: the set of uncacheable invocations shifts as the adapter
learns to model more of them, so aggregate on `BypassReason::kind` rather than
matching every variant.

The same rule sorts the statistics types. `AgentStats` and `CompilerStats`
accumulate whatever a session turns out to be worth measuring, so both are open
to extension; build them from `default` or `CompilerStats::new`. `RestoreStats`
stays constructible, because the shim reports one on every compilation — it is
an input to the crate rather than something the crate hands back.
