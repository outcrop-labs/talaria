# Admin: Agents

The Agents page is the roster: hire agents, set their identity and brains, bind their tools,
and retire them. Changes to a running agent deploy like software — a fresh container rolls up
beside the old one, a health check runs, then traffic cuts over — so nobody's conversation
dies to an edit. The policy panels in **Admin → Agents** govern how all agents behave; this
page governs each one.

Open: **Manage → Agents**. Members need the view granted (and the **Manage agents**
permission to edit anything).

## Hire an agent

**+** → **New agent**. Describe the job in a sentence or three — *"A release manager that
tracks our deploy trains…"* — and **Muse designs it**: identity, soul, and starter skills,
ready for your red pen. Set **Name** and handle, pick a **role** (from **Your organization**
or the **Common roles** Talaria maintains) and department, and optionally **Start the
container now**.

Empty fleet? *"Describe the first one and Muse designs it: identity, soul, and starter
skills."* — **Design your first agent**.

Outside agents can be pulled in too: the federation action (top of the page) federates agents
that live elsewhere into Talaria.

## The manage modal

Every agent opens a modal with the same eight tabs: **Summary · Config · Skills · Memory ·
Crons · Secrets · MCP · Versions**.

| Tab | What it holds |
| :--- | :--- |
| **Summary** | The agent at a glance |
| **Config** | Main model, model tiers (aliases), fallbacks, and the **Soul** |
| **Skills** | What it knows how to do — starter skills and additions |
| **Memory** | Its own memory volume, kept across conversations |
| **Crons** | Scheduled work it does on its own |
| **Secrets** | Credentials it can spend without ever reading |
| **MCP** | Tool servers it carries |
| **Versions** | Every rendered config, per version |

### Config

- **Main model** — its brain.
- **Model tiers (aliases)** — named slots (like `opus`) you can address per-message with a
  tier mention.
- **Fallbacks (when the model above is down)** — the chain it falls through instead of
  failing.
- **Soul** — the SOUL.md workspace: *who the agent is*. Saving it publishes a soul-only
  version on top of the last saved config.

Applying config rolls out safely: *new container rolling up beside the old one, health
check, then traffic cuts over*. Add a version note if the change deserves one.

### Versions

The rendered model/tool config per version — click a revision to see what changed. Versions
are append-only: **reverting to v3** doesn't rewind, it publishes the old config *as a new
version*, so history never has a hole.

### Secrets

Grant the agent a **workspace credential** (sealed in Admin → Secrets — it gets a handle to
spend, never the value) or add its own env-var-style entries (`FIGMA_TOKEN` — the value is
write-only). Nothing entered? *"Everything it needs comes from the shared platform env."*
Which secrets an agent may spend is admin-only; members see **Admins only.**

### MCP

The servers this agent carries. Chips say where each came from: **built-in** (the Talaria
toolkit, attached to every managed agent automatically) or **org registry** (assignment, tool
subsets, and credentials managed on the [MCP](./admin-mcp-observability-apps.md) page). Probe
states: `Connected` · `Login required` · `Unreachable` · `Error` — with **Test** to re-check.

## Retire an agent

The delete action asks in plain terms: it removes the definition, version history, secrets,
and (for Talaria-created agents) the memory volume — but *chats, tickets, and ledger history
it produced are kept*. It cannot be undone.

## Role templates and schedules

- **Role templates** — the starter personas new agents can be hired from. Talaria maintains
  the **Common roles**; editing one saves *your* version under the same name, yours is used
  from then on, and deleting it restores ours.
- **Schedules** — the org's cron jobs at a glance.

## Agent policy (Admin → Agents)

Three panels in the Admin area's **Agents** tab govern every agent at once:

| Panel | Controls |
| :--- | :--- |
| **QA judge** | Run the judge on quality review, **Enforcing** or **Advisory**; each board can override (enforcing / advisory / off), and boards on "inherit" follow this stance |
| **Confab guard** | Mode — `Off` / `Observe` / `Annotate` / `Strict` — plus minimum confidence, coaching agents from findings, and the findings feed |
| **Proactive outreach** | Periodic check-ins (every N minutes), a DM cap per day, and which agents are proactive |

## Words Agents uses

| Term | Meaning |
| :--- | :--- |
| **Soul** | The agent's SOUL.md — who it is, in its own voice |
| **Tier** | A named model alias on the agent; `@Name:tier` routes one reply to it |
| **Roll** | The safe deploy: new container beside the old, health check, cutover |
| **Muse** | Talaria's designer agent that drafts new agents from a job description |
| **Federate** | Pulling an agent that lives outside Talaria into the roster |
