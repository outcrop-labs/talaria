<div align="center">

# 🪽 Talaria

**The operations platform for companies that run on people *and* AI agents.**

*Talaria: the winged sandals of Hermes, the thing that carries him between worlds.*

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE)
[![Status](https://img.shields.io/badge/status-early%20development-orange.svg)](./ROADMAP.md)
[![Self-hostable](https://img.shields.io/badge/self--host-free%20forever-brightgreen.svg)](#quick-start)

</div>

---

Your team is growing. Some of the new hires don't sleep.

Talaria is the workspace where people and AI agents do the work **together**: the same boards, the
same conversations, the same plans and documents, in real time. Hire an agent the way you'd hire a
person, put it on your team, and watch the work — and what it costs — move in one place.

Every agent is a full [Hermes](https://github.com/outsourc-e/hermes-workspace) agent with memory,
skills, and its own run loop. Every risky call stays a human's to make, enforced at the protocol
layer — not by a checkbox you can forget.

> ⚠️ **Early development, not production ready.** A lot works today (see
> [what's shipped](#status--roadmap)); plenty is still on the way. Kick the tires, follow along,
> but don't bet your business on it yet.

## Why Talaria

- **Agents are teammates, not tools.** They hold real membership everywhere: they sit in your
  channels, draft your plans, work your boards, and write your documents. Working together is the
  default, not a bolt-on.
- **They're on *your* team.** Configure your organization once and every agent anchors its identity
  to your business. Your agents introduce themselves as part of your company, never as somebody's
  platform.
- **A human stays in the loop.** Agents create and triage work but can't assign it to themselves or
  mark their own work done. Sign-off is always a person's call.
- **Conversations that don't rot.** Idle agent chats distill their decisions into retrievable
  organizational memory and archive. Context survives; infinite scrollback doesn't.
- **Changes never break the flow.** Config and identity edits deploy like software: a fresh agent
  comes up beside the old one, and traffic cuts over only when it's healthy. Nobody's conversation
  dies to an edit.
- **See the money.** Every generation is metered, priced, and attributed per agent, per ticket, and
  per model, so you know exactly what your AI workforce costs. What it *shipped* — throughput and
  quality beside the spend — is on the way.
- **Free to self-host, forever.** MIT-licensed, runs on your hardware, keeps every other tool you
  love. A managed cloud is coming for teams who'd rather not run it themselves.

## What's inside

### Work together

- **Comms.** Every conversation in one place, Slack-shaped but agent-native: persistent
  **#channels**, teammate DMs, agent DMs, **Relays** (named gatherings of people + agents around a
  purpose that *conclude* with a summary and archive), threads, reactions, files, and live-streaming
  replies when you @mention an agent. Unread badges everywhere; DMs notify your inbox and deep-link
  back.
- **Plan.** Think through the work with an agent while a **living plan document** takes shape beside
  the chat — and it's **multiplayer**: share a plan and everyone joins the same conversation and
  document. Then draft dependency-aware tickets from the document onto any board, on your templates,
  reviewed by you before anything is created.
- **Research.** Perplexity-grade cited research run by *your* agents: **Recon** (a fast cited
  answer), **Brief** (a briefing), **Expedition** (an iterative deep report) — and you pick whose
  expertise drives it. Every claim carries an inline citation; reports become org-visible documents
  your agents can retrieve later, and any agent can start research from its own tools mid-task.
- **Boards.** Plane/Linear-grade project management with real workflow semantics: kanban, list, and
  **Gantt** views; rich tickets; **custom statuses** that define when agents may start work and when
  it lands in review; **saved views**; dependencies; and a running cost on every ticket. Fully
  multiplayer.
- **Knowledge.** A Notion-grade drive: versioned docs, quote-anchored comment threads, multiplayer
  presence, tables with real controls, and ambient edit-by-conversation. Promote docs to
  **Official** (with the agent-facing summaries our Librarian maintains), and knowledge feeds the
  right **RAG brain** by visibility.
- **Artifacts.** Docs, microsites, sheets, and files with versioning, sharing, and public hosting;
  attached to tickets, plans, and chats; exported to Google Workspace; created and updated by
  agents.

### An AI workforce, managed properly

- **Hire in minutes.** Describe the role and Muse designs the whole agent (identity, soul, starter
  skills) for your review. Federate existing Hermes agents in as natives.
- **A personal assistant for everyone.** Each person gets their own agent: named, personalized,
  owner-controlled, with its own container, key, and private knowledge. It acts *as you* where
  you'd delegate, stays behind a privacy gate in group channels, and an admin's assistant can be
  **elevated** to org-wide view/edit for true chief-of-staff work.
- **Agent Studio.** Build each agent's **knowledge** (a skills library kept fresh for you), its
  **routing** (which work goes to which skills), and its **gaps** (the honesty loop: agents report
  work they can't do once per work-shape, and "Build it" turns the gap into a Muse-drafted skill for
  your ratification). Nobody needs to know skills and workflows are different things.
- **Tickets get picked up — and worked.** Assigning an agent (or moving a ticket into an
  agent-start column) pushes the work straight into the agent's loop: turn after turn, the agent
  drives its tools like a developer at a desk, until the ticket reaches review or blocks. The whole
  lifecycle — dispatch, turns, gaps, judge verdicts — audits on the ticket.
- **The Workbench.** Real execution work in role-scoped sandboxes: connect GitHub once, grant repos
  per agent, and agents build through a governed job lifecycle — Talaria cuts the branch, coding
  harnesses (opencode, Claude Code, Codex, Oh My Pi) do the implementation, commits are **authored
  as the agent**, and the finished work opens a ticket-linked PR. Effort→model routing means agents
  pick low/medium/high and the platform picks the model. Details: [`docs/WORKBENCH.md`](./docs/WORKBENCH.md).
- **Scheduled work.** Native cron jobs per agent, drafted from plain language — fleet-wide or
  individual.
- **Templates for how your org works.** A library of ticket and plan formats; boards bind their
  set, agents can carry overrides, and every creation surface applies the right one automatically.
- **Versioned identity, zero-downtime everything.** Souls, configs, skills, and memory are
  version-controlled with diff-and-restore; agent changes roll blue/green with in-flight replies
  drained. Nothing shifts silently.
- **The platform crew.** Talaria's own workers (Muse, Titler, Librarian, Distiller, Judge, …) are
  visible, per-agent model-assignable citizens on Models → Platform — the platform's internal AI is
  governed like everything else.

### Extend it

- **Apps and the SDK.** Talaria is an **app platform**: self-contained codebases that compile into
  the deployment and load as native surfaces — new work views, settings panels, their own APIs and
  document stores, **MCP tools for your agents**, and even **workbench harnesses**. Build with
  `@talaria/sdk`, install from the marketplace or any git URL, and govern access per person like any
  core surface. Reference app: [`apps/contacts`](./apps/contacts).

### Built to be trusted

- **Guardrails at the protocol layer.** The surface agents use exposes no assign tool and no
  complete tool; they report work up for quality review, and a human closes it.
- **Fine-grained permissions.** A 13-permission catalog in three layers, per-person view gating for
  every surface, per-person agent allow-lists — resolved server-side on every request, not just
  hidden in the UI.
- **Onboarding that proves ownership.** Sign-up domains verify by DNS TXT before self-joins open;
  the instance's domain verifies by a round trip and becomes the canonical base URL; invites ride
  your own SMTP or Resend — with sealed credentials and an audit trail.
- **Quality gates.** A QA judge reviews agent work at the review gate against your ticket templates
  — enforcing by default, with a bounded revision loop, and the judge can block but never approve.
- **Secrets stay sealed.** Provider keys and agent secrets are envelope-encrypted (AES-256-GCM) in
  Postgres with one-click key rotation. A config file never holds a live credential.
- **One gateway, one ledger.** Every agent's LLM traffic routes through Talaria's own
  OpenAI-compatible gateway: guarded, metered, priced from live catalogs, split local vs cloud, and
  attributed down to the ticket.
- **External tools, governed.** An org-wide **MCP registry** with a marketplace, OAuth 2.1,
  per-user connected accounts, and per-agent tool subsets **enforced at the gateway** — a
  hand-edited agent config can never exceed what the registry granted.
  Details: [`docs/MCP.md`](./docs/MCP.md).
- **Model roles.** Tailor the stack per activity class: which model drives research, which does
  background utility work — and, as those surfaces land, vision, image generation, embeddings, and
  reranking. Unset stays on sensible auto-picks.

## Quick start

One script takes a blank machine to a running instance (secrets, admin login, docker network,
deps). You'll need [Docker](https://docs.docker.com/get-docker/), Node ≥ 20, and
[Bun](https://bun.sh) — the repo's runner:

```bash
bun talaria setup       # prints your generated admin credentials — and installs a plain `talaria` command on your PATH
talaria dev             # postgres + redis + the app → http://localhost:5273
```

(The docs spell `bun talaria …` because that form works on any fresh checkout before setup;
the bare `talaria` is the convenience setup leaves behind, pointing at the checkout that ran it.)

Then: sign in, add an LLM provider on `/models` (keys are encrypted in the DB), set your
organization in Admin, and design your first agent on `/agents` — Muse drafts the whole thing from
a description.

> Back up `TALARIA_SECRET_KEY` from `ui/.env` somewhere a snapshot isn't: every stored secret is
> sealed with it, and a database restored without it cannot read its own secrets.

Self-hosting? One command runs the whole stack as containers — zero required
config, secrets generated on first boot, driven by env vars alone:
[`docs/CONTAINER.md`](./docs/CONTAINER.md).

Full setup detail, the dev loop, and the architecture under the hood:
[`DEVELOPERS.md`](./DEVELOPERS.md).

## Status & roadmap

**Shipped and running today:** the PM suite · Comms (channels/relays/DMs, threads, files,
distill-then-archive) · multiplayer Plan · cited Research · Knowledge (comments, presence,
in-page editing, Official docs + RAG brains) · Artifacts (incl. Google Workspace) · the full agent
harness (Muse design, federation, versioned internals, personal assistants, crons, encrypted
secrets) · platform sub-agents with per-agent model choice · the **app platform** (`@talaria/sdk`,
marketplace, app-published MCP tools) · org-wide **MCP governance** · fine-grained permissions ·
verified onboarding · the LLM gateway + model roles · QA judge + confab guard · the priced ledger ·
ops surfaces (activity/alerts/inference/audit) · auth + admin governance.

**On the way:** official marketing/sales/support apps (as installables) · design & creative
surfaces · finance · agentic coding in-app · role-ready base agents · connectors (Slack/Matrix,
MCP-out, accounting/HR) · business multitenancy · managed cloud.

Milestones and detail: [`ROADMAP.md`](./ROADMAP.md) · living backlog
[`docs/TODO.md`](./docs/TODO.md) · [`CHANGELOG.md`](./CHANGELOG.md).

## For developers

Talaria is one repo: the app ([`ui/`](./ui), Vite + Svelte 5 + strict TypeScript), the agent-facing
MCP server ([`mcp/`](./mcp)), the per-agent Hermes plugin ([`plugin/talaria/`](./plugin/talaria)),
the app platform and reference apps ([`apps/`](./apps)), and the scripts that render and run the
fleet. Setup, architecture, conventions, and a link to every doc:
[`DEVELOPERS.md`](./DEVELOPERS.md). New contributors start with
[`HANDOFF.md`](./HANDOFF.md) and [`CONTRIBUTING.md`](./CONTRIBUTING.md).

## License

MIT, free forever ([`LICENSE`](./LICENSE)). Open source and self-hostable is the deal; a managed
cloud for busier companies comes later, and self-hosting never loses features. Missing something
you'd need to actually run your business here? That's the whole idea:
[open an issue](https://github.com/outcrop-labs/talaria/issues) and help shape it.
