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
response variant against `tests/fixtures/agent-protocol-v3.jsonl`. Its exhaustive
matches make a newly added variant fail to compile until the fixture and the
protocol-version decision are reviewed together.

Agent protocol v3 adds `begin_task` and `commit_task`, allowing an embedded
Cargo shim to create one prediction manifest for each real Cargo invocation.
The client and agent still require exact protocol and application-version
equality, including when they are shipped by different applications.

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

A breaking API change is *declared*, not numbered by hand. Mark the commit
breaking -- `feat!:`, or a `BREAKING CHANGE:` footer -- and release-plz prices
it into the version when it opens the release PR. Do not edit a crate's version
in a pull request: release-plz owns those numbers, and it knows that a crate on
`0.x` needs a minor bump where a `1.x` crate would need a major one.

One consequence is worth knowing before it surprises you. A pull request that
breaks an API on purpose leaves `public API compatibility` red, because the
version it compares against cannot move until the release PR exists. The
breaking marker on the commit is what makes the break deliberate; the red check
is the expected state of an honest breaking change, not a problem to solve
inside the pull request.

Read that job's output as a list of what broke, not as an instruction. Its
summary says "semver requires new major version" whatever the crate's position,
so for the crates on `0.x` it names a bump Cargo does not want -- there, a break
is a minor. The lint names above the summary are the useful part: they say which
items changed shape, which is what a reviewer needs to judge whether the break
was intended.

The published crates do not all share a version, because they do not promise
the same things:

| Crate | Version line | What it promises |
| --- | --- | --- |
| `mbx` | independent | The command line: subcommands and versioned JSON output. The library target is internal. |
| `mbx-cache-protocol` | independent | The remote cache wire contract, as described above. Depend on this to speak to an mbx cache. |
| `mbx-cache-core` | shared `0.x` | Unstable session, store, and agent primitives for coordinated embedding. |
| `mbx-cache-rustc` | shared `0.x` | Unstable rustc action modeling for coordinated embedding. |
| `mbx-cache-cargo` | independent `0.x` | Unstable Cargo invocation and shared-cache-root resolution. |
| `mbx-cache-store` | independent `0.x` | Unstable checkout claims and disk-bounded shared-store GC. |

The embedding crates stay on `0.x`, where a minor bump is allowed to break.
`mbx-cache-core` and `mbx-cache-rustc` move together; the smaller Cargo and
store crates release independently. Pin compatible minors and expect an
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
