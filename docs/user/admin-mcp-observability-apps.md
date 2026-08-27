# Admin: MCP, Observability, Apps

Three manage surfaces round out the control plane. **MCP** governs every tool server your
agents may reach — through Talaria's gateway, so the limits are enforced, not advisory.
**Observability** is the ops plane: compute, cost, the audit trail, and alerts. **Apps**
admin is installing and governing the apps themselves (the member side of apps is its own
[chapter](./apps.md)).

## MCP

Open: **Manage → MCP** — tabs **Built-in** (the Talaria toolkit, already available to every
managed agent) and **External**. Register once, then choose which agents carry each server
and which people may use it, down to individual tools.

| Do this | How |
| :--- | :--- |
| Add a server | **Custom server** (label, URL, auth mode, headers, timeout) or **Browse marketplace** |
| See its tools | **{n} tools** on the card, or **Discover tools** to ask the server for its catalog |
| Connect an OAuth server | **Connect** on the card — signs in the *org* account so agents can use it |
| Carry it on every agent | In **Manage access**, check **all agents** — rows below become per-agent tool overrides |
| Narrow one agent's tools | In **Manage access**, add that agent and pick a tool subset (**All tools** if untouched) |
| Gate a person | In **Manage access**, add a person rule — everyone with an assigned agent may use it otherwise |
| Turn one off | Kebab → **Disable server**; enable reverses it |
| Remove one | Kebab → **Remove server** — every agent loses it on its next roll |

Cards say where a server came from: `built-in`, `app` (published by an app — lifecycle
belongs to the Apps page), or a domain. **Switch to per-user auth** makes each person
connect their own account instead of the org's.

The marketplace lists hosted servers from the official registry — `official`, `verified`,
and `community` tiers. Only servers with a hosted endpoint appear; packages that need a
local process can't be one-click added.

## Observability

Open: **Manage → Observability** — tabs **Overview · Compute · Cost · Audit · Alerts**.
The overview cross-sections the rest: what's generating now, gateway health over 15 minutes,
spend today, the audit pulse, and any alerts.

| Tab | What it shows |
| :--- | :--- |
| **Compute** | Self-hosted usage: tokens today and over 30 days, generations, which backends served them |
| **Cost** | Tokens per day (last 14), spend by agent (30 days), and the self-hosted vs cloud split |
| **Audit** | The record, filterable by kind |
| **Alerts** | What needs a person: agents down, gateway trouble, unpriced ledger, stuck work |

The all-clear reads: *agents running, gateway answering, ledger priced, no stuck work* —
each alert deep-links to its fix.

### The audit kinds

| Kind | What lands there |
| :--- | :--- |
| **Tickets** | Board activity: status moves, dispatches, comments, gaps |
| **Channels** | Messages in channels you belong to |
| **Fleet** | Agent configuration versions |
| **Governance** | Admin actions: settings, permissions, renders, deletions — admin-only, off by default |

How long the audit keeps its record is **Admin → Security → Settings → Audit retention**
(days; 0 = keep forever).

Cost comes from the token ledger: every chat turn and channel reply lands in it once agents
start talking. Models without prices set on the [Models](./admin-models.md) page show as
unattributed — the "ledger priced" alert is the honesty check.

## Apps (admin)

The admin actions on **Manage → Apps** — install (marketplace or any git repository with a
`talaria.json`), enable, uninstall — and the grant: each app view is allowed per person in
Admin → People. The full lifecycle and the member experience:
[Apps](./apps.md).
