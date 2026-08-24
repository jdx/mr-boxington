# Security policy

## Supported versions

Security fixes are made on the latest released version and on the `main`
branch. Please upgrade to the newest release before reporting an issue that may
already have been fixed.

## Report a vulnerability

Please use [GitHub's private vulnerability reporting][report] rather than a
public issue. Include the affected mbx version, operating system, reproduction
steps, impact, and any relevant cache or remote-server configuration. Remove
bearer tokens, OIDC credentials, and private source paths from logs and
attachments.

You should receive an acknowledgement within seven days. We will coordinate
validation, remediation, and disclosure with you. Please do not publish the
report before a fix or mitigation is available.

[report]: https://github.com/jdx/mr-boxington/security/advisories/new

## Trust model

mbx restores compiler outputs and some of those outputs may later be linked or
executed. Treat anyone who can write to a cache as having the same trust as
someone who can contribute build artifacts:

- Grant remote-cache write access only to trusted CI and maintainers. Pull
  requests, merge requests, local shells, and unprotected branches are forced
  read-only by the client, but the server must still enforce authentication and
  authorization.
- A remote namespace prevents accidental key collisions; it is not an access
  control boundary. Use HTTPS, narrowly scoped credentials, and separate
  server-side authorization where projects have different trust domains.
- Remote cache storage and transport are treated as untrusted. mbx validates
  content digests, result metadata, and materialization paths before accepting
  data. A digest match proves integrity relative to the action record, not that
  the writer was trustworthy.
- The local cache is trusted at the level of the operating-system user who owns
  it. Do not share a writable local cache between mutually untrusted users.
- `MBX_VERIFY=1` recompiles while consulting the cache and compares results. It
  is useful when qualifying a server or investigating suspected cache
  corruption, but it does not replace access control.

Bearer tokens can be supplied directly or through a token file; OIDC is
recommended for CI where available. Avoid long-lived credentials in pull
request workflows, restrict token-file permissions, and never commit tokens to
the repository.

## In scope

Reports are especially valuable for cache poisoning, action-key confusion,
digest-validation bypasses, path traversal or unsafe materialization, credential
disclosure, privilege-boundary violations, and parsing vulnerabilities in data
received from a remote cache. Incorrect cache hits that silently alter build
outputs are also considered security-relevant.

Denial of service that requires control of the same local user account, and
vulnerabilities in an unsupported version that do not reproduce on the latest
release, are generally out of scope.
