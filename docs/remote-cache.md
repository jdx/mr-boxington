---
description: Connect mbx to a cache server or S3-compatible bucket and understand authentication and write policy.
---
# Remote cache

A remote cache lets ephemeral runners and teammates restore the compiler
actions a build needs. The local content-addressed store remains the working
cache; remote objects are downloaded into it and newly completed actions may be
uploaded from trusted CI.

Choose a **cache server** for server-side authorization and optional batch
transfers, compression, and compilation deduplication. Choose an
**S3-compatible bucket** to use object storage directly. The URL scheme
selects the backend.

| Backend | Use it when | Credentials |
| --- | --- | --- |
| [Cache server](#configure-a-server) | You want server-side grants and negotiated transfer extensions | Bearer token or GitHub OIDC |
| [S3-compatible bucket](#configure-an-s3-compatible-bucket) | You already operate object storage | Exported AWS credentials |
| [GitHub Actions cache](/github-action) | You want managed archive storage for GitHub jobs | Handled by the action |

Put `[remote]` settings in your [global configuration](/configuration), or use
`MBX_REMOTE_*` environment variables in CI. They are not accepted from a
repository's `.mbx.toml`. A configured `read-write` mode is still subject to
[the environment's write policy](#read-and-write-policy).

## Configure a server

```toml
[remote]
url = "https://cache.example.com"
namespace = "acme/backend"
mode = "read-write"
```

The namespace isolates one project's cache from another. It is required when a
remote URL is set. To host your own server, see [cache server](/cache-server).

## Authenticate

For a cache server, choose one authentication method:

- `MBX_REMOTE_TOKEN` for a bearer token.
- `MBX_REMOTE_TOKEN_FILE` for a file containing the token.
- `MBX_REMOTE_OIDC_AUDIENCE` for CI-issued OIDC credentials.

Avoid long-lived secrets in pull request workflows. On GitHub Actions, OIDC
requires `id-token: write` permission.

These settings are specific to cache servers. For an `s3://` URL, use the
[AWS credential variables](#configure-an-s3-compatible-bucket) instead.

## Configure an S3-compatible bucket

```toml
[remote]
url = "s3://acme-build-cache"
namespace = "acme/backend"
mode = "read-write"
```

Credentials come from the standard AWS environment variables:
`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, and `AWS_SESSION_TOKEN` for
temporary credentials. `MBX_REMOTE_S3_REGION` names the signing region, falling
back to `AWS_REGION` or `AWS_DEFAULT_REGION`.

Export temporary credentials into those variables before starting mbx. On GitHub
Actions,
[`aws-actions/configure-aws-credentials`](https://github.com/aws-actions/configure-aws-credentials)
can exchange the runner's OIDC token for temporary role credentials. mbx does
not directly read EKS IRSA, EC2 or ECS instance roles, `~/.aws/config` profiles,
or SSO sessions; export their credentials first.

The URL may carry a prefix, as `s3://acme-build-cache/teams/backend`, to share
one bucket between projects. Keys are laid out under
`<prefix>/<namespace>/v1/`, so a bucket policy can scope a writer to its own
prefix.

Readers need `s3:GetObject` on that prefix. Trusted writers also need
`s3:PutObject`. Add
`s3:ListBucket` on the bucket as well. mbx never lists anything, but without
that permission AWS answers `403` instead of `404` for an absent object, so a
miss cannot be told apart from a refusal.

Without it mbx still works: a refused read is treated as a miss, and it says so
once. A credential that cannot read the cache then looks the same as a cold
one, and only the warning distinguishes them. Credentials that S3 itself
rejects, such as a wrong secret or an expired token, are reported as errors
either way.

### Cloudflare R2 and MinIO

Set an endpoint, which switches to addressing the bucket in the path:

```toml
[remote]
url = "s3://acme-build-cache"
namespace = "acme/backend"
s3_endpoint = "https://<account>.r2.cloudflarestorage.com"
s3_region = "auto"
```

For MinIO, use its endpoint and signing region. `http://` is refused for anything but
a loopback address, since a signature and the objects it fetches are readable
in transit without TLS.

### What a bucket does not do

A cache server verifies every blob against the digest in its URL before storing
it. A bucket stores what it is given, so a corrupted object in the local store
can be published under a key naming different content. Writes are create-only,
so nothing later overwrites it: every machine that downloads it fails
verification and recompiles. `mbx cache verify` finds such an object locally.

A bucket also does not coordinate in-flight compilations, answer batched
lookups, stream blob packs, or negotiate compression. mbx asks for none of
them against S3 and falls back to per-object requests, the same requests it
makes against a server without the extensions. Expect more requests for the
same build, and no compression on the wire.

The task action manifest that drives [prefetch](#prefetch) is updated in place.
Conditional writes prevent concurrent updates from overwriting one another. AWS
S3, R2, and current MinIO all implement them. Against a store that does not, mbx
says so once and continues without them. Blobs and action results are
content-addressed, so writing one twice is harmless, but concurrent manifest
updates can then lose each other's predictions, which costs prefetch coverage on
later builds. `MBX_REMOTE_S3_CONDITIONAL_WRITES=required` refuses such a store
instead.

### Who may publish

A cache server enforces namespace grants; a bucket enforces its object-storage
permissions. Whatever a bucket credential can write is within reach of code
holding that credential. The client-side [write
policy](#read-and-write-policy) still applies, so pull requests never publish,
but code holding a write credential can bypass mbx and write to the bucket
directly. Enforce the same restriction in your storage permissions.

Scope the write credential to the builds you trust. On GitHub Actions, restrict
the role's trust policy to the branches that may assume it, and give pull
request jobs a read-only role.

## Read and write policy

Configured mode is constrained by the environment:

| Context | Effective behavior |
| --- | --- |
| Protected branch push on GitHub Actions or GitLab CI | Configured mode |
| Pull request, merge request, local shell, or unprotected branch | Read-only; `write-only` becomes disabled |
| Tag or release build | Read-only; `write-only` becomes disabled |

This policy prevents untrusted code from publishing objects that later builds
would trust. The server should still authenticate and authorize requests; the
client-side policy is not an access-control boundary.

## GitLab CI

GitLab CI follows the same write policy as GitHub Actions: a
push pipeline on a protected branch may write, merge requests and unprotected
branches are read-only, and tag pipelines cannot publish to the remote.

The OIDC flow is GitHub-specific, so authenticate GitLab jobs with a bearer
token in a masked, protected CI/CD variable:

```yaml
# Install mbx and a Rust toolchain in the job image first.
build:
  variables:
    MBX_REMOTE_URL: https://cache.example.com
    MBX_REMOTE_NAMESPACE: acme/backend
    MBX_REMOTE_MODE: read-write
  script:
    - mbx build --workspace
```

Set `MBX_REMOTE_TOKEN` in the project's CI/CD variables, masked and limited to
protected branches, not in the YAML.

## Prefetch

After a command has published its action manifest, another machine can warm
the same build without running Cargo:

```sh
mbx prefetch build --workspace --release
```

The workspace's `Cargo.lock` and complete Cargo argument list select the same
manifest as a normal mbx build. Prefetch requires a configured remote in
`read-only` or `read-write` mode, waits for every predicted action, and reports
what it pulled:

```text
prefetched 161 actions; 214.8 MiB downloaded and 214.8 MiB stored locally
```

If no manifest has been published for the workspace and command, mbx prints
`no recorded actions for this workspace and Cargo command` and exits
successfully. There was nothing to fetch, which is normal for a first build. A
lockfile that has not been built yet borrows the manifest of the lockfile before
it in Git history, so a dependency bump still prefetches the unchanged part of
the graph. A shallow checkout needs that history fetched. For GitHub Actions
builds of the default pull-request merge commit, `fetch-depth: 2` includes its
base parent; builds of a PR head or another ref may need more history. A lookup
that fails, such as an unreachable host or refused credentials, is an error, so
CI can tell an empty cache from a broken one. A typical place for the command is
a runner or devcontainer image's start-up hook, so the store is warm before
anyone builds.

## In-flight deduplication

When a cache server advertises action promises, read-write runners atomically
claim a cold compiler invocation before starting it. One runner compiles and
publishes the result; other runners wait for its promise, rebuild the final
action key from the promised input prediction, verify every input, and restore
the published result. The prediction is only fulfilled after the action result
and all referenced blobs are remotely durable.

Claims are keyed by the pre-discovery invocation digest because a cold runner
does not yet know the compiler-discovered inputs in the final action key. They
are leases: a runner that dies or cannot publish leaves no durable cache record,
and the server expires its claim so another runner can compile. Any endpoint
error, unsupported server, read-only policy, or expired client wait degrades to
an ordinary compilation. Read-only runners never acquire claims.

## Deferred publication

Uploads are not on the critical path of the build that produced them. They are
queued while the build continues and drained before the session exits. An action
result is published only after every blob it references, so a reader never
fetches a result whose outputs it cannot restore. A command killed before the
queue drains may leave some results only in the local store. Other machines must
compile work that was not uploaded.

A failed upload is reported, counted in `remote_failures`, and recovered from;
the build keeps its local result either way. The session summary reports what
was published and what the drain cost. Where the server accepts them, queued
blobs are published several to a request:

```text
mbx[cache]: uploads: 143 published (118 of them in 2 packs), 0 not published; 412.0ms waited for after the build
```

`MBX_STATS_REPORT` carries the same figures as `background_uploads`,
`background_upload_failures`, `upload_drain_duration_ns`,
`remote_blob_pack_uploads`, and `remote_blob_pack_upload_blobs`.

## Batched lookups

Prefetch requests known actions together when the server supports batched
lookups, reducing the number of HTTP requests. `remote_action_lookups` counts
requests, not actions, so the same build reports far fewer of them against a
server with the extension.

Batch lookups and packed uploads are negotiated. A server without them, or one that advertises an
endpoint it does not serve, gets the single-object requests every version of mbx
has made. Nothing needs configuring either way.

## Transfer behavior

Blobs travel zstd-compressed when the server supports it: the client accepts
compressed downloads, and it compresses uploads when the server's capabilities
list `zstd` among its compressors. Failed requests are retried according to
`MBX_HTTP_RETRIES`, and a stalled attempt is cut short by `MBX_HTTP_TIMEOUT`.

`MBX_HTTP_DOWNLOAD_TIMEOUT` exists separately because artifacts can be much
larger than metadata responses. It is a deadline for the whole download and
spans every retry and the backoff between them, which bounds how long one blob
can hold a build open. A packed request scales the deadline up with the bytes
and object count it asks for, since the configured value describes a single
blob. Raise it if a slow link makes large artifacts run out of time before
their retries are spent.

The download deadline applies per object. `MBX_HTTP_READ_STALL_BUDGET` limits
the cumulative time spent on failed reads in a session. Once that budget is
exhausted, mbx stops remote reads and compiles locally. Successful reads do not
count against the budget, even when slow. Set it to `0` to disable this
session-wide limit while retaining the per-download deadline.
