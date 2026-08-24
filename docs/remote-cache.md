# Remote cache

A remote cache lets ephemeral runners and teammates restore only the rustc
actions a build needs. The local content-addressed store remains the working
cache; remote objects are downloaded into it and newly completed actions may be
uploaded from trusted CI.

## Configure a server

```toml
[remote]
url = "https://cache.example.com"
namespace = "acme/backend"
mode = "read-write"
```

The namespace isolates one project's cache from another. It is required when a
remote URL is set.

## Authenticate

Use one of:

- `MBX_REMOTE_TOKEN` for a bearer token.
- `MBX_REMOTE_TOKEN_FILE` for a file containing the token.
- `MBX_REMOTE_OIDC_AUDIENCE` for CI-issued OIDC credentials.

Avoid long-lived secrets in pull request workflows. On GitHub Actions, OIDC
requires `id-token: write` permission.

## Read and write policy

Configured mode is constrained by the environment:

| Context | Effective behavior |
| --- | --- |
| Protected branch push on GitHub Actions or GitLab CI | Configured mode |
| Pull request, merge request, local shell, or unprotected branch | Read-only; `write-only` becomes disabled |
| Tag or release build | Remote cache disabled |

This policy prevents untrusted code from publishing objects that later builds
would trust. The server should still authenticate and authorize requests; the
client-side policy is defense in depth, not an access-control boundary.

## Transfer behavior

Remote blobs are compressed with zstd. `MBX_HTTP_DOWNLOAD_TIMEOUT` is separate
from the normal request timeout because artifacts can be much larger than
metadata responses. Failed requests are retried according to
`MBX_HTTP_RETRIES`.
