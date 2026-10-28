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
| Add a server | **Custom server** (label, URL, auth mode, as many auth headers as it needs, timeout) or **Browse marketplace** |
| See its tools | **{n} tools** on the card, or **Discover tools** to ask the server for its catalog — the card then says *ready*, *needs credentials*, *unreachable* or the server's own error |
| Give it an API key | **Set credentials** on the card (or kebab → **Edit credentials…**): the publisher's declared fields, or free header rows; saving re-tests |
| Connect an OAuth server | **Connect** on the card — signs in the *org* account so agents can use it. A provider that needs a registered app shows **Set up**: the provider's developer portal, the exact callback URL to register, client id/secret, scopes |
| Disconnect the org's OAuth account | Kebab → **Disconnect org account** (admins): within a few minutes, without a restart, agents that carry the server can no longer use its tools (each call answers that the account isn't connected) until someone connects it again |
| Re-check how it authenticates | Kebab → **Re-detect auth** (a server that was down at install, or that started speaking OAuth later) |
| Carry it on every agent | In **Manage access**, check **all agents** — rows below become per-agent tool overrides |
| Narrow one agent's tools | In **Manage access**, add that agent and pick a tool subset (**All tools** if untouched) |
| Gate a person | In **Manage access**, add a person rule — everyone with an assigned agent may use it otherwise |
| Turn one off | Kebab → **Disable server**; enable reverses it |
| Remove one | Kebab → **Remove server** — every agent that carries it loses it within about a minute, without a restart |

Cards say where a server came from: `built-in`, `app` (published by an app — lifecycle
belongs to the Apps page), or a domain. **Switch to per-user auth** makes each person
connect their own account instead of the org's.

The marketplace is a local copy of the official registry (synced every half hour; **Sync now**
if the status line says it has not synced yet). Search by publisher, name or description, and
filter by tier (`official`, `verified`, `community`) or by hosted endpoint vs runnable package
(npm/pypi/docker — Talaria runs those in a hardened container). Installing runs discovery in the
same step: a server that wants a key asks for it right there, one that speaks OAuth offers
Connect.

**Per agent:** an agent's settings → **MCP** tab shows the same registry from that agent's side —
toggle a server on or off for the agent, narrow its tools, and (with `agents.manage`) add from
the marketplace straight onto the agent.

## Observability

Open: **Manage → Observability** — tabs **Overview · Compute · Host · Cost · Audit · Alerts**.
The overview cross-sections the rest: what's generating now, gateway health over 15 minutes,
spend today, the host (CPU and the fullest mount), the audit pulse, and any alerts.

| Tab | What it shows |
| :--- | :--- |
| **Compute** | Self-hosted usage: tokens today and over 30 days, generations, which backends served them |
| **Host** | The machine: CPU and load, memory, swap, disk per mount (fullest first), and the processes using them. Refreshes every few seconds. A container deploy shows the host, not the container, once the host mounts are in place |
| **Cost** | Tokens per day (last 14), spend by agent (30 days), and the self-hosted vs cloud split |
| **Audit** | The record, filterable by kind |
| **Alerts** | What needs a person: agents down, gateway trouble, unpriced ledger, stuck work, a filling disk or a box out of memory |

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
start talking. Cache and reasoning tokens are accounted separately (cache writes bill at
1.25× input, reads at 0.1× — reasoning rides inside output), dollar figures are rate-card
estimates rather than provider invoices, and models without prices set on the
[Models](./admin-models.md) page show as unattributed — the "ledger priced" alert is the
honesty check. **Admin → Settings** also carries rolling-window LLM budgets (org-wide and
per-caller token/$ ceilings, checked before every gateway call) and a minimum cron interval;
both are unlimited/unset by default. An agent that reaches its budget ends the turn and says
when the window reopens. If you raise the limit before then, the agent works again at once.
One exception: a conversation in which the agent had switched to its fallback model stays on
the fallback until that time, until it has been idle for about an hour, or until you restart
the agent. New conversations use the main model.

## Apps (admin)

The admin actions on **Manage → Apps** — install (marketplace or any git repository with a
`talaria.json`), enable, uninstall — and the grant: each app view is allowed per person in
Admin → People. The full lifecycle and the member experience:
[Apps](./apps.md).
