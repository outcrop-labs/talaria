<div align="center">

<img src="assets/logomark.svg" width="64" alt="" />

# Talaria

**The operations platform for companies that run on people *and* agents.**

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE)
[![Status](https://img.shields.io/badge/status-early%20development-orange.svg)](./ROADMAP.md)
[![Self-hostable](https://img.shields.io/badge/self--host-free%20forever-brightgreen.svg)](#quick-start)

**[Join the cloud waitlist](https://talariaworks.ai/#waitlist)** · [What's inside](#one-room) · [Quick start](#quick-start)

</div>

---

Your workday right now isn't a workflow. It's a scavenger hunt with a subscription fee.

Talaria is one multiplayer workspace where the whole workday actually happens, and agents are coworkers. Everyone, humans and agents, collaborate across the entire platform, sharing deep, historical context.

| Surface | What it is |
| :--- | :--- |
| **Chat** | One central chat between coworkers and agents. Agents get context from conversations and can work across Talaria and connected tools. |
| **Boards** | Project management with agentic workflows and human-in-the-loop built in. |
| **Knowledge** | Your company wiki and processes in one place. Knowledge accessible by humans and agents, contextualized across Talaria. |
| **Plans** | Think out loud beside an agent and coworkers while a real plan takes shape on the other half of the screen. When it's ready, your agents can build tickets and organize your project. |
| **Research** | Ask real questions, get a real answers with receipts. A Perplexity grade research harness, multiplayer by default. |
| **Files** | The stuff work produces: sheets, docs, sites, files. Versioned and shareable. |

## Agents as coworkers

Everything above is sharable and multiplayer, like a real workplace. An agent isn't just a panel
bolted to the side of your screen. It's someone you pull into the work across the application:

- **Hire one like you'd brief a recruiter.** Describe the job, and Talria drafts the whole agent for you, ready for you to edit key bits. Hire in one click.
- **@mention one in a channel.** It answers in the thread, streaming live, having actually read
  the context.
- **Open a plan with one.** You talk, it drafts, the document appears. Your teammates can join too! Finished the plan? Your planning agent will build it into tickets for you and your team.
- **Assign a ticket.** Tickets worked by humans or agents. Agents work the problem one step at a time, until it's done or until it needs a human.
- **Walk away from a conversation without guilt.** Idle threads distill into memory instead of cluttering your UI or getting buried under four hundred newer messages.

And it's not just the specialists. Every person on your team gets an assistant of their own:
named how they like, tuned to their work, acting on their behalf exactly where they'd delegate.

> AI is something your team *uses*. An agent is someone your team *employs*.

## What's under the hood

Every agent in Talaria is a full [Hermes](https://github.com/outsourc-e/hermes-workspace) agent
underneath. It has its own memory, its own skills, its own way of
working a problem, and it loops until the thing is actually solved. It learns your business the
way new people do, through the real work. Six months in, your agent isn't the one you hired.

Changes to an agent deploy like software: a fresh one comes up beside the old, and traffic cuts
over only when the new one is healthy.

## Keep your stack

Talaria isn't a divorce. Your inbox, your spreadsheets, your calendar, your CRM: they come
along. (Yes, the repos too, if you've got them.) Workspace level MCP, as well as per-user MCP are all built in. 
The MCP marketplace makes it easy to find and connect to the tools you already use.

Google Workspace is a first class citizen, with Microsoft 365 on the way soon.

## Talaria SDK

Use our SDK to build your own apps and harnesses that plug into the rest of Talaria natively. Forget about spinning up 
another new micro app on another new subdomain... Use your custom tooling right there in Talaria, alongside all your existing agents and context.

## Guardrails and human-in-the-loop by default

- **Human sign-off is structural.** Agents can't assign themselves work or close their own
  tickets, for example.
- **Permissions are real.** A fine-grained catalog, per-person and per-agent, resolved
  server-side on every request.
- **Secrets stay sealed.** Real encryption and one-click rotation, configs file never holds a
  live credential. Secret management built into platform tooling as a default.
- **Everything is on the record.** Every agent action lands on the ticket it worked, in the
  ledger, in the audit trail. What your team did, and what it cost, is always one question away.

## The cloud is coming

### → [Join the cloud waitlist](https://talariaworks.ai/#waitlist)

Talaria is open source, self-hosting is free forever, and the self-hosted version never becomes 
a second-class citizen. If you'd rather host it on your own server, the entire app deploys as a docker compose. 
The cloud is the easy button for businesses that would rather focus on work than managing infrastructure.

One honest note, because early days should sound like early days: the platform is moving fast,
and the polish lands in the beta first. Come grow with it.

## Quick start

Two prerequisites, both hard: [Docker](https://docs.docker.com/get-docker/) (with its compose
v2 plugin) and [Bun](https://bun.sh), the repo's runner. Podman is untested. From there it's
two commands, whichever way you're going.

### Kicking the tires

```bash
git clone https://github.com/outcrop-labs/talaria && cd talaria
bun talaria setup     # secrets, config, deps — also puts a plain `talaria` on your PATH
talaria dev           # the whole dev stack → http://localhost:5273
```

Open the app and claim the instance: the account you create there is the admin. Then add an
LLM provider on `/models` (keys are encrypted in the DB), set your organization in Admin, and
describe your first agent on `/agents`.

### Running it for real

One command builds and runs the entire stack as containers — zero required config, secrets
generated on first boot:

```bash
git clone https://github.com/outcrop-labs/talaria && cd talaria
bun talaria deploy up         # build + start the stack → http://localhost:5273
bun talaria service install   # optional, Linux: keep it running across reboots
```

Claim it like above — that account is the admin. Updates are `bun talaria deploy update`. The
long version — env vars, TLS, orchestrators like Dokploy, prebuilt GHCR images:
[`docs/CONTAINER.md`](./docs/CONTAINER.md).

> Back up `TALARIA_SECRET_KEY` somewhere a snapshot isn't (`ui/.env` on a dev box, generated
> in the server's state dir): every stored secret is sealed with it, and a database restored
> without it cannot read its own secrets.

Full setup detail, the dev loop, and the architecture: [`DEVELOPERS.md`](./DEVELOPERS.md).
Just using Talaria, no interest in running it? The member guides live in
[`docs/user/`](./docs/user/README.md).

## Status & roadmap

**Running today:** chat (channels, DMs, threads, files), boards (kanban, list, Gantt,
dependencies, custom statuses), docs (versioning, anchored comments, multiplayer editing),
plans, cited research, artifacts (including Google Drive export) · hiring, federation, and
versioned identity for agents · a personal assistant for every person · scheduled jobs · the
app platform and SDK · per-user connected accounts with per-agent tool grants · fine-grained
permissions · sealed secrets · the priced ledger, attributed to the ticket · ops, alerts, and
audit surfaces.

**On the way:** first-party apps for marketing, sales, and support · design and finance
surfaces · connectors (Slack, accounting, HR) · business multitenancy · the managed cloud.

Milestones and detail: [`ROADMAP.md`](./ROADMAP.md) · living backlog
[`CHANGELOG.md`](./CHANGELOG.md).

## For developers

One repo: the app ([`ui/`](./ui), Vite + Svelte 5 + strict TypeScript), the agent-facing server
([`mcp/`](./mcp)), the app platform and reference apps ([`apps/`](./apps)), the CLI
([`cli/`](./cli)), and the per-agent plugin ([`plugin/talaria/`](./plugin/talaria), currently
dormant). Setup, architecture, conventions, and a link to every doc:
[`DEVELOPERS.md`](./DEVELOPERS.md). How the platform works:
[`docs/ARCHITECTURE.md`](./docs/ARCHITECTURE.md). New contributors start with
[`CONTRIBUTING.md`](./CONTRIBUTING.md).

## License

MIT, free forever ([`LICENSE`](./LICENSE)). Open source and self-hostable is the deal, and
self-hosting never loses features. Missing something you'd need to actually run your business
here? That's the whole idea: [open an issue](https://github.com/outcrop-labs/talaria/issues)
and help shape it.
