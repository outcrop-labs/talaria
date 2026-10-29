# Admin: Agents

The Agents page is the roster: hire agents, set their identity and brains, bind their tools,
and retire them. Most changes reach a running agent in place, from its next turn, without a
restart: its tools and toolsets, its delegation cap, its soul (from its next new
conversation), and its MCP servers and tools (within a few minutes). A change the agent can only take
at start — its Developer switch, a rotated key, a model or tier, most config edits — restarts
its container: the agent is offline for about a minute, and a turn in flight waits for it.
None of these starts an agent someone stopped: a stopped agent stays stopped, and takes the
change when it next starts.
The policy panels in **Admin → Agents** govern how all agents behave; this page governs each
one.

Open: **Manage → Agents**. Members need the view granted (and the **Manage agents**
permission to edit anything).

## Hire an agent

**+** → **New agent**. Describe the job in a sentence or three — *"A release manager that
tracks our deploy trains…"* — and **Muse designs it**: identity, soul, and starter skills,
ready for your red pen. Set **Name** and handle, pick a **role** (from **Your organization**
or the **Common roles** Talaria maintains) and department, and optionally **Start the
container now**.

To skip the description, **Configure manually →** opens the same review form empty: type the
name, handle and department, and **Create agent** hires it. **Duplicate** on a roster agent
opens that form with the agent's chassis (model tiers, tools and plugins) preselected, and the
identity is yours to fill in.

A department is the agent's own: it names its container and its memory, so no two agents share
one. A department another agent already holds (even a retired one) is refused — the modal says
*Taken by …* as you type it — and so is a federated roster that would put two agents in one. An
install from before this rule may still hold such a pair: the older agent runs in the
department's container, the other one is left out of the fleet, and both agents' **Summary**
tabs and the alerts say so. Nothing you do on the left-out agent reaches the other one's
container: starting, stopping and rolling it are refused, a config change saves as a version
without being applied, and its memory tab answers that it has none of its own. Retire and delete
one of them to untie it; Talaria never merges two agents into one. If the newer agent had already
taken the container before the upgrade, the first fleet roll after an image deploy puts the older
one back; on an install run from a checkout, **Roll** the older one to put it back in its
container.

Empty fleet? *"Describe the first one and Muse designs it: identity, soul, and starter
skills."* — **Design your first agent**.

