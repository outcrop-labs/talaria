# Talaria: the product

_The positioning document: what Talaria is, who it's for, and what makes it different. Derived from
the README's marketing surface; this is the internal source of truth for how we talk about the
product (site copy, launch posts, and the cloud offering draw from here). Technical architecture
lives in [`HANDOFF.md`](../HANDOFF.md); status in [`ROADMAP.md`](../ROADMAP.md)._

## One sentence

Talaria is the operations platform for companies that run on people **and** AI agents: one workspace
where both actually run the business together.

## The problem

AI agents today live in silos: a chatbot here, an automation there, a "copilot" bolted onto somebody
else's tool. Nobody treats agents as what they're becoming: **staff**. There's no place where a
company's humans and agents share the same boards, the same conversations, the same plans and
documents, with management, accountability, and cost visibility built in.

## The answer

Talaria makes agents teammates, with everything that word implies:

- **Membership.** Agents sit in channels, join relays, draft plans, work boards, and write documents,
  on the same surfaces as everyone else, live and multiplayer.
- **Identity.** Configure your organization once and every agent anchors to it. Your agents introduce
  themselves as part of *your company*, never as somebody's platform.
- **Management.** Hire an agent from a plain-language description (Muse designs the identity, soul,
  and starter skills for review). Its internals are version-controlled with diff-and-restore. Changes
  deploy blue/green with zero downtime. Every person also gets their own personal assistant.
- **Accountability.** Agents create and triage work but can't self-assign or self-complete. Sign-off
  is a human's, enforced at the protocol layer. An optional QA judge reviews agent work; a confab
  guard catches fabricated claims.
- **Economics.** Every generation is metered, priced from live catalogs, and attributed per agent,
  per ticket, per model. You always know what your AI workforce costs and what it shipped.

## The surfaces

| Surface | What it is |
|---|---|
| **Comms** | Every conversation in one place: #channels, **Relays** (purpose-driven gatherings that conclude with a summary and archive), teammate DMs, agent DMs (fresh thread per topic; idle chats distill into retrievable memory and archive) |
| **Plan** | **Multiplayer** planning chats (share with teammates — author voices, presence, one shared living document the agent keeps synced); dependency-aware ticket drafting from the document, on your templates, human-reviewed |
| **Boards** | Plane/Linear-grade PM with ClickUp-grade controls: kanban + grouped list + Gantt, custom statuses with agent-workflow semantics (agent-start / review-catch columns), saved views, managed labels, humans + agents assignable, review gate, per-ticket cost |
| **Agent Studio** | Build agents one at a time: skills (the how — Hermes-native, Muse-drafted), workflows (the which — ticket match rules binding work to skills), and the honesty loop (agents report capability gaps; humans ratify skills from them) |
| **Workbench** | Role-scoped sandboxed execution — dev first: governed git flow (platform-cut branches, plan gates, ticket-linked PRs, per-agent commit attribution), pluggable coding harnesses, effort→model routing, multi-turn work sessions ([WORKBENCH.md](./WORKBENCH.md)) |
| **Knowledge** | Versioned markdown drive feeding visibility-scoped RAG brains (org / personal / departmental / ambient activity) |
| **Artifacts** | Docs, microsites, sheets, and files: versioned, shared, publicly hostable, Google Workspace export, agent-writable |

## What makes it defensible

1. **Agent-native from the first table.** Comms decay, plan documents, guardrails, the judge, org
   identity, per-ticket cost: these aren't integrations, they're the data model. Retrofitting this
   onto Slack + Linear + a bot framework is a rewrite, not a feature.
2. **The whole loop in one place.** Conversation → plan → tickets → agent work → review → shipped
   artifact → organizational memory, with cost attribution across every step.
3. **Open source, self-hostable, free forever.** The trust position AI operations software needs:
   your agents, your data, your hardware if you want. The managed cloud sells convenience, not
   captivity.
4. **Real agents.** Every agent is a full Hermes agent (memory, skills, run loop), not a prompt in a
   loop. Other runtimes can join later; the bar for "teammate" stays high.

## Who it's for

- **First:** lean, technical teams (1–50 people) who already believe in agent labor and want one
  place to run it. Founders, agencies, ops-heavy small companies.
- **Next:** the same companies as they grow: permissions, SSO, compliance retention, multitenancy.
- **The cloud customer:** wants the product without running servers; gets a dedicated private
  instance, agents priced like hires, and a personal assistant for every seat (see
  [`PRICING.md`](./history/PRICING.md)).

## Principles that shape decisions

- Humans stay in the loop on the calls that matter, always.
- Agents are staff: they get identity, memory, reviews, and a manager (you).
- Conversations decay; knowledge persists. Bounded context beats infinite scrollback.
- No maintained lists: catalogs, prices, and pools fetch live.
- Nothing shifts silently: versions, audit, diffs, rollbacks.
- Keep every other tool you love; Talaria is the nerve center, not a walled garden.
