# Security

## Report a vulnerability

GitHub's private vulnerability reporting is the channel — no email to send, and no public
issue to open. On `outcrop-labs/talaria`: **Security** → **Advisories** →
[**Report a vulnerability**](https://github.com/outcrop-labs/talaria/security/advisories/new).
The advisory stays private until a fix is released.

If that link 404s for you, the repository's owners have not turned the feature on yet
(Settings → Code security → Private vulnerability reporting): say so in an issue that names
nothing sensitive, and do not describe the vulnerability there — the channel is being fixed,
not bypassed.

Do not file a vulnerability as a public issue. A public report is the one channel that makes
a vulnerability worse before it is fixed.

## What to include

- **The affected component** — the surface, route, or file.
- **The version or commit you tested** — the app shows it under Admin → Security → Updates,
  and `/api/healthz` carries it as `version` (null when running from source).
- **Reproduction** — the shortest steps that reach the flaw.
- **Impact** — what an attacker gets: which data, whose permissions, what survives a
  restart.
- **Whether a secret was exposed** — say so plainly if a credential, key, or token was
  readable, and rotate it before anything else.

## In scope

- Authentication and session handling.
- The sealed-secret store and its encryption ([`docs/ENCRYPTION.md`](./docs/ENCRYPTION.md)).
- Agent credential and sandbox boundaries, including the per-agent `tak_` keys
  ([`docs/AGENT-KEY-MIGRATION.md`](./docs/AGENT-KEY-MIGRATION.md)).
- The MCP gateway.
- The app's permission checks.
- The container and deploy surface this repo ships.

## Out of scope

- Findings that are your own instance's misconfiguration — an exposed port, a
  `TALARIA_SECRET_KEY` that leaked with its backups, a deployment wider than it needed to
  be.
- Third-party sidecars' own bugs. Those go upstream, to their maintainers.
- Model or provider behaviour: a prompt that gets a model to say something, with no Talaria
  boundary crossed.

## Process

1. You report through private vulnerability reporting.
2. A maintainer acknowledges it once they have read it.
3. We work the fix with you, ship it in a release, and publish the advisory alongside.

Reporters are credited in the advisory unless they ask not to be. No response time is
promised.