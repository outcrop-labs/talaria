# Upstream notes: hermes-workspace

> **Lift-source reference (legacy Phase-1).** These are notes on an upstream we lift features FROM, not a
> dependency Talaria runs as the product. We rip hermes-workspace's chat and agent UX into Talaria's own
> UI (`ui/`); we don't run or proxy hermes-workspace. The patches below and the combined "fleet" image
> come from the old Phase-1 setup that ran hermes-workspace directly (now legacy). The current fleet
> engine is the gateway plane (`:8642`) documented in the root README. Keeping this here for history and
> because the `HERMES_MISSION_API_URL` write-up still explains upstream internals we studied.

Three small changes to [`outsourc-e/hermes-workspace`](https://github.com/outsourc-e/hermes-workspace),
each opened as its own PR from the `joniler` fork (authored `jon@packledger.co`). Upstream merge is the
maintainers' call. In the Phase-1 stack that ran hermes-workspace directly, we used a **combined image**
that includes all three (see below).

| Patch | What | PR branch |
|---|---|---|
| [`hermes-workspace-mission-api-url.patch`](./hermes-workspace-mission-api-url.patch) | `HERMES_MISSION_API_URL`, decouple Conductor mission dispatch from the dashboard URL (no-op unless set) | `feat/hermes-mission-api-url` |
| [`hermes-workspace-dockerfile-pnpm-workspace.patch`](./hermes-workspace-dockerfile-pnpm-workspace.patch) | copy `pnpm-workspace.yaml` before install so a clean-clone `docker build` works on modern pnpm | `fix/dockerfile-copy-pnpm-workspace` |
| [`hermes-workspace-gateway-agents-roster.patch`](./hermes-workspace-gateway-agents-roster.patch) | add `/api/gateway/agents` so the Agents screen shows real agents (the fleet) instead of a stub | `feat/gateway-agents-roster` |

See [`hermes-workspace-mission-api-url.md`](./hermes-workspace-mission-api-url.md) for the detailed
write-up on the flagship change.

## The combined "fleet" image (legacy Phase-1)

The old Phase-1 stack ran `packledger/hermes-workspace:fleet` = stock hermes-workspace + all three
patches. To (re)build it from a clean clone:

```bash
git clone https://github.com/outsourc-e/hermes-workspace
cd hermes-workspace
git apply /path/to/talaria/docs/upstream/*.patch
docker build -t packledger/hermes-workspace:fleet .
```

Then set `HERMES_WORKSPACE_IMAGE=packledger/hermes-workspace:fleet` in `stack/.env`. When the PRs merge
upstream, drop the patches and go back to the published `ghcr.io/outsourc-e/hermes-workspace` image.
