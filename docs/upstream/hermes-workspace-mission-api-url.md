# Upstream PR — hermes-workspace: `HERMES_MISSION_API_URL`

Target: [`outsourc-e/hermes-workspace`](https://github.com/outsourc-e/hermes-workspace)
Patch: [`hermes-workspace-mission-api-url.patch`](./hermes-workspace-mission-api-url.patch)
(4 files, +57/-9)

## Title

`feat: HERMES_MISSION_API_URL — decouple Conductor mission dispatch from the dashboard URL`

## Body

### What

Adds an optional `HERMES_MISSION_API_URL` env var. When set, the workspace sends only its
**Conductor mission-dispatch calls** (`/api/conductor/*`: the capability probe, create, poll, cancel)
to that base URL. Everything else (sessions, skills, config, MCP, kanban, websockets, …) keeps going
to `HERMES_DASHBOARD_URL` exactly as before. When unset, `HERMES_MISSION_API_URL` **defaults to
`HERMES_DASHBOARD_URL`**, so the change is a no-op for existing setups.

### Why

Today the Conductor mission API is assumed to live on the dashboard (`:9119`). Teams who run a
dedicated fleet manager / orchestrator want to answer *just* the mission calls from that service
without having to reverse-proxy the entire dashboard in front of it. Right now that forces an
all-or-nothing choice: either point the whole dashboard URL at the proxy, or don't integrate.

This turns the existing "dashboard has conductor? use it : fall back to native-swarm" behavior into a
clean, opt-in seam: point `HERMES_MISSION_API_URL` at your orchestrator and the workspace uses it for
missions while the dashboard stays exactly where it is. It makes fleet-manager integrations a
sanctioned one-liner instead of a full-dashboard proxy hack.

### How

Mirrors the existing `gatewayFetch` / `dashboardFetch` split in `src/server/gateway-capabilities.ts`:

- Factor the shared request/auth body of `dashboardFetch` into a private `baseFetch(requestPath, init)`.
- Add `missionApiBase()` (`HERMES_MISSION_API_URL` normalized, else `CLAUDE_DASHBOARD_URL`) and a
  `missionFetch(path, init)` that targets it. Auth is injected identically, so the mission endpoint is
  expected to be dashboard-token compatible (a bridge can accept or ignore the token).
- Route the four `/api/conductor/*` call sites (`probeConductor`, `createDashboardConductorMission`,
  the mission poll, and `conductor-stop`'s cancel) through `missionFetch`.
- Document the var in `.env.example`.

No behavior change unless the operator sets the new var. No dependency changes.

### Verified (runtime, against a live stack)

Built a `hermes-workspace` image from this branch and ran it with `HERMES_MISSION_API_URL` pointed at
a mission bridge while `HERMES_DASHBOARD_URL` pointed **straight at a real Hermes dashboard**. Result:
the only request the bridge received was the Conductor capability probe
`GET /api/conductor/missions`. Every other dashboard call (`/api/status`, `/`, `/api/mcp`,
`/api/plugins/kanban/board`, …) went directly to the dashboard, bypassing the bridge.

For contrast, with `HERMES_MISSION_API_URL` unset (or pointed at the same place as the dashboard) the
behavior is identical to today, since it defaults to `HERMES_DASHBOARD_URL`. So the change is a no-op
unless you opt in, and when you opt in it cleanly peels off *only* the mission calls.

The receiving end was already proven separately: the bridge serves `/api/conductor/*` and round-trips
missions to a mission-control fleet (create/poll/cancel).

> Heads-up (separate from this change): building the image from a clean clone currently fails on
> modern pnpm because the Dockerfile's dependency-install step copies `package.json` +
> `pnpm-lock.yaml` but **not** `pnpm-workspace.yaml`, where the `allowBuilds:` approvals live, so pnpm
> aborts on ignored build scripts. One-line fix (also copy `pnpm-workspace.yaml`) in
> [`hermes-workspace-dockerfile-pnpm-workspace.patch`](./hermes-workspace-dockerfile-pnpm-workspace.patch);
> happy to send it as its own PR.

### Real-world use

This is the sanctioned integration point for **Talaria** (https://github.com/outcrop-labs/talaria), a
small MIT bridge that lets hermes-workspace drive a
[mission-control](https://github.com/builderz-labs/mission-control) fleet as its orchestration brain.
Talaria currently proxies the whole dashboard to answer conductor calls; with this var it only needs
to answer the mission calls, which is cleaner and lower-risk for everyone.
