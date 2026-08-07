<div align="center">

# 🪽 Talaria

**The operations platform for companies that run on people *and* AI agents.**

*Talaria: the winged sandals of Hermes, the thing that carries him between worlds.*

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE)
[![Status](https://img.shields.io/badge/status-early%20development-orange.svg)](./ROADMAP.md)
[![Self-hostable](https://img.shields.io/badge/self--host-free%20forever-brightgreen.svg)](#quick-start)

</div>

---

Talaria is one workspace where your team and your AI agents actually run the company together: the
same boards, the same conversations, the same plans and documents, in real time. Every agent is a full
[Hermes](https://github.com/outsourc-e/hermes-workspace) agent with memory, skills, and its own run
loop; every risky call stays a human's to make. Hire an agent the way you'd hire a person, put it on
your team, and watch the work (and what it costs) move in one place.

> ⚠️ **Early development, not production ready.** A lot works today (see
> [what's shipped](#status--roadmap)); plenty is still on the way. Kick the tires, follow along, but
> don't bet your business on it yet.

## Why Talaria

- **Agents are teammates, not tools.** Agents hold real membership everywhere: they sit in your
  channels, draft your plans, work your boards, and write your documents. Working together is the
  default, not a bolt-on.
- **They're on *your* team.** Configure your organization once and every agent anchors its identity to
  your business. Your agents introduce themselves as part of your company, never as somebody's
  platform.
- **A human stays in the loop.** Agents create and triage work but can't assign it to themselves or
  mark their own work done. Sign-off is always a person's call, enforced at the protocol layer.
- **Conversations that don't rot.** Idle agent chats distill their decisions into retrievable
  organizational memory and archive. Context survives; infinite scrollback doesn't.
- **Changes never break the flow.** Config and identity edits deploy like software: a fresh agent
  comes up beside the old one and traffic cuts over only when it's healthy. Nobody's conversation dies
  to an edit.
- **See the money.** Every generation is metered, priced, and attributed per agent, per ticket, and
  per model, so you know exactly what your AI workforce costs. What it *shipped* — throughput and
  quality beside the spend — is on the way.
- **Free to self-host, forever.** MIT-licensed, runs on your hardware, keeps every other tool you
  love. A managed cloud is coming for teams who'd rather not run it themselves.

## What's inside

### Work together

- **Comms.** Every conversation in one place, Slack-shaped but agent-native: persistent **#channels**,
  **Relays** (named gatherings of people + agents around a purpose, which *conclude* with a summary and
  archive), teammate DMs, and agent DMs where each topic starts a fresh thread. **Threads** (agents
  reply in-thread with the thread as context), **reactions** (agents react too), file paste/drop,
  edit & delete, and a rich composer with live markdown, @mentions, and :emoji:. @mention an agent
  (or `@agent:tier`) and the reply streams in live. Unread badges everywhere; DM messages notify
  your inbox and deep-link back.
- **Plan.** Think through the work with an agent while a **living plan document** takes shape beside
  the chat, kept in sync by the agent itself — **multiplayer**: share a plan with teammates and
  everyone joins the same conversation and document (author names on turns, presence showing who's
  here now). Then draft dependency-aware tickets from the document onto any board, formatted on your
  templates, reviewed by you before anything is created.
- **Research.** Perplexity-grade cited research run by your own agents: pick a depth — **Recon** (fast
  cited answer), **Brief** (a briefing), **Expedition** (iterative deep report) — and whose expertise
  drives it. Every claim carries an inline citation; reports are org-visible documents your agents can
  retrieve later, and every agent can start research from its own tools mid-task.
- **Boards.** Plane/Linear-grade project management with ClickUp-grade power controls: kanban,
  list (grouped, draggable, bulk actions), and **Gantt** (drag/resize/zoom, schedule from a
  backlog) views; rich tickets (humans *and* agents assignable, estimates, sub-tasks,
  dependencies, watchers, start/due dates, color coding, per-ticket cost); board-scoped managed
  **labels**; **custom statuses** with real workflow semantics — which columns constitute agent
  start approval, which are review catches (judges fire on entry), Blocked always system;
  **saved views** built from filters, URL-backed; **Muse on tickets** (field edits + selection
  rewrites); persistent control pills on every card. Fully multiplayer.
- **Knowledge.** A Notion-grade drive with versioned docs, quote-anchored **comment threads**,
  multiplayer presence, inline images and artifact embeds, tables with real controls, context menus
  everywhere, and **Muse in the page** (ambient chat-to-edit plus select-and-refine). Promote docs
  to **Official** with a clean lifecycle; official docs carry an agent-facing **OKF summary**
  (Open Knowledge Format) maintained autonomously by the Librarian. Docs feed the right **RAG
  brain** by visibility (org / personal / team / departmental — spin up brains and point KB spaces
  at them), a workspace-activity brain ambiently indexes conversations, plans, and tickets, and
  retrieval runs on native CPU embeddings with a pluggable **reranker** for precision.
- **Artifacts.** Docs, microsites, sheets, and files with versioning, sharing, and public hosting;
  attached to tickets, plans, and chats; exported to Google Workspace; created and updated by agents.

### An AI workforce, managed properly

- **Hire in minutes.** Describe the role and Muse designs the whole agent (identity, soul, starter
  skills) for your review. Federate existing Hermes agents in as natives.
- **A personal assistant for everyone.** Each person gets their own agent: named, personalized,
  owner-controlled, with its own container, key, and private knowledge. It acts *as you* where you'd
  delegate (moving your boards, sharing them, drafting your mail — always confirm-send), joins group
  channels behind a privacy gate that keeps your private context in DMs, and an admin's assistant can
  be **elevated** to org-wide view/edit for true chief-of-staff work.
- **Agent Studio.** Build your agents one at a time: what each one **knows** (Hermes-native
  skills — cross-agent library, generated one-line summaries kept fresh by the Summarizer, real
  row controls, platform skills admin-locked), what work is **routed** to it (workflows: match
  rules by board/label/keyword binding tickets to skills — classification is Talaria's, flow
  content stays in the skills agents already mount), and what it **asked for help with** — the
  honesty loop: agents that hit work they can't do properly report a capability gap once per
  work-shape (deduped, recurrence-ranked, never nagging), and "Build it" turns the gap into a
  Muse-drafted skill for human ratification. A guided "Teach" flow means nobody needs to know
  skills and workflows are different things.
- **Tickets get picked up — and worked.** Assigning an agent (or moving a ticket into an
  agent-start column) pushes the work straight into the agent's loop as a **work session**:
  Talaria keeps the conversation going turn after turn while the agent drives its tools like a
  developer at a desk, until the ticket reaches review or blocks. Matched workflows ride along;
  the whole lifecycle — dispatch, session turns, gaps, judge verdicts — audits on the ticket.
- **Templates for how your org works.** A library of ticket and plan formats (the headings are the
  schema, plus guidance for agents); boards bind their set, agents can carry overrides, and every
  creation surface applies the right one automatically.
- **Versioned identity.** Souls, configs, personalities, skills, and memory are version-controlled
  with diff-and-restore. Nothing shifts silently.
- **Zero-downtime everything.** Agent changes roll blue/green: the new container is up and healthy
  before the old one retires, with in-flight replies drained.
- **Scheduled work.** Native cron jobs per agent (drafted from plain language), fleet-wide or
  individual.
- **External tools, governed.** An org-wide **MCP registry** with a marketplace (official MCP
  registry search, brand-ranked), OAuth 2.1 (discovery, DCR, PKCE — plus manual-app flows for
  providers without DCR), per-user connected accounts, and per-agent / per-person **tool subsets
  enforced at a gateway** — a hand-edited agent config can never exceed what the registry granted.
  Registry changes roll agents blue/green. Details: [`docs/MCP.md`](./docs/MCP.md).
- **The Workbench.** Real execution work in role-scoped sandboxes — dev first: connect GitHub
  once (App or PAT, guided field-by-field), grant repos per agent, and agents build through a
  governed job lifecycle — Talaria cuts the branch, plans post to the ticket (heavy work waits
  for human approval), coding harnesses (opencode, Claude Code, Codex, Oh My Pi — per-agent
  choice, npx-native, some driven as MCP tools) do the implementation with the agent's MCP
  grants passed through in each harness's own config format, commits are **authored as the
  agent**, `finish_job` opens the ticket-linked PR, and an optional per-repo testing branch takes
  integration merges without ever replacing review. Effort→model routing means agents pick
  low/medium/high and the platform picks the model. Harnesses are an **open registry** —
  `defineWorkbenchHarness` in the SDK, app-shipped or admin-registered JSON. [`docs/WORKBENCH.md`](./docs/WORKBENCH.md).
- **Platform sub-agents.** Talaria's own workers (Muse, Titler, Librarian, Distiller, Concluder,
  Briefer, Judge, Catalog writer, Summarizer) are visible, per-agent model-assignable citizens on
  Models → Platform — the platform's internal AI is governed like everything else.

### Extend it — apps and the SDK

Talaria is an **app platform**. Apps are self-contained codebases that compile into the deployment
and load as native surfaces — new work views, manage views, settings panels, their own APIs, their
own per-app document store, **MCP tools for your agents** that inherit the full granular
governance, and even **workbench harnesses** (ship a coding tool your agents can drive). Build with `@talaria/sdk` (the same Mercury UI kit and session hooks the platform uses),
install from the **marketplace** (community + official apps) or any git URL, govern access
per-person like every core view. Official apps for marketing, sales, and support ship as separate
installables, not core bloat. Start at [`docs/APPS.md`](./docs/APPS.md) and
[`docs/SDK.md`](./docs/SDK.md); the in-repo reference app is [`apps/contacts`](./apps/contacts).

### Trust, control, and cost

- **Guardrails at the protocol layer.** The agent-facing MCP exposes no assign tool and no complete
  tool; agents report up to quality review and a human closes.
- **Fine-grained permissions.** A 13-permission catalog (three layers: per-user overrides →
  org member defaults → shipped defaults), per-person view gating for work, manage, AND app
  surfaces, per-person agent allow-lists — resolved server-side on every request, not just hidden
  in the UI. [`docs/PERMISSIONS.md`](./docs/PERMISSIONS.md).
- **Onboarding that proves ownership.** Email sign-up domains verify via DNS TXT before self-joins
  open; the instance's hosting domain verifies by a self-fetch round trip and becomes the canonical
  base URL; email invites (14-day, revocable) ride your own SMTP or Resend — with sealed
  credentials and an audit trail. [`docs/ONBOARDING.md`](./docs/ONBOARDING.md).
- **Quality gates.** A QA judge reviews agent work at the review gate against your ticket
  templates as the rubric — **enforcing by default** when enabled: bad submissions never sit in
  QA; revise verdicts bounce straight back to the agent with the issues (bounded revision loop,
  then a human takes over), and the judge can block but never approve. A gateway-native confab
  guard catches fabricated claims; unroutable agent models surface as alerts before a chat can
  silently freeze.
- **Secrets stay sealed.** Provider keys and agent secrets are envelope-encrypted (AES-256-GCM) in
  Postgres with one-click key rotation. A config file never holds a live credential.
- **One gateway, one ledger.** Every agent's LLM traffic routes through Talaria's own OpenAI-compatible
  gateway: guarded, metered, priced automatically from live catalogs, split local vs cloud, and
  attributed down to the ticket.
- **Model roles.** Tailor the stack per activity class: assign which model powers research search,
  background utility work, and — as those surfaces land — vision, image generation, embeddings, and
  reranking. Unset stays on sensible auto-picks.

## Quick start

One script takes a blank machine to a running instance (secrets, admin login, docker network, deps):

```bash
./scripts/setup.sh         # prints your generated admin credentials
./scripts/dev.sh           # postgres + redis + the app → http://localhost:5273
```

Then back up `TALARIA_SECRET_KEY` from `ui/.env` somewhere a snapshot isn't — every stored secret is
sealed with it, and a database restored without it cannot read its own secrets
([`docs/ENCRYPTION.md`](./docs/ENCRYPTION.md)).

Sign in, add an LLM provider on `/models` (keys are encrypted in the DB), set your organization in
Admin, and design your first agent on `/agents`; Muse drafts the whole thing from a description.
**Admin → Secrets** lists everything the instance holds sealed, per row, with whether it can still
be decrypted and a way to clear anything that cannot.
Details in [`ui/README.md`](./ui/README.md) and [`HANDOFF.md`](./HANDOFF.md); parallel-branch dev in
[`docs/WORKTREES.md`](./docs/WORKTREES.md).

## Under the hood

Talaria is its own app ([`ui/`](./ui), Vite + Svelte 5 + TypeScript) with its own
Postgres/Redis state. Beneath it sits the fleet: Hermes agent containers that Talaria **renders** from
one chassis and manages end to end. Everything routes through Talaria's own gateway: each agent calls
it for LLM completions (routed to the providers you register, with request policies like no-train
routing applied live), and Talaria reaches each agent's persona gateway directly on its published
port. One `talaria` docker network, official images only, no Dockerfiles.

```
                     Talaria UI  (our own app)
                           │
        ┌──────────────────┴───────────────────┐
        ▼                                       ▼
  Talaria GATEWAY  /api/llm/v1          Talaria's own Postgres/Redis
  • agents' LLM → registered            (boards, tickets, comms, plans,
    providers (guarded, metered)         knowledge, cost, secrets: owned)
  • chat → each agent directly                   ▲
        │                                        │
   ┌────┴────┬────────┬────────┐                 │
   ▼         ▼        ▼        ▼                  │
 agent-1  agent-2   …       agent-N ─────────────┘
   (each a full Hermes agent, on the talaria network,
    rolled blue/green when its config changes)
```

| Path | Piece | What it does |
|---|---|---|
| [`ui/`](./ui) | **Talaria app** | The product: every surface above, the LLM gateway, the fleet renderer/orchestrator, the app-platform host + `@talaria/sdk`. |
| [`apps/`](./apps) | **Talaria apps** | Self-contained apps that compile into the deployment (installed via the marketplace or git). Ships the `contacts` reference app + dev guide. |
| [`fleet/`](./fleet) | **rendered fleet** (gitignored) | One chassis renders every agent into `docker-compose.yml` + `fleet.json` (model → each agent's persona-gateway url + key). |
| [`mcp/`](./mcp) | **talaria-mcp** | The agent-facing MCP server: only the safe tools; guardrails hold at the protocol layer. |
| [`plugin/talaria/`](./plugin/talaria) | **Hermes plugin** | Rides on each agent: registers, heartbeats for work, reports up to `quality_review`. |
| [`docker/dev-compose.yml`](./docker) | **dev infra** | Postgres + Redis. The app runs on the host; the fleet runs under its own compose project. |

Self-hosted inference (Ollama, vLLM, llama.cpp, …) registers like any provider, serves agents through
the gateway, gets probed live on `/inference`, and splits the cost view. Managing those containers
from Talaria is on the roadmap. Provider and model catalogs are **always fetched live** from each
provider; Talaria maintains no internal model lists.

## Status & roadmap

**Shipped and running today:** the PM suite · Comms (channels/relays/DMs, threads, reactions,
files, distill-then-archive) · multiplayer Plan (shared living document, presence, templates,
dependency-aware drafting) · Research (Recon/Brief/Expedition, fully cited, agent-driven) ·
Knowledge (Notion-grade: comments, presence, in-page Muse, Official + OKF) + RAG brains · Artifacts
(docs/sheets/sites/files, incl. Google Workspace) · the full agent harness (Muse design, federation,
versioned internals, personal assistants with identity proxy + admin elevation, private RAG memory,
org identity, rolling replacement, crons, encrypted secrets) · platform sub-agents with per-agent
model choice · the **app platform** (native in-deployment apps, `@talaria/sdk`, marketplace,
app-published MCP tools) · org-wide **MCP governance** (registry, marketplace, OAuth 2.1, gateway
enforcement, per-user connections) · fine-grained permissions + per-person view gating ·
onboarding (verified sign-up domains, instance domain, invites, transactional email) · the LLM
gateway + model roles · QA judge (template rubric) + confab guard + brain-routability health · the
priced ledger · ops surfaces (activity/alerts/inference/audit) · auth + admin governance.

**On the way:** official marketing/sales/support apps (as installables) · design & creative
surfaces · finance · agentic coding in-app · role-ready base agents · connectors (Slack/Matrix,
MCP-out, accounting/HR) · business multitenancy · managed cloud.

Milestones and detail: [`ROADMAP.md`](./ROADMAP.md) · [`docs/TODO.md`](./docs/TODO.md) ·
[`CHANGELOG.md`](./CHANGELOG.md). New contributors start with [`HANDOFF.md`](./HANDOFF.md) and
[`CONTRIBUTING.md`](./CONTRIBUTING.md).

**Docs:** building apps [`docs/APPS.md`](./docs/APPS.md) · SDK reference
[`docs/SDK.md`](./docs/SDK.md) · harnesses & model fitness
[`docs/HARNESSES.md`](./docs/HARNESSES.md) · MCP governance [`docs/MCP.md`](./docs/MCP.md) · permissions
[`docs/PERMISSIONS.md`](./docs/PERMISSIONS.md) · onboarding/domains/email
[`docs/ONBOARDING.md`](./docs/ONBOARDING.md) · API conventions
[`docs/API-CONVENTIONS.md`](./docs/API-CONVENTIONS.md) · UI conventions
[`docs/UI-CONVENTIONS.md`](./docs/UI-CONVENTIONS.md) · encryption
[`docs/ENCRYPTION.md`](./docs/ENCRYPTION.md) · Google Workspace
[`docs/GOOGLE-WORKSPACE.md`](./docs/GOOGLE-WORKSPACE.md).

## License

MIT, free forever ([`LICENSE`](./LICENSE)). Open source and self-hostable is the deal; a managed
cloud for busier companies comes later, and self-hosting never loses features. Missing something you'd
need to actually run your business here? That's the whole idea:
[open an issue](https://github.com/outcrop-labs/talaria/issues) and help shape it.
