# Talaria stack (legacy Phase-1 fleet engine)

> **Heads up: this documents the legacy Phase-1 fleet engine.** In Phase 1 the "fleet engine" was a
> Docker stack that stood up **hermes-workspace** + **mission-control** and fronted them with
> **talaria-bridge**. That scaffolding is kept here for reference, but it is not the product. The
> product is the Talaria app in [`ui/`](../ui) (boards, tickets, teams, multiplayer, auth), backed by
> its own Postgres/Redis. Talaria does not run hermes-workspace or mission-control anymore; we lifted
> the good parts into our own app. The current fleet engine is just the **gateway plane** (the
> multiplexer on `:8642`). See the [top-level README](../README.md) for where things actually stand.
>
> The rest of this file describes the Phase-1 stack as it was built and verified. The `docker compose
> up` flow below still works; just read it as the legacy engine, not Talaria's identity.

Brings up the Phase-1 pieces: **hermes-workspace** (the cockpit) + **mission-control** (the ops
console) + **talaria-bridge** (both planes), wired to your fleet.

## Topology

```
                         hermes-workspace
        HERMES_API_URL ──►│              │◄── HERMES_DASHBOARD_URL
       (gateway plane)    ▼              ▼    (dashboard plane, legacy)
                  talaria-bridge :8642   talaria-bridge :9119
                        │                      │
       routes /v1/chat by model               ├─ serves /api/conductor/* + kanban ─► mission-control
       + merges /api/sessions                 └─ proxies the rest ─► real Hermes dashboard
                        │
                        ▼
        agent-1 … agent-N gateways (the fleet, from fleet.json)
```

The **gateway plane** (`:8642`, model-routed chat) is the part that survives as today's fleet engine.
The **dashboard plane** (`:9119`) and the mission-control bridging were Phase-1 scaffolding for
fronting hermes-workspace + mission-control, and are legacy.

## Prereqs

- The root `packledger-services` stack is up (creates the `packledger-services_edge` network).
- Your Hermes fleet is up. Talaria reaches the agent gateways over the fleet network. This stack
  joins `ai_default` (the `ai/orchestration` fleet net); change it in `docker-compose.yml` if yours
  differs.

## Run

```bash
cp .env.example .env                 # fill MISSION_CONTROL_API_KEY + HERMES_PASSWORD
cp fleet.example.json fleet.json     # declare your agents: model → gateway url + API_SERVER_KEY
docker compose up -d --build
../scripts/verify-stack.sh           # should print ALL PASS
```

`fleet.json` is the **fleet manifest** (and is gitignored, since it holds the per-agent keys). Each
entry maps a model name (the agent's `API_SERVER_MODEL_NAME`) to its gateway URL and key. The gateway
plane exposes each as a model, so in the Phase-1 workspace the model switcher became the agent
switcher.

### Two deployment shapes, both supported from one manifest

Hermes runs a fleet in either of two shapes, and the gateway plane handles both with the *same* entry:

- **A) Separate installs (canonical):** one Hermes gateway per agent, each on its own host
  (`http://agent-x:8642`). List one entry per agent.
- **B) Multiple profiles on one host:** one Hermes install, several profiles, each profile's API
  server on its own port (`:8643`, `:8644`, ...). List one entry per profile pointing at the same
  host / different ports, and set `"profile"` so the gateway rewrites the forwarded `model` field to
  that profile.

Optional per-entry fields (for profile-routed gateways):

| Field | Purpose |
|---|---|
| `profile` | Hermes profile name; forwarded as the upstream `model` so the gateway routes to it. |
| `upstreamModel` | Explicit override for the forwarded `model` (defaults to `profile ?? model`). |
| `pathPrefix` | Prepended to upstream calls, e.g. `/p/<profile>`, for Hermes' emerging single-endpoint profile multiplex (`multiplex_profiles`; see NousResearch/hermes-agent #24913, #23735). Default: none. |

Callers are unaffected either way; they only ever see the gateway's exposed `model` ids. See
[`fleet.example.json`](./fleet.example.json) for both shapes side by side.

Local debug ports (bound to `127.0.0.1`): dashboard plane `:9119` (legacy), gateway plane `:8642`,
mission-control `:8700`, workspace `:8711`.

## Expose (Cloudflare tunnel)

Routing lives in the Cloudflare dashboard (Zero Trust → Networks → Tunnels → Public Hostnames). Add
`workspace.packledger.co` → `http://hermes-workspace:3000` and gate it with Cloudflare Access, like
the other AI UIs. (Phase-1 workspace exposure; the product app in `ui/` is served separately.)

## Notes

- **mission-control has no published image**, build it from source (pinned commit `d09e608`); see the
  comment in [`docker-compose.yml`](./docker-compose.yml).
- **hermes-workspace** is the published `ghcr.io/outsourc-e/hermes-workspace:latest`.
- Two of the workspace's surfaces still ride the workspace-native path (out of the bridge's reach):
  the dedicated *agents-online* widget resolves to the fleet's default agent. Chat, sessions,
  missions, and the kanban board are all per-agent / fleet-wide today.
