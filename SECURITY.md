# Security policy

## Reporting a vulnerability

Please report vulnerabilities privately. Use GitHub private vulnerability reporting
(Security Advisories) at <https://github.com/avbel/yrem/security/advisories/new>.
Do not open a public issue for security problems.

## Supported versions

Only the latest 0.x release receives security fixes.

## Scope notes

- The token is sent as a sensitive header. It is never written to logs or `Debug` output, and `doctor` redacts it.
- Recalled memory text is untrusted data, not instructions. A memory may contain text written by an earlier session or another client.
- The yantrikdb server has no TLS of its own. Outside a trusted LAN, put it behind a TLS proxy or VPN and use an `https://` URL; the default build and release binaries support TLS (`--no-default-features` builds a plain-HTTP binary).
