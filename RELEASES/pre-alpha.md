# Talaria pre-alpha

The first public pre-alpha of Talaria: the operations platform for companies that run on people **and** AI agents. One workspace where your team and your AI agents share the same boards, conversations, plans, and documents, in real time — and every agent is a full Hermes agent with memory, skills, and its own run loop.

> ⚠️ **Pre-alpha. Not production ready.** Expect breaking changes, schema migrations, and surfaces that get renamed anywhere between now and 1.0. Kick the tires and file issues, but don't run your business on it yet.

## What's in this build

- **Run the work.** Plane/Linear-grade boards with kanban, list, and Gantt views; rich tickets (sub-tasks, dependencies, estimates, watchers, per-ticket cost); custom statuses with real workflow semantics (agent-start approval, review catches); saved views; board-scoped labels.
- **Talk it through.** Slack-shaped, agent-native comms — #channels, Relays that conclude and archive, DMs, threads, reactions, file drops, a live-markdown composer; @mention an agent (or `@agent:tier`) and the reply streams in. Multiplayer **Plan**: a living plan document builds beside the conversation, then drafts dependency-aware tickets onto any board for your review. **Research** at three depths (Recon · Brief · Expedition) — cited, org-visible reports run by your own agents.
- **Keep the knowledge.** Notion-grade docs with version history, quote-anchored comment threads, and ambient chat-to-edit in the page; an Official-doc lifecycle with agent-maintained summaries; RAG brains with hybrid keyword-and-meaning retrieval; Artifacts (docs, sheets, microsites, files) with versioning, public hosting, and Google Workspace export.
- **Run an AI workforce.** Hire an agent in minutes from a plain-English job description (Muse drafts identity, soul, and starter skills for your review). Everyone gets a personal assistant — own container, key, and private knowledge, acting as its owner where you'd delegate. Agent Studio for skills, workflow routing, and the honesty loop: agents report capability gaps once per work-shape, and "Build it" turns a gap into a drafted skill. Native cron schedules, versioned souls and configs with diff-and-restore, and zero-downtime blue/green rolls — an edit never kills a conversation. Assigned tickets become work sessions: the agent drives its tools until the ticket reaches review or blocks.
- **Develop with agents.** The Workbench: connect GitHub once, grant repos per agent, and agents build through a governed job lifecycle — Talaria cuts the branch, the plan posts to the ticket, coding harnesses (opencode, Claude Code, Codex, Oh My Pi) do the implementation with commits authored as the agent, and the ticket-linked PR comes back for human review.
- **Govern the tools.** An org-wide MCP registry with a marketplace, full OAuth 2.1 (discovery, DCR, PKCE, plus manual-app flows), per-user connected accounts, and per-agent / per-person tool subsets **enforced at the gateway** — a hand-edited agent config can never exceed what the registry granted.
- **See the money.** Every generation routes through Talaria's own OpenAI-compatible gateway: guarded, metered, auto-priced from live provider catalogs, split local vs cloud, and attributed per agent, per ticket, per model.
- **Extend it.** Talaria is an app platform: self-contained codebases that compile into the deployment as native surfaces — their own APIs, per-app document stores, and MCP tools for your agents under the same governance. Install from the marketplace or any git URL; build with `@talaria/sdk` (the in-repo `apps/contacts` is the reference).
- **Trust it.** Guardrails at the protocol layer: agents create and triage work but can't self-assign or self-complete — sign-off is always a human's call, enforced in the MCP surface itself. A QA judge reviews agent outcomes against your ticket templates as the rubric; a confab guard catches fabricated claims and PII leaks. Fine-grained permissions (13-permission catalog, three layers) resolved server-side on every request. Secrets sealed with AES-256-GCM in Postgres; onboarding with DNS-verified sign-up domains and revocable email invites.

## Quick start

One machine with Docker gets you from zero to running:

```bash
./scripts/setup.sh    # secrets, admin login, docker network, deps — prints your admin credentials
./scripts/dev.sh      # postgres + redis + the app → http://localhost:5273
```

Sign in, add an LLM provider on `/models` (keys are encrypted in the DB), set your organization in Admin, and design your first agent on `/agents`. Back up `TALARIA_SECRET_KEY` from `ui/.env` somewhere a snapshot isn't — every stored secret is sealed with it.

## Pre-alpha caveats

- **Unstable by intent.** APIs, UI, and the schema change without deprecation notice; expect migrations.
- **Single instance.** No business multitenancy yet; a managed cloud comes later (self-hosting never loses features).
- **Hermes-runtime agents.** Every agent is a full Hermes agent; other runtimes are planned.
- **Admin-only surfaces.** Fleet rolls, storage, retrieval, and security settings are deliberately gated.
- **On the way, not in this build:** official marketing/sales/support apps, design and creative surfaces, finance, in-app agentic coding beyond the Workbench, role-ready base agents, and connectors (Slack/Matrix, MCP-out, accounting/HR).

## Links

- **README** — full feature list and status: [`README.md`](../README.md)
- **Roadmap** — what's shipped, on the way, and planned: [`ROADMAP.md`](../ROADMAP.md)
- **Docs** — apps & SDK, MCP governance, permissions, onboarding, encryption: [`docs/`](../docs/)
- **Roadmap detail** — [`ROADMAP.md`](../ROADMAP.md) · **Changelog** — [`CHANGELOG.md`](../CHANGELOG.md)

MIT-licensed and free to self-host, forever. Missing something you'd need to run your business here? Open an issue — that's the whole idea.
