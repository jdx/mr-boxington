# Remote cache

A remote cache lets ephemeral runners and teammates restore only the rustc
actions a build needs. The local content-addressed store remains the working
cache; remote objects are downloaded into it and newly completed actions may be
uploaded from trusted CI.

mbx reaches a remote in one of two ways. A **cache server** speaks the mbx
protocol and answers with the extensions built on top of it. An
**S3-compatible bucket** stores the same objects with nothing to run. The URL's
scheme chooses; everything else on this page applies to both.

## Configure a server

```toml
[remote]
url = "https://cache.example.com"
namespace = "acme/backend"
mode = "read-write"
```

The namespace isolates one project's cache from another. It is required when a
remote URL is set. To host your own server, see [cache server](/cache-server).

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

Anything that mints temporary credentials must export them there first. On
GitHub Actions that is
[`aws-actions/configure-aws-credentials`](https://github.com/aws-actions/configure-aws-credentials),
which exchanges the runner's OIDC token for a role, so no long-lived secret has
to exist. Credential sources that are not environment variables — EKS IRSA,
EC2 and ECS instance roles, `~/.aws/config` profiles, SSO — are not read
directly; export the variables and they all work.

The URL may carry a prefix, as `s3://acme-build-cache/teams/backend`, to share
one bucket between projects. Keys are laid out under
`<prefix>/<namespace>/v1/`, so a bucket policy can scope a writer to its own
prefix.

An IAM policy needs `s3:GetObject` and `s3:PutObject` on that prefix. Add
`s3:ListBucket` on the bucket if you can: mbx never lists anything, but AWS
answers `403` rather than `404` for an object that is not there unless the
caller holds it, and that permission is what lets a miss be told apart from a
refusal.

Without it mbx still works — a refused read is treated as a miss, and it says
so once — but a credential that genuinely cannot read the cache then looks the
same as a cold one, and only the warning distinguishes them. Credentials that
S3 itself rejects, such as a wrong secret or an expired token, are always
reported as errors either way.

### Cloudflare R2 and MinIO

Set an endpoint, which switches to addressing the bucket in the path:

```toml
[remote]
url = "s3://acme-build-cache"
namespace = "acme/backend"
s3_endpoint = "https://<account>.r2.cloudflarestorage.com"
s3_region = "auto"
```

MinIO is the same with its own endpoint. `http://` is refused for anything but
a loopback address, since a signature and the objects it fetches are readable
in transit without TLS.

### What a bucket does not do

A cache server verifies every blob against the digest in its URL before storing
it. A bucket stores what it is given, so a corrupted object in the local store
can be published under a key naming different content, and because writes are
create-only nothing later overwrites it: every machine that downloads it fails
verification and recompiles. `mbx cache verify` finds such an object locally.

A bucket also does not coordinate in-flight compilations, answer batched
lookups, stream blob packs, or negotiate compression. mbx asks for none of
them against S3 and falls back
to per-object requests, which is what every version of the protocol has done
against a server without the extensions. Expect more requests for the same
build, and no compression on the wire.

The one record mbx updates in place — the task action manifest that drives
[prefetch](#prefetch) — needs conditional writes. AWS S3, R2, and current MinIO
all implement them. Against a store that does not, mbx says so once and
continues without: blobs and action results are content-addressed, so writing
one twice is harmless, but concurrent manifest updates can then lose each
other's predictions, which costs prefetch coverage on later builds and nothing
else. `MBX_REMOTE_S3_CONDITIONAL_WRITES=required` refuses such a store instead.

### Who may publish

A cache server authenticates and authorizes every request. A bucket does not:
whatever the credentials may write, mbx may write. The client-side [write
policy](#read-and-write-policy) still applies — pull requests never publish —
but with a bucket that policy is the only thing between an untrusted build and
your cache unless IAM agrees.

Scope the write credential to the builds you trust. On GitHub Actions, restrict
the role's trust policy to the branches that may assume it, and give pull
request jobs a read-only role.

## Authenticate

For a cache server, use one of:

- `MBX_REMOTE_TOKEN` for a bearer token.
- `MBX_REMOTE_TOKEN_FILE` for a file containing the token.
- `MBX_REMOTE_OIDC_AUDIENCE` for CI-issued OIDC credentials.

Avoid long-lived secrets in pull request workflows. On GitHub Actions, OIDC
requires `id-token: write` permission.

These authenticate to a server and are refused alongside an `s3://` URL, which
authenticates with AWS credentials instead.

## Read and write policy

Configured mode is constrained by the environment:

| Context | Effective behavior |
| --- | --- |
| Protected branch push on GitHub Actions or GitLab CI | Configured mode |
| Pull request, merge request, local shell, or unprotected branch | Read-only; `write-only` becomes disabled |
| Tag or release build | Read-only; `write-only` becomes disabled |

This policy prevents untrusted code from publishing objects that later builds
would trust. The server should still authenticate and authorize requests; the
client-side policy is defense in depth, not an access-control boundary.

### In-flight deduplication

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

## GitLab CI

The write policy recognizes GitLab CI with the same shape as GitHub Actions: a
push pipeline on a protected branch may write, merge requests and unprotected
branches are read-only, and tag pipelines cannot publish to the remote.

The OIDC flow is GitHub-specific, so authenticate GitLab jobs with a bearer
token in a masked, protected CI/CD variable:

```yaml
build:
  variables:
    MBX_REMOTE_URL: https://cache.example.com
    MBX_REMOTE_NAMESPACE: acme/backend
    MBX_REMOTE_MODE: read-write
  script:
    - mbx build --workspace
```

Set `MBX_REMOTE_TOKEN` in the project's CI/CD variables (masked, and limited
to protected branches) rather than in the YAML.

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

A workspace and command nobody has published a manifest for print
`no recorded actions for this workspace and Cargo command` and exit
successfully — there was nothing to fetch, which is normal for a first build.
A lookup that *fails* — unreachable host, refused credentials — is an error,
so CI can tell an empty cache from a broken one. A typical place for the
command is a runner or devcontainer image's start-up hook, so the store is
warm before anyone builds.

## Deferred publication

Uploads are not on the critical path of the build that produced them: they are
queued while the build continues and drained before the session exits, and an
action result is published only after every blob it references, so a reader
never fetches a result whose outputs it cannot restore. A command killed part
way through publishes less than it stored, which costs only hit rate — the
next build recomputes what never landed.

A failed upload is reported, counted in `remote_failures`, and recovered from;
the build keeps its local result either way. The session summary reports what
was published and what the drain cost, and where the server accepts them,
queued blobs are published several to a request rather than one at a time:

```text
mbx[cache]: uploads: 143 published (118 of them in 2 packs), 0 not published; 412.0ms waited for after the build
```

`MBX_STATS_REPORT` carries the same figures as `background_uploads`,
`background_upload_failures`, `upload_drain_duration_ns`,
`remote_blob_pack_uploads`, and `remote_blob_pack_upload_blobs`.

## Batched lookups

A prefetch knows every action it wants before it asks for any of them, so where
the server offers batched lookups it asks for them together instead of once per
action. `remote_action_lookups` counts requests rather than actions, so the same
build reports far fewer of them against a server with the extension.

Both extensions are negotiated. A server without them, or one that advertises an
endpoint it does not serve, gets the single-object requests every version of mbx
has made; nothing needs configuring either way.

## Transfer behavior

Remote blobs are compressed with zstd. Failed requests are retried according
to `MBX_HTTP_RETRIES`, and a stalled attempt is cut short by
`MBX_HTTP_TIMEOUT`.

`MBX_HTTP_DOWNLOAD_TIMEOUT` exists separately because artifacts can be much
larger than metadata responses. It is a deadline for the whole download, not a
budget per attempt — it spans every retry and the backoff between them, which
bounds how long one blob can hold a build open. A packed request scales the
deadline up with the bytes and object count it asks for, since the configured
value describes a single blob. Raise it if a slow link makes large artifacts
run out of time before their retries are spent.
