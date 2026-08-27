<div align="center">

# 🪽 Talaria

**The operations platform for companies that run on people *and* agents.**

*Talaria: the winged sandals of Hermes, the thing that carries him between worlds.*

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE)
[![Status](https://img.shields.io/badge/status-early%20development-orange.svg)](./ROADMAP.md)
[![Self-hostable](https://img.shields.io/badge/self--host-free%20forever-brightgreen.svg)](#quick-start)

**[Join the cloud waitlist](https://talariaworks.ai/#waitlist)** · [What's inside](#one-room) · [Quick start](#quick-start)

</div>

---

## The workday, honestly

It's 9:47 on a Tuesday. Eleven tabs. Six notifications. One vague memory of a decision that was
made somewhere: a thread? a call? a doc? The answer exists, in one of your apps, possibly the one
you're paying for and forgot about.

And somewhere under all of it is the actual job. The part you're good at. The part you came for.

Your workday isn't a workflow. It's a scavenger hunt with a subscription fee.

## One room

Talaria is one live workspace where the whole day actually happens. And everybody's in it,
including the new hires who don't sleep.

| Surface | What it is |
| :--- | :--- |
| **Chat** | You know chat. You have four apps for it. This one has teammates who can also read the ticket, the doc, and the sheet, and answer in the thread, live, while you watch. |
| **Boards** | Columns with convictions. Statuses mean something here, including exactly when an agent may start work and when it has to stop and wait for a person. |
| **Docs** | Docs with a memory. Every version, every decision, every comment pinned to the sentence it's actually about. |
| **Plans** | Think out loud beside an agent while a real plan takes shape on the other half of the screen; the whole team is in one conversation and the document builds itself beside it. When it's ready, it becomes tickets on the board, reviewed by you before anything moves. |
| **Research** | Ask a real question, get a real answer, with receipts. Every claim carries its source, and the report files itself where the whole company can find it forever. |
| **Artifacts** | The stuff work produces: sheets, docs, sites, files. Versioned, shareable, hosted, and exportable to the tools your accountant already loves. |

One login. One search. Zero scavenger hunt. And even if you never hire an agent, this alone is a
better day at work.

## Agents in the room

Everything above is shared and live, the way a real workplace is. So an agent isn't a panel
bolted to the side of your screen. It's someone you pull into the work:

- **Hire one like you'd brief a recruiter.** Describe the job; Talaria's own hiring manager
  drafts the whole agent for your red pen, starter skills and all.
- **@mention one in a channel.** It answers in the thread, streaming live, having actually read
  the context.
- **Open a plan with one.** You talk, it drafts, the document appears. Your teammates join both
  halves.
- **Assign a ticket.** It works the problem one step at a time, until it's done or until it needs
  a human. Everything it did is on the ticket when you review it.
- **Walk away from a conversation without guilt.** Idle threads distill into company memory
  instead of rotting under four hundred newer messages.

And it's not just the specialists. Every person on your team gets an assistant of their own:
named how they like, tuned to their work, acting on their behalf exactly where they'd delegate.
The meeting notes write themselves. The Friday digest assembles itself. The awkward follow-up
email gets drafted; you press send.

> AI is something your team *uses*. An agent is someone your team *employs*.

## The grind has somewhere to go

Every job is two jobs. The work you were hired for: the judgment, the craft, the customers, the
wins. And everything that piled on top of it: the queue-watching, the chasing, the copying
between apps, the follow-up nudge at 4:55.

Talaria gives the pile somewhere to go. A teammate who never minds it. The 11pm queue becomes
nobody's problem. The first pass on anything, a draft, a summary, a brief, is already done when
you sit down. What comes back to you is the part where you're actually needed: the decision, the
taste, the relationship, the call.

The agents learn the business, a little more every day. The people learn to delegate, which, be
honest, is just another word for leading. The company learns to remember. Six months in,
everybody's operating one level up.

Same headcount. More headroom.

## What's under the hood

Every agent in Talaria is a full [Hermes](https://github.com/outsourc-e/hermes-workspace) agent
underneath. The ops hire, the research crew, the personal assistant each person gets. All of
them. Not a script wearing a name tag: it has its own memory, its own skills, its own way of
working a problem, and it loops until the thing is actually solved. It learns your business the
way new people do, through the real work. Six months in, your agent isn't the one you hired.

Then again, neither is your team.

Changes to an agent deploy like software: a fresh one comes up beside the old, and traffic cuts
over only when the new one is healthy. Nobody's conversation dies to an edit.

## Keep your stack

Talaria isn't a divorce. Your inbox, your spreadsheets, your calendar, your CRM: they come
along. (Yes, the repos too, if you've got them.)

Each person links their own Google Workspace and GitHub accounts, plus the long tail of
everything else through the built-in connector. Each agent gets exactly the tools you grant it,
and not one more. That part is enforced by the platform, not by hope. Your agent updates the
sheet, sends the digest, chases the follow-up. It writes the code when there's code to write,
with the pull request already attached to the ticket.

Your stack stays your stack. Now it just has hands.

## Build it, or install it

Every company has that one dashboard. You know the one. The thing that would make everyone's day
20% easier if it existed, and nobody's ever had a spare quarter to build it.

**Today:** a little dev work and the [`@talaria/sdk`](./apps), and it's real. A native Talaria
app with its own surface, its own data, its own tools for your agents, governed like everything
else. A weekend project, not a quarter.

**Tomorrow:** the app marketplace. Somebody else's Saturday project becomes your Monday install.
The platform grows because the people growing it use it too.

## The rules are in the walls

The rules are in the walls, not in a policy doc:

- **Human sign-off is structural.** Agents can't assign themselves work or close their own
  tickets. The platform simply has no door for it.
- **Permissions are real.** A fine-grained catalog, per-person and per-agent, resolved
  server-side on every request. Not just hidden in the interface.
- **Secrets stay sealed.** Real encryption and one-click rotation. A config file never holds a
  live credential.
- **Everything is on the record.** Every agent action lands on the ticket it worked, in the
  ledger, in the audit trail. What your team did, and what it cost, is always one question away.

## The cloud is coming

The whole platform, and none of the plumbing. We keep it running, updated, and awake (keeping
things awake is kind of our thing), and you keep your attention on the business. The open beta
opens soon, and the waitlist is open now. First seats go to the people already in line.

### → [Join the cloud waitlist](https://talariaworks.ai/#waitlist)

And since you were wondering: yes, Talaria is open source. The repo is public, self-hosting is
free forever, and the self-hosted version never becomes the second-class one. If you'd rather
run it on your own hardware, the setup script takes about ten minutes and the license is MIT.
The cloud is just the easy button.

One honest note, because early days should sound like early days: the platform is moving fast,
and the polish lands in the beta first. Come grow with it.

## Quick start

One script takes a blank machine to a running instance (secrets, admin login, docker network,
deps). You'll need [Docker](https://docs.docker.com/get-docker/), Node ≥ 20, and
[Bun](https://bun.sh), the repo's runner:

```bash
bun talaria setup       # prints your generated admin credentials, and installs a plain `talaria` command on your PATH
talaria dev             # postgres + redis + the app → http://localhost:5273
```

Then: sign in, add an LLM provider on `/models` (keys are encrypted in the DB), set your
organization in Admin, and describe your first agent on `/agents`.

> Back up `TALARIA_SECRET_KEY` from `ui/.env` somewhere a snapshot isn't: every stored secret is
> sealed with it, and a database restored without it cannot read its own secrets.

Running it for real? One command runs the whole stack as containers, zero required config,
secrets generated on first boot: [`docs/CONTAINER.md`](./docs/CONTAINER.md). Prebuilt images
(`nightly`, `rc`, versioned) are published to GHCR, see [`RELEASING.md`](./RELEASING.md).
Full setup detail, the dev loop, and the architecture: [`DEVELOPERS.md`](./DEVELOPERS.md).

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
[`DEVELOPERS.md`](./DEVELOPERS.md). New contributors start with [`HANDOFF.md`](./HANDOFF.md)
and [`CONTRIBUTING.md`](./CONTRIBUTING.md).

## License

MIT, free forever ([`LICENSE`](./LICENSE)). Open source and self-hostable is the deal, and
self-hosting never loses features. Missing something you'd need to actually run your business
here? That's the whole idea: [open an issue](https://github.com/outcrop-labs/talaria/issues)
and help shape it.