Outside agents can be pulled in too: the federation action (top of the page) federates agents
that live elsewhere into Talaria. A credential written inline in an imported config (a platform
token, a provider's key, an `Authorization` header) is never stored as written. Each one the
agent uses becomes one of its own secrets, named for where it sat
(`CONFIG_PLATFORMS_TELEGRAM_TOKEN`), and the config keeps a `${CONFIG_…}` reference in its
place. A model's inline key, an MCP server's inline credentials and the API server's own `key`
are dropped, because agents never use them: they reach models through the gateway (put the
provider's key on **Models**), MCP servers through the MCP registry, and Talaria calls the agent
with a key of its own. The federation result lists both what was sealed and what was dropped. A
credential that spans several lines or runs past 8 KB (an inline PEM key) cannot be delivered
at all, so federation refuses that agent and names where it sat.

Agents federated before that sealing existed may still store those credentials as written, in
every version of their config. The federation dialog's last line, **Seal the credentials their
stored configs still hold**, opens a dry run: it reads every agent's stored versions and lists
what sealing would do, agent by agent, writing nothing. **Seal** then does it. Each credential
becomes one of the agent's `CONFIG_…` secrets with the value its current version holds, and
every version, old ones included, keeps a reference in its place, so the history and the
history views stop serving the credential. The value the agent runs on does not change. A
running agent that gains secrets restarts once, before its config changes, so it never reads a
reference it cannot resolve. A stopped one's container is removed instead (a restart would
bring back the old container, which lacks the secrets); **Start** creates a new one. **Seal**
acts on the agents the dry run listed, and no others. A credential only older versions held is
not kept: those versions keep a reference with nothing behind it, and reverting to one leaves
that setting out. One too long or multi-line to seal skips its agent unless you tick **Take out
credentials that cannot be sealed**. Each sealed agent writes an `agent.config_sealed` audit row
with the secret names and paths, never a value; one whose secrets were stored but whose configs
were not rewritten (its restart failed, or an edit landed meanwhile) writes
`agent.config_seal_incomplete` with the reason. Ask again and it finishes. The seal keeps going
if you close the dialog. Database backups taken before the seal still hold the old literals.

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
confirm; the fleet is re-rendered and a running container is rolled onto the new key (it stops,
a fresh one starts in its place, and the roll waits for it to report healthy); until then the
agent is locked out, so a toolkit call or a workbench session in flight on the old key fails and
has to be retried, while conversations wait for the new container and carry on there. A
stopped agent stays stopped and takes the new key when it next starts, and so does an agent
Talaria does not run. The new value is never shown; the audit trail records
who rotated which key. If the roll fails, roll the agent again from the roster, or stop and
start it.

Admins can roll the whole fleet from the roster: **Roll all** asks first, in the page, then
rolls every running agent the same way, in place, a couple at a time so the host stays
responsive (tick *Also start and roll agents that are stopped* to include the stopped ones).
Each agent's row shows where its roll stands: rolling, rolled, or failed with the reason, and an
agent mid-roll cannot be rolled again until it finishes. A roll waits three minutes for the fresh
container to report healthy; one that is still booting then (a slow disk can take longer) shows
*starting* rather than *failed*, and turns to rolled once it is up — or to failed, with the
reason, if it never comes up or stops running. Two agents that share a department (a pair from
before a department became one agent's) are rolled once, as the agent that runs in its
container; the other is listed as skipped. The strip above the roster keeps the tally and every
failure sentence until you dismiss it.

Start, stop, restart, retire and re-hire wait up to three and a half minutes for docker. When
docker is slower than that (an image pull, a starved disk), the control stops waiting but the
action goes on, and the row says so — *still stopping*, *still starting* and so on — until the
roster shows the agent in its new state.

### Config

**Save version** saves the form as a new version; **Save & apply** also puts it on the running
agent. When the apply does not land (a roll step failed, the container never reported healthy,
the roll found the agent mid-roll, the agent is retired, or it has no container of its own), the
version is still saved, and the form stays open and says *Saved as version N, not applied:* with
the reason, rather than closing as if it had worked. A soul saved from the workspace says the
same in a notice. A roll whose fresh container is still booting past the roll's three-minute
wait has applied: the form closes, says so in a notice, and the roster shows the agent
*starting* until it is up. **Save & apply** with nothing changed saves nothing new, so it has
nothing new to apply — and it checks whether the agent runs the current version: a version saved
with **Save version** and never applied, or one whose roll did not land, keeps the form open with
*Version N is not applied:* and why, even in a form you just opened. A running agent on the
current version, or a stopped one (its next start puts the current version on), closes quietly.
**Roll** the agent from the roster to put version N on it.

- **Main model** — its brain.
- **Model tiers (aliases)** — named slots (like `opus`) you can address per-message with a
  tier mention. An agent with **pinned sessions** on always answers its chats from its main
  model (Hermes fixes a session's model at its first turn), so its chats show the tier chip
  switched off, with that reason on hover.
- **Fallbacks (when the model above is down)** — the chain it falls through instead of
  failing.
- **Soul** — the SOUL.md workspace: *who the agent is*. Saving it publishes a soul-only
  version on top of the last saved config.

Applying config writes it to the running agent in place. If the edit touches something the
agent reads only at start (a model, a tier, most config keys), its container restarts — offline
for about a minute, with a turn in flight waiting for it; a soul-only edit restarts nothing and
reaches the agent's next new conversation. Add a version note if the change deserves one.

### Versions

The rendered model/tool config per version — click a revision to see what changed. Versions
are append-only: **reverting to v3** doesn't rewind, it publishes the old config *as a new
version*, so history never has a hole. A setting in the old version that names a sealed
credential (`${CONFIG_…}`) the agent no longer holds is left out of the new version, and its
note says where: kept, the agent would read the reference's own text as the value.

### Secrets

Grant the agent a **workspace credential** (sealed in Admin → Secrets — it gets a handle to
spend, never the value) or add its own env-var-style entries (`FIGMA_TOKEN` — the value is
write-only). An entry reaches the agent's terminal and its code execution. Names Talaria or
the agent's runtime use themselves are refused with the reason — `TALARIA_*`, `LLM_*`,
`HERMES_*`, `TERMINAL_*`, and Hermes's own provider and platform names such as
`OPENAI_API_KEY`, `GITHUB_TOKEN` or `TELEGRAM_BOT_TOKEN`, which Hermes strips from every
command the agent runs; give the credential a name of its own instead. An entry saved
under such a name before the rule is marked **Not loaded** in the list with the same reason:
the render leaves it out of the container (and logs each one), and it stays listed until you
remove it. Most of these did nothing a tool could use, but one kind did: a key Hermes's own
tools read for themselves, such as a `TAVILY_API_KEY` its web search used. That stops working
when the agent next restarts; pick the web search provider and its key for every agent in
**Admin → Search → Agent web search** instead. Upgrading a live instance? Releases before
this one mark the same entries **Would be refused today**, so the Secrets tabs show beforehand
which ones stop loading. Nothing entered?
*"Everything it needs comes from the shared platform env."*
Which secrets an agent may spend is admin-only; members see **Admins only.**

A `CONFIG_…` entry is a credential sealed out of the agent's own config when it was federated,
and the config refers to it by that name; the list says where (**used at**
`platforms.telegram.token`). Replace its value here to rotate it, then roll the agent. Removing
one the current config still uses removes that setting too: the confirm names it, and the
config gets a new version without it, so the agent never reads the `${CONFIG_…}` text itself as
the value. (The API refuses the delete with a 409 unless the caller asks for the setting to go
as well.) If one goes some other way, the render leaves out the setting that names it and warns,
until the secret is set again under that name. Hermes reads these values for itself; they are kept out of every command the agent
runs (its terminal, background processes and code execution), the way Hermes keeps its own
platform tokens out. That is a guard, not a wall: the values are still in the environment of
the agent's own process, which runs as the same user as its commands. An agent hired
from this one as a template takes these values along only when the person hiring manages this
agent (or is an admin); otherwise the clone is hired without them, and its first version's note
lists what was left out.

### MCP

The servers this agent carries. Chips say where each came from: **built-in** (the Talaria
toolkit, attached to every managed agent automatically) or **org registry** (assignment, tool
subsets, and credentials managed on the [MCP](./admin-mcp-observability-apps.md) page). Probe
states: `Connected` · `Login required` · `Unreachable` · `Error` — with **Test** to re-check.

Below the servers, **Hermes toolsets**: the tools the agent's own harness carries before any
MCP server (`browser`, `terminal`, `file`, `code_execution`, `web`, …) — exactly the switches
Hermes' own `hermes tools` offers, minus speech-to-text and the Discord and Yuanbao platform
tools no Talaria agent uses. Hermes' bundles (`safe`, `debugging`, `search`, `coding`) are not
rows: closing one closed its member tools while their own rows still read open, so a list that
named one now shows those members closed instead. A box is the toolset being on for this
agent; clearing one closes it from the agent's next turn, without a restart, and a new agent starts with the shell, code
execution, the file tool and self-scheduling closed until a manager opens them.
Two kinds of row are locked: `todo` and `clarify`, which Talaria's own tools replace for every
agent (Hermes' own todo list would also forget its plan every turn of an ordinary chat), and
anything an admin closed org-wide (**Admin → Security → Agent toolsets**, below).
The row's tooltip says which.

The `browser` row is Hermes' own browser, driving the Chromium in the agent's image, and it
works only with `terminal` on. With both on (the Developer Agent switch opens `terminal`), the
agent gets Hermes' `browser_exec`, which runs Python against the browser. With the terminal
closed (a new agent's default), the agent has no browser even with `browser` ticked, and the
line under the grid says why: on this image, Hermes' browser does not refuse private addresses
(the machine Talaria runs on, other agents' containers, your LAN), on the grounds that the
terminal reaches them anyway. That holds only for an agent that has a terminal, so only that
agent gets the browser. The agent's `web_extract` refuses those addresses either way. A browser
an agent's own config sets is named on that line too: with the terminal closed, Talaria closes
the `browser` toolset for one that Hermes runs locally, and leaves a CDP endpoint or a hosted
browser (which keep the check) alone.

The browser's engine is installed from npm onto the agent's volume before the agent starts
(once per Hermes version), so an agent whose host cannot reach the npm registry starts without
browser tools, and the app's log says why. A start waits at most 90 seconds for the install.
If it runs out of time, starts skip the install for the next 5 minutes. Turn `browser` and
`terminal` off for an agent that must not reach your network.

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
too, for the same reason as everywhere else. Saving restarts nobody: the fleet is re-rendered at
once and every running agent reads the floor on its next turn, which is what the panel says when
it saves. Names the api's
environment pins (`TALARIA_MANDATORY_DISABLED_TOOLSETS`) show locked as well, since unticking
them here would change nothing.

**Admin → Search → Agent web search** picks the web provider every agent searches with, through
its own Hermes: Tavily, Exa, Parallel, Firecrawl, Keenable, the Perplexity Search API, Brave
Search, or SearXNG at the workspace search URL, each with its key (stored encrypted, never shown
again), plus an optional separate provider for reading pages. Left on Hermes's default, Talaria
sets nothing. Once you pick a provider, or set a search URL (every agent gets it as
`SEARXNG_URL`), agents stop falling back to the anonymous free tiers of Exa, Parallel, Firecrawl
and Keenable unless you tick **Allow Hermes's keyless public tiers**. If the agents cannot connect
to that URL, their searches fail with SearXNG's connection error instead of falling back, so the
**Search URL** panel tests it twice: **from app** and **from agents**. The second runs the same
test inside a running agent, against the address the agents actually dial (a `127.0.0.1` URL
becomes `host.docker.internal`), and reads **not probed** when no agent is running, or when a
containerized install (`TALARIA_AGENT_DIAL=container`) has a loopback URL: there a `127.0.0.1`
address is the app's own container, so it is not handed to the agents at all and the panel says
so. A SearXNG listening only on the host's loopback shows up from the app and down from the
agents. A `SEARXNG_URL` in the api's environment wins over the field, which then shows it and
cannot be edited; set but empty counts as not set. Brave Search and SearXNG only search. Unless you also pick a
provider under **Read pages**, agents' `web_extract` lands on them and fails, and the panel warns
about it. The choice applies to every agent. It overrides an agent's own web backend settings, and
an agent secret with the same name as the vendor key. Research is not affected: it searches with
the search URL or a registered search MCP server. Unlike the toolset floor, a change here lands on
each agent only at its next roll: the vendor key is in the agent's environment.

Besides these panels, one rule is stamped into every agent's soul at hire and in every role template, so it holds no matter how the soul is later edited: **agents never do dev work in chat**. Code changes — however small — ship through a ticket and a workbench job; a dev-work request in chat produces (or links) a ticket and moves execution to the workbench, and instructions given in chat are posted to the ticket as comments.
