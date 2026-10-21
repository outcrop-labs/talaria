# The MCP end-to-end suite

`ui/e2e/` is a Playwright suite that drives the MCP surface the way an admin does: the
catalog, installing from the marketplace, credentials, OAuth, an agent's MCP tab, the
gateway, and the custom-server form. Nothing in it is mocked inside Talaria. The api, the
built ui and the browser are real. The servers on the other end are stubs, so a run does
not depend on anyone's real MCP server or on how the official registry happens to be
behaving. The behaviour it holds the product to is [MCP.md](./MCP.md).

## What it proves

The tests run in order, in one browser, and each one names its own servers. They are not
*serial* in Playwright's sense: a failed chapter does not skip the ones after it. Playwright
answers a failure with a fresh worker, so a chapter that builds on an earlier one (5 assigns
the open server, 6 calls it as chapter 5's agent) asks for what it needs: it reuses what an
earlier chapter made, or registers the server by URL and hires the agent itself. When the
run ends, `afterAll` removes every server, agent and placeholder LLM endpoint it created, so
a rerun starts from the same registry.

| # | Chapter | It passes when |
|---|---|---|
| 1 | Catalog | `POST /api/mcp/library/sync` mirrors the stub registry and the catalog reaches `phase: ready`. Search with `includeDead=1` finds all five stub entries, and the superseded version of `e2e/open` is absent (`version=latest`). The three live servers are never judged dead; if the liveness pass has already marked `e2e/dead` dead, the default search hides it. The marketplace shows the four live entries by title. `e2e/dead` is not checked there, because whether it shows depends on when the liveness pass ran |
| 2 | Open install | one click in the marketplace registers **E2E Open**; discovery runs; the card says *ready* and shows its 3 tools |
| 3 | Credentials | **E2E Key** answers 401, so the credentials editor opens by itself; once `X-Api-Key` is set, the card is *ready* with 3 tools, and the key's value never appears in an api answer |
| 4 | OAuth | **E2E OAuth** is detected as OAuth; *Connect org account* opens the popup, which goes through the stub's `/authorize` (302, no login page) back to Talaria's callback (PKCE verified); the card says *connected* and lists its tools |
| 5 | Agent tab | an agent hired through the api (no container) gets **E2E Open** from its MCP tab, narrowed to `echo`; the registry card counts *1 agent*; switching the toggle off removes it again. The tab sends exactly what the UI builds plus `apply: false`, which drops the container roll (see below) |
| 6 | Gateway | as that agent, `POST /api/mcp/gw/e2e-open`: `tools/list` returns `echo` only, `tools/call echo` echoes, `tools/call shout` is refused with `-32602`, the stub's call log shows the echo and no shout, and a caller with no credential gets 401 |
| 7 | Custom form | a raw URL registered through *Custom server* is discovered and its card is *ready* |

To call the gateway, the agent presents its own credential, the
`TALARIA_AGENT_KEY_<SLUG>` the api renders into the fleet env (`E2E_FLEET_ENV`, default
`<repo>/fleet/.env`, which is where the api writes it when it runs from `api/`). If no
fleet env is readable, the suite falls back to the org-wide key, `E2E_AGENT_KEY` plus
`x-agent-name`. If neither is available, chapter 6 checks the allowance through
`GET /api/fleet/defs/{id}/mcp` and records a `limitation` annotation saying that the call
itself was not made.

**No container is ever started.** An MCP edit on an agent defaults to `apply: true`,
which re-renders the fleet and then *rolls* the agent: a blue-green container swap that
pulls the Hermes image and joins the `talaria` docker network. That is fleet
reconciliation, not the MCP surface, and in CI it has neither the image nor the network.
So chapter 5 adds `apply: false` to each request its MCP tab sends (`page.route`), and
chapter 6 assigns through the api with `apply: false`. The fleet render still runs. It
writes the assignment the gateway enforces and the agent's key the gateway accepts.

**Hiring needs the fleet tree.** A hire renders the agent from `<fleet dir>/chassis.yml`
and appends its key to `<fleet dir>/.env`. A worktree gets both from
`talaria worktree`. CI seeds them the same way (the chassis template plus an `.env`
header) before the app starts. On a fresh database, chapter 5 also adds a placeholder
LLM endpoint, because a hire refuses an instance with no models. Nothing ever calls that
endpoint, and `afterAll` removes it.

## The stubs

`scripts/e2e/mcp-stubs.mjs` is one Node or Bun process on one port (`E2E_STUB_PORT`,
default 5390). It uses only the standard library.

- **`/v0.1/servers`** is a fake official registry. It pages with a cursor, two entries per
  page whatever `limit` asks, so the mirror's cursor walk is really exercised. It also
  honours `version=latest`, `updated_since` and `search`. The entries are:
  - `e2e/open`, `e2e/key` and `e2e/oauth`, which point at the stubs below;
  - `e2e/dead`, an https URL on a port nothing listens on;
  - `e2e/package`, an npm package entry;
  - an older `e2e/open` marked `isLatest: false`.

  Entries carry the stub's start time as `updatedAt`, so an incremental pass sees each
  entry once.
- **`/open/mcp`** is streamable-HTTP MCP: `initialize`, `tools/list` (`echo`, `shout`,
  `count`) and `tools/call`.
- **`/key/mcp`** is the same server, but it answers 401 unless the request carries
  `X-Api-Key: stub-secret-4f9c2a`. The key is shaped so that it cannot be a substring
  of any server name the run registers, because chapter 3 asserts that it never comes back
  from the api. It answers in SSE framing, so the event-stream reader is tested too. Its 401 names no OAuth metadata, and the origin-level
  `/.well-known/oauth-protected-resource` is deliberately a 404, so discovery reads it as
  a key server.
- **`/oauth/mcp`** sits behind OAuth 2.1. Its 401 carries
  `WWW-Authenticate: Bearer resource_metadata=".../.well-known/oauth-protected-resource/oauth/mcp"`.
  Behind that sit the protected-resource metadata, the RFC 8414 authorization-server
  metadata for the issuer `<stub>/oauth`, dynamic client registration, an
  `/authorize` that 302s straight back with a code, and a `/token` that checks the
  redirect URI and the PKCE S256 verifier. The bearer `/token` issues is the one
  `/oauth/mcp` accepts. Refresh tokens work too.
  The metadata is served at the RFC 8414 path
  (`/.well-known/oauth-authorization-server/oauth`), at the appended form, and at
  `<issuer>/.well-known/openid-configuration`. Only that last one is reached today. Of
  the candidate URLs Talaria builds for an issuer (talaria-mcp-oauth
  `as_metadata_candidates`), it is the only one copied from the issuer URL as is. The
  others are rebuilt from the scheme and host and lose a non-default port such as
  `:5390`.
- **`/_e2e/calls`** lists every `tools/call` that reached a stub. **`/_e2e/health`**
  reports readiness. The stub logs one line per request (method, path, status; never a
  header or a body) to stdout.

**Why HTTPS.** The catalog mirror keeps only `https://` remotes, so a stub on plain http
would be filtered out before the suite saw it. Instead of a test-only exception in the
product, the stub mints a throwaway CA (with `openssl`) into `E2E_STUB_DIR`. It reuses
that CA on every later start, because a running api keeps trusting the CA it started
with. Next to the CA it writes `ca-bundle.pem`: the system roots plus that CA. The api
reaches the stub through three operator settings, none of which is a back door:

| Setting | Why |
|---|---|
| `TALARIA_FETCH_ALLOW_HOSTS=127.0.0.1` | the SSRF guard refuses loopback otherwise |
| `SSL_CERT_FILE=<E2E_STUB_DIR>/ca-bundle.pem` | the api's TLS roots must include the stub CA |
| `TALARIA_MCP_REGISTRY_URL=https://127.0.0.1:5390/v0.1/servers` | the catalog mirror walks the stub, not the official registry |

`node scripts/e2e/mcp-stubs.mjs --init` mints the CA and prints exactly those lines.

## Running it locally, against a worktree

The api reads all three settings when it starts, so the stack has to be started with
them. A stack that is already running cannot be repointed.

```bash
# 1. In the worktree (bun talaria worktree <name>): mint the CA and see the env.
#    E2E_STUB_DIR defaults to <OS temp>/talaria-e2e-stub-<port>, the same dir on
#    every start, so --init and the server always agree on the CA.
E2E_STUB_DIR=$PWD/ui/e2e/.state/stub node scripts/e2e/mcp-stubs.mjs --init

# 2. Start the stack with those lines in the shell. talaria dev passes the
#    shell's env through to the api it spawns. Add the OAuth callback base
#    when the browser origin is not the one the request arrives on.
env $(E2E_STUB_DIR=$PWD/ui/e2e/.state/stub node scripts/e2e/mcp-stubs.mjs --init) \
  AUTH_PUBLIC_URL=http://localhost:<app port> bun talaria dev

# 3. Start the stubs (another shell), then run the suite.
E2E_STUB_DIR=$PWD/ui/e2e/.state/stub node scripts/e2e/mcp-stubs.mjs
cd ui && E2E_BASE_URL=http://localhost:<app port> \
  E2E_DATABASE_URL=<the worktree's DATABASE_URL> E2E_REDIS_URL=<its REDIS_URL> \
  bun run test:e2e
```

- **The session.** The global setup mints an admin session the way a sign-in does: it
  writes a `users` row to `E2E_DATABASE_URL` and sets `sess:<sid>` in `E2E_REDIS_URL`.
  Pass `E2E_SESSION=<cookie value>` to use an existing session instead.
- **A cold api.** A worktree's api can take minutes to compile after the app answers. The
  setup polls `/api/me` for up to `E2E_API_WAIT_MS` (10 minutes by default).
- **Preflight.** Before the first test, the setup asks the api to probe the stub
  (`POST /api/mcp/test`). If the probe fails, the run stops with the sentence above
  instead of failing in chapter 2. `E2E_PREFLIGHT=warn` runs the tests anyway, which helps
  when diagnosing a stack.
- **Your catalog.** Pointing a stack's catalog at the stub adds the stub's entries to its
  mirror. A *full* walk against the stub then prunes every real entry. Use a throwaway
  worktree, not one whose catalog you care about.
- **An api that cannot follow the registry override.** A binary built before
  `TALARIA_MCP_REGISTRY_URL` existed keeps its catalog on the official registry. Set
  `E2E_MARKETPLACE=0` and chapter 1 skips. Chapters 2–4 then register the stub servers by
  URL, with the request body the marketplace sends, and every chapter after the install
  still runs.
- **Output.** Traces and screenshots from failures go to `ui/e2e/test-results/` (or
  `E2E_OUTPUT_DIR`). In CI, the HTML report goes to `ui/e2e/report/` (or
  `E2E_REPORT_DIR`). The minted session goes to `ui/e2e/.state/` (or `E2E_STATE_DIR`),
  and so does the stub's CA if you keep `E2E_STUB_DIR` there, as above. All of it is
  gitignored. Playwright **empties** `E2E_OUTPUT_DIR` when a run starts, so keep logs and
  JSON reports somewhere else.
- **The browser.** `@playwright/test` is pinned in `ui/package.json`. Run
  `npx playwright install chromium` once if the matching browser is not cached.

`bun run test:e2e` from the repo root runs the same suite. `bun run typecheck:e2e` (in
`ui/`) typechecks it. The ui's svelte-check does not cover `e2e/`.

## In CI

The `e2e` job in [`.github/workflows/ci.yml`](../.github/workflows/ci.yml) runs when its
surface moves: `ui/`, `scripts/e2e/`, the `talaria-mcp*` crates, `talaria-routes-fleet`,
`api/Cargo.lock`, or the workflow itself. It:

1. writes the run's directories under `$RUNNER_TEMP` into `$GITHUB_ENV` (job-level `env:`
   cannot use the `runner` context) and seeds the fleet tree (`chassis.yml` from
   `scripts/chassis.template.yml`, plus an `.env`);
2. boots `postgres:16-alpine` and `redis:7-alpine` the way `api-integration.yml` does,
   and replays the migrations (`bun run migrations:check`, which doubles as the snapshot
   check);
3. builds the api in debug mode, reading the api job's cargo cache, and builds the ui;
4. starts the stubs, with `--init` writing the three settings into `$GITHUB_ENV`;
5. starts the app with `bun run start`, which spawns the api binary with that env, with
   `AUTH_PUBLIC_URL` set to the app's origin;
6. installs chromium and runs the suite.

On failure it uploads one artifact holding `e2e-results/` (traces and screenshots),
`e2e-report/` (the HTML report) and `e2e-logs/` (the app and stub logs). The logs sit
beside the Playwright output, not inside it, because Playwright empties its output
directory when the run starts.
