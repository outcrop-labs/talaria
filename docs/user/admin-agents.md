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
| **Summary** | The agent at a glance, and its credential |
| **Config** | Main model, model tiers (aliases), fallbacks, and the **Soul** |
| **Skills** | What it knows how to do — starter skills and additions |
| **Memory** | Its own memory volume, kept across conversations |
| **Crons** | Scheduled work it does on its own |
| **Secrets** | Credentials it can spend without ever reading |
| **MCP** | Tool servers it carries |
| **Versions** | Every rendered config, per version |

### Summary

The agent at a glance: role, send address, Google identity, and the switches that shape it.
At the bottom, for its managers, **Rotate credential**: the key the agent presents to Talaria
is revoked and a fresh one minted, the answer to a leaked key that is not "retire the agent".
The confirm says what happens, and it is exact: the old key stops working the moment you
confirm; the fleet is re-rendered and a running container is rolled onto the new key (a fresh
one comes up beside it, traffic cuts over once it is healthy); until then the old container is
locked out, so a toolkit call or a workbench session in flight on the old key fails and has to
be retried, while conversations carry on in the new container. An agent Talaria does not run
gets the new key the next time it starts. The new value is never shown; the audit trail records
who rotated which key. If the roll fails, roll the agent again from the roster, or stop and
start it.

Admins can roll the whole fleet from the roster: **Roll all** asks first, in the page, then
rolls every running agent the same zero-downtime way, a couple at a time so the host stays
responsive (tick *Also start and roll agents that are stopped* to include the stopped ones).
Each agent's row shows where its roll stands: rolling, rolled, or failed with the reason, and an
agent mid-roll cannot be rolled again until it finishes. The strip above the roster keeps the
tally and every failure sentence until you dismiss it.

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

Below the servers, **Hermes toolsets**: the tools the agent's own harness carries before any
MCP server (`browser`, `terminal`, `file`, `code_execution`, `web`, …). A box is the toolset
being on for this agent; clearing one closes it on the next roll, and a new agent starts with
the shell, code execution, the file tool and self-scheduling closed until a manager opens them.
Two kinds of row are locked: `todo` and `clarify`, which Talaria's own tools replace for every
agent, and anything an admin closed org-wide (**Admin → Security → Agent toolsets**, below).
The row's tooltip says which.

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

One more lives under **Admin → Security → Agent toolsets**: the org-wide toolset floor. It is
the same checklist the agent editor shows, over the whole fleet: clearing a box closes that
Hermes toolset for every agent, whatever its managers opened, and managers then see it locked in
the agent's own editor with "closed org-wide by an admin". `todo` and `clarify` are locked here
too, for the same reason as everywhere else. Saving rolls nothing: the floor lands on each agent
at its next render or roll, which is what the panel says when it saves. Names the api's
environment pins (`TALARIA_MANDATORY_DISABLED_TOOLSETS`) show locked as well, since unticking
them here would change nothing.

**Admin → Search → Agent web search** picks the web provider every agent searches with, through
its own Hermes: Tavily, Exa, Parallel, Firecrawl, Keenable, the Perplexity Search API, Brave
Search, or SearXNG at the workspace search URL, each with its key (stored encrypted, never shown
again), plus an optional separate provider for reading pages. Left on Hermes's default, Talaria
sets nothing. Once you pick a provider, or set a search URL the agents can reach, agents stop
falling back to the anonymous free tiers of Exa, Parallel, Firecrawl and Keenable unless you tick
**Allow Hermes's keyless public tiers**. Research is not affected: it searches with the search URL
or a registered search MCP server. Like the toolset floor, a change lands on each agent at its
next roll.

Besides these panels, one rule is stamped into every agent's soul at hire and in every role template, so it holds no matter how the soul is later edited: **agents never do dev work in chat**. Code changes — however small — ship through a ticket and a workbench job; a dev-work request in chat produces (or links) a ticket and moves execution to the workbench, and instructions given in chat are posted to the ticket as comments.
