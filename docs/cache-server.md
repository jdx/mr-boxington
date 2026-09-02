# Cache server

[`jdx/mr-boxington-cache`](https://github.com/jdx/mr-boxington-cache) is the
self-hostable remote cache server. It implements version 1 of the mbx
action-cache protocol: immutable blobs, atomic action-result commits,
namespace isolation, and streaming blob packs. It also serves
[mise](https://mise.jdx.dev)'s task cache. Any server implementing the
[protocol](/protocol-compatibility) works with mbx; this page documents the
reference implementation.

## Run it

The repository's development stack starts the service, PostgreSQL, and MinIO:

```sh
docker compose up --build
```

For a standalone instance with filesystem storage:

```sh
cargo install mbx-cache --locked
mbx-cache \
  --allow-anonymous \
  --data-dir ./data \
  --listen 127.0.0.1:8080
```

Anonymous access is intended only for a trusted local network. Production
installations should terminate TLS at an ingress or proxy and configure
tokens. The repository ships a Helm chart for horizontally scaled Kubernetes
deployments and a Terraform-managed single-host example.

Clients connect as described in [remote cache](/remote-cache): a `[remote]`
URL, a required namespace, and a token or OIDC audience.

## Configuration

Every option has a matching environment variable and CLI flag; run
`mbx-cache --help` for the complete list.

| Environment variable | Default | Purpose |
| --- | ---: | --- |
| `MBX_CACHE_LISTEN` | `0.0.0.0:8080` | Listen address |
| `MBX_CACHE_STORAGE` | `filesystem` | `filesystem` or `s3` |
| `MBX_CACHE_DATA_DIR` | `/var/lib/mbx-cache` | Filesystem blob root |
| `MBX_CACHE_DATABASE_URL` | `memory://` | PostgreSQL URL or development memory store |
| `MBX_CACHE_S3_BUCKET` | none | Required for S3 storage |
| `MBX_CACHE_S3_PREFIX` | `v1` | Object-key prefix |
| `MBX_CACHE_S3_ENDPOINT` | AWS default | S3-compatible endpoint |
| `MBX_CACHE_S3_REGION` | `us-east-1` | S3 region |
| `MBX_CACHE_S3_PATH_STYLE` | `false` | Enable for MinIO and similar services |
| `MBX_CACHE_TOKENS_JSON` | none | Static token grants |
| `MBX_CACHE_OIDC_PROVIDERS_JSON` | none | Trusted OIDC providers and claim grants |
| `MBX_CACHE_ALLOW_ANONYMOUS` | `false` | Allow access without configured grants |
| `MBX_CACHE_MAX_BLOB_BYTES` | `5368709120` | Maximum upload size |

AWS credentials use the standard SDK credential chain, including environment
variables, workload identity, ECS, and EC2 roles.

## Authorization

Authorization is deny-by-default. Namespace patterns may be an exact name,
`*`, or a prefix ending in `/*`.

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
Configure trusted issuers, acceptable audiences, and claim-based grants:

```json
[
  {
    "issuer": "https://token.actions.githubusercontent.com",
    "audiences": ["https://cache.example.com"],
    "rules": [
      {
        "claims": {
          "repository": "acme/backend",
          "repository_owner_id": "12345"
        },
        "read": ["acme/backend"],
        "write": ["acme/backend"]
      }
    ]
  }
]
```

The server discovers the issuer's JWKS endpoint and verifies the signature,
issuer, audience, expiry, not-before time, and subject before applying the
first matching rule. Rules are alternatives; every claim within a rule must
match exactly. Pin stable identity claims such as GitHub's numeric
`repository_owner_id` alongside the repository name, since names can be
reclaimed and the ID cannot. Add `ref` or `environment` claims when only a
narrower workflow identity should write. Symmetric JWT algorithms are never
accepted.

On the client side, mbx acquires the GitHub Actions job token itself: set
`MBX_REMOTE_OIDC_AUDIENCE`, or use
[`jdx/mr-boxington-action`](https://github.com/jdx/mr-boxington-action) with
`backend: server` as shown in [GitHub Action](/github-action#cache-server).

## Operations

Blob storage is expired by the bucket's own lifecycle rule; the server removes
the metadata those objects leave behind and exits without serving:

```sh
mbx-cache --sweep-metadata-older-than-days 35
```

Keep the sweep age longer than the storage lifecycle so objects expire first.
A dangling reference is never fatal: a client that cannot fetch a blob treats
the action as a miss and recompiles.

Multiple stateless replicas can run against the same PostgreSQL database and
S3 bucket. Readiness and liveness probes use `/v1/status`, and `/metrics`
exposes Prometheus counters for actions, blobs, and blob-pack transfers with
fixed, low-cardinality labels. Namespaces, tokens, and digests never appear as
labels. The server has no deletion endpoint. Retention and disaster recovery
are administrative concerns: back up PostgreSQL, and use bucket versioning or
replication as required.

A server advertising action promises can coordinate identical cold
compilations across replicas. Claims are short-lived database leases scoped by
namespace and invocation digest; completion is accepted only after the named
action result is durable. Abandoned claims expire, completed promises are
immutable for their retention window, and no promise endpoint bypasses the
namespace's write authorization.

## Protocol

The wire protocol (endpoints, media types, canonical hashing, and the
evolution rules) is documented in
[protocol compatibility](/protocol-compatibility). The
[repository README](https://github.com/jdx/mr-boxington-cache) covers
implementation details beyond it, including blob-pack framing and validation
behavior.
