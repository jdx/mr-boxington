---
description: Run the reference mbx cache server with filesystem, S3, or Azure Blob storage, namespace grants, and CI authentication.
---
# Cache server

[`jdx/mr-boxington-cache`](https://github.com/jdx/mr-boxington-cache) is the
self-hostable remote cache server. It implements version 1 of the mbx
action-cache protocol: immutable blobs, atomic action-result commits,
namespace isolation, and streaming blob packs. It also serves
[mise](https://mise.jdx.dev)'s task cache. Any server implementing the
[protocol](/protocol-compatibility) works with mbx; this page documents the
reference implementation.

## Run it

The server repository includes a Docker Compose development stack that runs
the service, PostgreSQL, and MinIO. Clone the repository and start it:

```sh
git clone https://github.com/jdx/mr-boxington-cache.git
cd mr-boxington-cache
docker compose up --build
```

The stack listens on port 8080 and accepts the token `development-token` for
the `default` namespace only. To connect mbx to it, use
`namespace = "default"` in the client configuration that follows and export
`MBX_REMOTE_TOKEN=development-token`. Change the token before you expose the
service.

For a standalone instance with filesystem storage, install the server from its
repository and run it:

```sh
cargo install --locked --git https://github.com/jdx/mr-boxington-cache mbx-cache
mbx-cache \
  --allow-anonymous \
  --data-dir ./data \
  --listen 127.0.0.1:8080
```

The `mbx-cache` crate on crates.io is a reserved placeholder with no binary.
Install from the repository as shown, run `cargo install --locked --path .` in
your clone of the repository, or use the `ghcr.io/jdx/mbx-cache:main` container
image.

The standalone example is for local evaluation. It binds to loopback, allows
anonymous access, and keeps metadata in memory because it sets no database
URL. Restarting the process therefore empties the cache, even though `./data`
keeps the files. Point `--database-url` at PostgreSQL for an instance that
keeps its cache across restarts.

Production installations should terminate TLS at an ingress or proxy and
configure tokens. The repository ships a Helm chart for horizontally scaled
Kubernetes deployments and a Terraform-managed single-host example on Azure.

Connect mbx to the standalone example with this
[global configuration](/configuration):

```toml
[remote]
url = "http://127.0.0.1:8080"
namespace = "example/project"
mode = "read-only"
```

Run `mbx doctor` to check that the server is reachable and compatible. The
reference server answers that check without authorization, so `mbx doctor`
passes even with a token or namespace the server will refuse.

A local shell is read-only even when `mode` is `read-write`; use
[trusted CI](/remote-cache#read-and-write-policy) to populate the remote
through mbx. For an authenticated deployment, use HTTPS and add a token or OIDC
audience as described in [Authenticate](/remote-cache#authenticate). mbx
refuses a plain-HTTP URL when a token or OIDC audience is set, unless the host
is a loopback address.

## Configuration

Every option has a matching environment variable and CLI flag; run
`mbx-cache --help` for the complete list.

| Environment variable | Default | Purpose |
| --- | ---: | --- |
| `MBX_CACHE_LISTEN` | `0.0.0.0:8080` | Listen address |
| `MBX_CACHE_STORAGE` | `filesystem` | `filesystem`, `s3`, or `azure` |
| `MBX_CACHE_DATA_DIR` | `/var/lib/mbx-cache` | Filesystem blob root |
| `MBX_CACHE_DATABASE_URL` | `memory://` | PostgreSQL URL or development memory store |
| `MBX_CACHE_DATABASE_MAX_CONNECTIONS` | `32` | Maximum concurrent PostgreSQL metadata connections |
| `MBX_CACHE_AZURE_ACCOUNT` | none | Storage account name; required for Azure storage |
| `MBX_CACHE_AZURE_CONTAINER` | none | Blob container name; required for Azure storage |
| `MBX_CACHE_AZURE_PREFIX` | `v1` | Azure object-key prefix |
| `MBX_CACHE_AZURE_CREDENTIAL_TYPE` | `auto` | Azure credential discovery mode |
| `MBX_CACHE_AZURE_ENDPOINT` | Azure default | Override for compatible emulators |
| `MBX_CACHE_AZURE_ALLOW_HTTP` | `false` | Allow an HTTP emulator endpoint |
| `MBX_CACHE_S3_BUCKET` | none | Required for S3 storage |
| `MBX_CACHE_S3_PREFIX` | `v1` | S3 object-key prefix |
| `MBX_CACHE_S3_ENDPOINT` | AWS default | S3-compatible endpoint |
| `MBX_CACHE_S3_REGION` | `us-east-1` | S3 region |
| `MBX_CACHE_S3_PATH_STYLE` | `false` | Enable for MinIO and similar services |
| `MBX_CACHE_TOKENS_JSON` | none | Static token grants |
| `MBX_CACHE_OIDC_PROVIDERS_JSON` | none | Trusted OIDC providers and claim grants |
| `MBX_CACHE_ALLOW_ANONYMOUS` | `false` | Allow anonymous reads and writes; ignored once tokens or OIDC providers are configured |
| `MBX_CACHE_ANONYMOUS_READ_NAMESPACES_JSON` | none | Namespace patterns anyone may read; writes still need a grant |
| `MBX_CACHE_MAX_BLOB_BYTES` | `5368709120` (5 GiB) | Maximum upload size |

AWS credentials use the standard SDK credential chain, including environment
variables, workload identity, ECS, and EC2 roles. For Azure, the default `auto`
credential type discovers credentials. On Azure VMs, set
`MBX_CACHE_AZURE_CREDENTIAL_TYPE` to `managed_identity` so the service
environment holds no storage account key.

## Authorization

Authorization is deny-by-default. Namespace patterns may be an exact name,
`*`, or a prefix ending in `/*`.

To serve a deliberately public cache, set
`MBX_CACHE_ANONYMOUS_READ_NAMESPACES_JSON` to a JSON array of patterns such as
`["acme/public/*"]`. Requests without a token may read matching namespaces,
and every write still needs a token or OIDC grant. Unlike
`MBX_CACHE_ALLOW_ANONYMOUS`, this setting works alongside tokens and OIDC
providers.

### Static tokens

`MBX_CACHE_TOKENS_JSON` is an array of grants:

```json
[
  {
    "token": "replace-with-a-secret",
    "read": ["acme/*", "public"],
    "write": ["acme/project-a"]
  }
]
```

Rotate tokens by deploying the old and new grants together, moving clients to
the new token, then removing the old grant. Inject the JSON through a secret,
not through deployment configuration.

### OIDC

OIDC lets CI use short-lived identity tokens instead of stored secrets.
Configure trusted issuers, acceptable audiences, and claim-based grants in
`MBX_CACHE_OIDC_PROVIDERS_JSON`. This example grants access only to pushes on
`acme/backend`'s `main` branch. Replace the numeric owner ID and add separate
read-only rules for any other identities that need access:

```json
[
  {
    "issuer": "https://token.actions.githubusercontent.com",
    "audiences": ["https://cache.example.com"],
    "rules": [
      {
        "claims": {
          "repository": "acme/backend",
          "repository_owner_id": "12345",
          "ref": "refs/heads/main",
          "event_name": "push"
        },
        "read": ["acme/backend"],
        "write": ["acme/backend"]
      }
    ]
  }
]
```

The server discovers the issuer's JSON Web Key Set (JWKS) endpoint and verifies
the signature, issuer, audience, expiry, not-before time, and subject. It then
allows the request when any rule grants the namespace for the requested access
and every claim in that rule matches the token exactly. Rule order does not
matter: a broad read-only rule does not hide a narrower write rule listed after
it.

Pin stable identity claims such as GitHub's numeric `repository_owner_id`
alongside the repository name, since names can be reclaimed and the ID cannot.
A write rule should pin `event_name: "push"` and an exact protected-branch
`ref`, as the example does, and can add `ref_type: "branch"`. Pinning `ref`
alone is unsafe because events such as `pull_request_target` carry the base
branch's ref. Add `workflow_ref`, `job_workflow_ref`, or `environment` claims
when only a narrower workflow identity should write. The server never accepts
symmetric JWT algorithms.

On the client side, mbx acquires the GitHub Actions job token itself: set
`MBX_REMOTE_OIDC_AUDIENCE`, or use
[`jdx/mr-boxington-action`](https://github.com/jdx/mr-boxington-action) with
`backend: remote` as shown in [GitHub Action](/github-action#cache-server).
The audience mbx requests must be one of the provider's `audiences`, or the
server rejects the token. With the example provider, set
`MBX_REMOTE_OIDC_AUDIENCE` or the action's `oidc-audience` input to
`https://cache.example.com`.

## Operations

For S3 or Azure Blob storage, expire blobs with an S3 bucket lifecycle rule or
an Azure Blob lifecycle management rule. Run a separate metadata sweep to
remove old records. The sweep exits after
cleanup without starting the server. Run it with the server's
`MBX_CACHE_DATABASE_URL`; against the default `memory://`, it finds nothing to
remove.

```sh
MBX_CACHE_DATABASE_URL=postgres://... mbx-cache --sweep-metadata-older-than-days 35
```

Keep the sweep age longer than the storage lifecycle so objects expire first.
Otherwise the sweep drops records for objects that still exist, and clients
recompile actions that storage could still have served. A dangling reference is
never fatal: a client that cannot fetch a blob treats the action as a miss and
recompiles.

Multiple stateless replicas can run against the same PostgreSQL database and
S3 bucket or Azure Blob container. Readiness and liveness probes use
`/v1/status`, and `/metrics` exposes Prometheus counters for actions, blobs,
and blob-pack transfers with fixed, low-cardinality labels. Namespaces, tokens,
and digests never appear as labels.

The server has no deletion endpoint, so retention and disaster recovery are
administrative concerns. Back up PostgreSQL, and configure storage recovery
features as required: S3 versioning or replication, or Azure Blob versioning
and storage redundancy.

The reference server does not implement action promises. It does not advertise
`features.action_promises`, so runners that miss the same action at the same
time each compile it.
[In-flight deduplication](/remote-cache#in-flight-deduplication) needs a server
that implements the promise endpoints described in
[Protocol compatibility](/protocol-compatibility#remote-cache-protocol).

## Protocol

The wire protocol (endpoints, media types, canonical hashing, and the
evolution rules) is documented in
[protocol compatibility](/protocol-compatibility). The
[repository README](https://github.com/jdx/mr-boxington-cache) covers
implementation details beyond it, including blob-pack framing and validation
behavior.
