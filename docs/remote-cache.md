---
description: Connect mbx to a cache server or S3-compatible bucket and understand authentication and write policy.
---
# Remote cache

A remote cache lets ephemeral runners and teammates restore the compiler
actions a build needs. Builds still read from the local content-addressed
store: mbx downloads remote objects into it, and trusted CI can upload newly
completed actions from it.

Choose a **cache server** for server-side authorization and optional batch
transfers, compression, and compilation deduplication. Choose an
**S3-compatible bucket** to use object storage directly. The URL scheme
selects the backend.

| Backend | Use it when | Credentials |
| --- | --- | --- |
| [Cache server](#configure-a-server) | You want server-side grants and negotiated transfer extensions | Bearer token or GitHub OIDC |
| [S3-compatible bucket](#configure-an-s3-compatible-bucket) | You already operate object storage | AWS credential variables or an EC2 instance role |
| [GitHub Actions cache](/github-action#github-actions-cache) | You want managed archive storage for GitHub jobs | Handled by the action |

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

- `MBX_REMOTE_TOKEN` for a bearer token
- `MBX_REMOTE_TOKEN_FILE` for a file containing the token
- `MBX_REMOTE_OIDC_AUDIENCE` for a GitHub Actions OIDC token

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

mbx looks for credentials in two places, in order:

1. The standard AWS environment variables: `AWS_ACCESS_KEY_ID`,
   `AWS_SECRET_ACCESS_KEY`, and `AWS_SESSION_TOKEN` for temporary credentials.
2. The EC2 instance role, through the Instance Metadata Service Version 2
   (IMDSv2). mbx tries it only when `AWS_ACCESS_KEY_ID` is not set; see
   [EC2 instance roles](#ec2-instance-roles).

Without an instance role, export temporary credentials into the AWS environment
variables before starting mbx. mbx does not directly read EKS IAM Roles for
Service Accounts (IRSA), ECS task roles, `~/.aws/config` profiles, or SSO
sessions; export their credentials first. On GitHub Actions,
[`aws-actions/configure-aws-credentials`](https://github.com/aws-actions/configure-aws-credentials)
can exchange the runner's OIDC token for temporary role credentials.

`MBX_REMOTE_S3_REGION` names the signing region, falling back to `AWS_REGION` or
`AWS_DEFAULT_REGION`. When credentials come from an instance role and none of
these is set, mbx uses the region the instance reports.

The URL may carry a prefix, as `s3://acme-build-cache/teams/backend`, to share
one bucket between projects. Keys are laid out under
`<prefix>/<namespace>/v1/`, so a bucket policy can scope a writer to its own
prefix.

Readers need `s3:GetObject` on that prefix, and trusted writers also need
`s3:PutObject`. Add `s3:ListBucket` on the bucket as well. mbx never lists
anything, but without that permission AWS answers `403` instead of `404` for an
absent object, so a miss cannot be told apart from a refusal.

Without `s3:ListBucket`, mbx still works: it treats a refused read as a miss and
warns once. A credential that cannot read the cache then looks the same as a
cold one, and only the warning distinguishes them. mbx reports credentials that
S3 itself rejects, such as a wrong secret or an expired token, as errors either
way.

### EC2 instance roles

On an EC2 instance with a role attached, such as a CI runner, no setup is
needed. mbx fetches the role's credentials when a build starts and asks for new
ones a few minutes before they expire, so a build that outlives one set of
credentials keeps its remote cache. Nothing is exported, so other tools in the
same job keep using the instance profile through their own credential chain.

Before it asks the instance role, mbx checks for a credential source that the
AWS tools would consult first. It refuses the remote when it finds one, rather
than sign as the instance's identity, which can be broader than a profile, pod,
or task role. Any of these blocks the lookup:

- `AWS_ACCESS_KEY_ID` is set without `AWS_SECRET_ACCESS_KEY`.
- `AWS_PROFILE`, `AWS_WEB_IDENTITY_TOKEN_FILE`, or an
  `AWS_CONTAINER_CREDENTIALS_*` variable is set. mbx does not read those
  sources.
- A `[default]` profile in `~/.aws/credentials` or `~/.aws/config` sets access
  keys, a `credential_process`, SSO, or a role.
- One of those files exists but cannot be read.

When a profile file blocks the lookup, point `AWS_SHARED_CREDENTIALS_FILE` and
`AWS_CONFIG_FILE` at empty files to use the instance role anyway.

The metadata service throttles per instance, so mbx tries a lookup that gets a
`429`, a server error, or no answer up to three times with a short backoff.
Renewal runs in the background: requests keep signing with the credentials they
hold until the new set arrives, so a metadata service that hangs does not slow
the cache while those credentials are valid.

A job in a container on an EC2 instance can fail the first lookup with a
timeout even though the instance has a role. The metadata service's default hop
limit of 1 keeps its answer from reaching the container. The error names
`HttpPutResponseHopLimit` when the token request times out after connecting.
To fix that, raise the limit to 2 with
`aws ec2 modify-instance-metadata-options --http-put-response-hop-limit 2`.

::: warning The hop limit covers every container
The setting applies to the whole instance: every container on the host can then
read the instance role's credentials, not only the mbx job. Raise it only where
the role's permissions suit everything that runs there, or run the job outside a
container.
:::

Off EC2, a lookup that finds no metadata service fails after about a second, and
mbx refuses the remote with the usual missing-credentials error, which names
both sources. Set `AWS_EC2_METADATA_DISABLED=true` to skip the lookup, as with
the AWS SDKs. `AWS_EC2_METADATA_SERVICE_ENDPOINT` overrides the metadata
address. `mbx doctor` reports which source supplied the credentials and, for an
instance role, how long they have left.

### Cloudflare R2 and MinIO

Set an endpoint, which switches to addressing the bucket in the path:

```toml
[remote]
url = "s3://acme-build-cache"
namespace = "acme/backend"
s3_endpoint = "https://<account>.r2.cloudflarestorage.com"
s3_region = "auto"
```

For MinIO, use its endpoint and signing region. mbx refuses `http://` for
anything but a loopback address, since without TLS a signature and the objects
it fetches are readable in transit.

`s3_force_path_style` (`MBX_REMOTE_S3_FORCE_PATH_STYLE`) overrides the
addressing. Set it to `false` for a provider that requires the bucket in the
host name, or to `true` to use path addressing on AWS. Without an endpoint, mbx
already uses path addressing for a bucket whose name contains a dot. The `s3_*`
settings apply only to an `s3://` URL: with any other scheme, mbx refuses
`s3_endpoint`, `s3_region`, `s3_force_path_style`, and an
`s3_conditional_writes` other than `auto`.

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

The action manifest lists the compilations mbx
[predicts](/how-it-works#prediction-and-dep-info) for a workspace's
`Cargo.lock`, and [prefetch](#prefetch) reads it. mbx updates the manifest in
place, and conditional writes prevent concurrent updates from overwriting one
another. AWS S3, R2, and current MinIO all implement them. Against a bucket that
does not, mbx says so once and continues without them. Blobs and action results
are content-addressed, so writing one twice is harmless. Without conditional
writes, though, concurrent manifest updates can lose each other's predictions,
which costs prefetch coverage on later builds.
`MBX_REMOTE_S3_CONDITIONAL_WRITES=required` refuses such a bucket instead.

### Who may publish

A cache server enforces namespace grants; a bucket enforces only its
object-storage permissions. The client-side [write
policy](#read-and-write-policy) still keeps mbx from publishing in a pull
request, but any code holding a write credential can bypass mbx and write to
the bucket directly. Enforce the same restriction in your storage permissions.

Scope the write credential to the builds you trust. On GitHub Actions, restrict
the role's trust policy to the branches that may assume it, and give pull
request jobs a read-only role.

## Read and write policy

`mode` accepts `read-write` (the default), `read-only`, or `write-only`.
`write-only` publishes new results but never restores from the remote; use it
for a trusted job that should repopulate the remote from its own compilations
instead of restoring earlier results. `mbx prefetch` refuses it.

mbx narrows the configured mode to what the environment allows:

| Context | Effective behavior |
| --- | --- |
| Push to a protected branch (not a tag) on GitHub Actions or GitLab CI | Configured mode |
| Anything else: pull or merge requests, merge queue runs, tag pushes and `release` events, scheduled or manually dispatched runs, unprotected branches, other CI systems, and local shells | Read-only; `write-only` becomes disabled |

Only the triggering event counts, not what the job does. A release job that
runs on a push to a protected branch keeps its configured mode, while a
scheduled or manually dispatched job on that same branch cannot publish. To keep
a trusted job from publishing, set `MBX_REMOTE_MODE=read-only` for it.

mbx reads the trigger from the CI system's own variables:

| CI system | Detected by | May write when |
| --- | --- | --- |
| GitHub Actions | `GITHUB_ACTIONS` | `GITHUB_EVENT_NAME` is `push`, `GITHUB_REF_TYPE` is `branch`, and `GITHUB_REF_PROTECTED` is true |
| GitLab CI | `GITLAB_CI` | `CI_PIPELINE_SOURCE` is `push`, `CI_COMMIT_REF_PROTECTED` is true, and neither `CI_COMMIT_TAG` nor `CI_MERGE_REQUEST_IID` is set |

This policy prevents untrusted code from publishing objects that later builds
would trust. The server should still authenticate and authorize requests; the
client-side policy is not an access-control boundary.

With a remote configured, `mbx doctor` shows the result on its `policy` line:

```text
  ok  policy       configured read-write, effective read-only
```

## GitLab CI

GitLab CI follows the same [write policy](#read-and-write-policy) as GitHub
Actions: a push pipeline on a protected branch may write; merge requests,
unprotected branches, and other pipeline sources such as schedules are
read-only; and tag pipelines cannot publish to the remote.

The OIDC flow is GitHub-specific, so authenticate GitLab jobs with a bearer
token in a masked, protected CI/CD variable:

```yaml
# install mbx and a Rust toolchain in the job image first
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

After a build has published the action manifest for its lockfile, another
machine can warm its own store from that manifest without running Cargo:

```sh
mbx prefetch build --workspace --release
```

As in a normal mbx build, the workspace's `Cargo.lock`, together with the
operating system and architecture, selects the manifest. The Cargo arguments
only locate the workspace: `build`, `test`, and `clippy` share one manifest
whatever their profile or features, so prefetch warms actions that any of them
recorded for that lockfile, not only the ones this command would compile.

Prefetch requires a configured remote in `read-only` or `read-write` mode,
waits until its downloads finish, and reports what it pulled:

```text
prefetched 161 actions; 214.8 MiB downloaded and 214.8 MiB stored locally
```

- **Nothing recorded.** If no manifest has been published for the workspace's
  lockfile, mbx prints `no recorded actions for this workspace and Cargo command`
  and exits successfully. There was nothing to fetch, which is normal for a
  first build.
- **New lockfile.** A lockfile that has not been built yet borrows the manifest
  of the most recent earlier lockfile in Git history that has one, looking back
  up to eight lockfile states, so a dependency bump still prefetches the
  unchanged part of the graph. A shallow checkout needs that history fetched. For GitHub Actions builds of the default pull-request merge
  commit, `fetch-depth: 2` includes its base parent; builds of a PR head or
  another ref may need more history.
- **Failed lookup.** A lookup that fails, such as an unreachable host or
  refused credentials, is an error, so CI can tell an empty cache from a broken
  one.
- **Where to run it.** A typical place for the command is a runner or
  devcontainer image's start-up hook, so the store is warm before anyone builds.

## In-flight deduplication

When a cache server advertises action promises, read-write runners atomically
claim a cold compiler invocation before starting it. One runner compiles and
publishes the result; other runners wait for its promise, rebuild the final
action key from the promised input prediction, verify every input, and restore
the published result. mbx fulfills the promise only after the action result
and all referenced blobs are remotely durable.

Claims are keyed by the pre-discovery invocation digest because a cold runner
does not yet know the compiler-discovered inputs in the final action key. They
are leases: a runner that dies or cannot publish leaves no durable cache record,
and the server expires its claim so another runner can compile. Any endpoint
error, unsupported server, read-only policy, or expired client wait degrades to
an ordinary compilation. Read-only and write-only runners never acquire claims.

## Deferred publication

Uploads are not on the critical path of the build that produced them. mbx
queues them while the build continues and drains the queue before the build
session exits. Where the server accepts them, mbx publishes queued blobs several
to a request. An action result is published only after every blob it
references, so a reader never fetches a result whose outputs it cannot restore.
A command killed before the queue drains may leave some results only in the
local store. Other machines must compile work that was not uploaded.

mbx reports a failed upload, counts it in `remote_failures`, and continues; the
build keeps its local result either way.

The default short and CI [build summaries](/configuration#build-summaries) show
only the total bytes uploaded, and the CI summary adds a line when background
uploads fail. The full summary (`MBX_SUMMARY=full`) also reports how many
uploads were published, how many of them went in packs, and how long the build
session waited for them after the build:

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

Batched lookups and packed uploads are negotiated. A server without them, or
one that advertises an endpoint it does not serve, gets the single-object
requests every version of mbx has made. Nothing needs configuring either way.

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
session-wide limit while retaining the per-download deadline. The budget
applies to cache servers. An `s3://` remote keeps the per-download deadline but
has no session-wide limit.
