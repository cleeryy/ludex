# Security policy

## Supported versions

| Version | Supported |
|---|---|
| `0.2.x` (current release line) | Yes |
| `0.1.x` | Best effort |
| `edge` prereleases | No; rolling test builds |

Security fixes land on `main` and in the latest stable release. The `edge`
channel is for the homelab and may contain unreleased code.

## Reporting a vulnerability

**Do not open a public issue, discussion or pull request for a vulnerability.**

Use GitHub's private vulnerability reporting:

<https://github.com/cleeryy/ludex/security/advisories/new>

Include, when possible:

- Affected version/commit and deployment mode (Docker, binary, desktop local
  mode).
- A description of the impact and the attack path.
- Minimal reproduction steps or a proof of concept.
- Whether the issue is already public, and any suggested fix.

Do not include real credentials, personal data, copyrighted game files or third
party systems in the report. Use a throwaway instance.

## What to expect

- Acknowledgement within 3 business days.
- An initial assessment and severity/impact within 7 days.
- A coordinated disclosure date after a fix is available. Credit is given unless
  you ask to stay anonymous.
- No bug bounty is offered.

## Scope

In scope:

- `ludex-server`: authentication/authorization, path traversal, archive
  extraction, ingest URL validation/SSRF, SQLite handling, download tickets,
  range streaming.
- `apps/web`: XSS, token handling, unsafe URL handling.
- `apps/desktop`: local server lifecycle, install path handling, process launch,
  IPC commands.
- Packaging and deployment: `docker/`, `compose.yaml`, `scripts/`,
  `deploy/gitops/`, release workflows.

Out of scope:

- Vulnerabilities in third-party games, game archives or the user's reverse
  proxy.
- Attacks that require an already-compromised admin account, root on the host,
  or physical access.
- Missing hardening headers with no demonstrated impact.
- Social engineering and spam.
- DRM circumvention or piracy features; Ludex does not provide them.

## Operational guidance

- Run Ludex behind HTTPS and never expose the data directory.
- Keep the image/binary pinned and updated deliberately.
- Use one account per person; reserve `admin` for operators.
- Back up `/data`; it contains the SQLite database and JWT secret.
- The GitOps updater verifies a SHA-256 published in the same GitHub release over
  TLS and rolls back on a failed health check. Report any integrity gap in that
  path as a vulnerability.
