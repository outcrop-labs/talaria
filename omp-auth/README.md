# talaria-omp-auth — the omp auth bridge

Oh My Pi's own OAuth flows, driven headlessly, behind a loopback HTTP API the
Rust api calls. It exists so that **Talaria can hold coding-account credentials
without reimplementing a single provider**.

## Why

Talaria keeps each agent's coding accounts sealed in Postgres and serves them to
that agent's omp over omp's auth-broker protocol. Being the broker makes Talaria
the canonical refresher, which means performing the flows: 23 providers' client
ids, PKCE quirks, device-code endpoints, token exchanges, userinfo enrichment and
per-provider refresh hooks. omp already knows all of it — declaratively, in
`@oh-my-pi/pi-catalog`'s compiled auth rules — and publishes it on npm several
times a week. So nothing here mirrors a provider parameter. Upstream's engine
runs verbatim; we keep the credential it returns.

## The bundle, and the one patch-shaped thing in it

`bun run build` produces a single self-contained `dist/server.js` (~15 MB) that
the api spawns with no `node_modules` beside it.

Reaching omp's OAuth code pulls `@oh-my-pi/pi-natives`, whose platform package
is **364 MB of prebuilt binaries with no musl build at all** — and Talaria's app
image is alpine. So the bundler substitutes `src/natives-stub.ts` for it. Read
that file before touching it: the native pieces in the graph are a
custom-URL-scheme callback receiver, a Windows long-path helper, advisory file
locks, a native process manager, a mermaid renderer and Apple Foundation
Models. None participates in an authorization-code, device-code or paste-code
flow on Linux, which is every flow this bridge runs. Every stub either answers
honestly or **throws with its own symbol in the message**, so a future omp
release that routes a login through one fails loudly instead of misbehaving
quietly.

## Flow shapes

All three of omp's shapes work without a terminal or a browser on this host:

- **authorization code + loopback callback** (anthropic, openai-codex,
  google-gemini-cli, …) — we surface the authorize URL. When the person's
  browser can reach this host's loopback (a dev stack on the same machine) the
  callback completes it; when it cannot, every one of these providers declares a
  paste-code fallback and we take the pasted code or redirect URL. That
  fallback is the provider's own instruction text, not our invention.
- **device code** (xai-oauth, muse-code, kimi-code, factory-droid) — a URL and a
  user code; upstream polls. Nothing to paste.
- **custom** (github-copilot, cursor, perplexity, alibaba, …) — prompts and/or
  polling through the same controller callbacks.

## Surface

Loopback-bound and bearer-gated (`OMP_AUTH_BRIDGE_PORT`,
`OMP_AUTH_BRIDGE_TOKEN`). Credentials are never written to disk — no omp agent
dir, no sqlite, no token file — and a finished login's credential is served
**once**, to the api that seals it.

| Route | What it does |
|---|---|
| `GET /healthz` | liveness, unauthenticated |
| `GET /v1/providers` | the OAuth roster: id, name, storeAs, flow, pasteCode |
| `GET /v1/models?provider=` | that provider's models, from omp's bundled catalog |
| `POST /v1/login` | start a flow → the session |
| `GET /v1/login/{id}` | poll: url, instructions, pending prompt, credential |
| `POST /v1/login/{id}/input` | answer the pending prompt or paste the code |
| `DELETE /v1/login/{id}` | cancel |
| `POST /v1/refresh` | refresh one stored oauth credential |

## Verifying it after an upstream bump

```bash
bun run build
bun scripts/probe-flows.ts        # every provider on the roster, one login start each
```

The probe starts a flow for **all 23** providers, observes each until it is
waiting on a person, cancels it, and prints one line per provider with an N/N
count. It is network-bound (device-code flows really do register with the
provider) so it is a manual gate, not a CI job. Last run: 23/23.
