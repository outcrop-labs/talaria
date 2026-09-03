# Working with agents

Agents are teammates you hire: each one has a soul, its own memory and skills, tools you
grant it, and a model for a brain. You meet them everywhere work happens — mention one in a
channel, chat privately, plan beside one, assign one a ticket — and everything it did is on
the record when you review. What they may do alone is bounded by the platform, not by trust:
an agent never assigns itself work and never signs off its own ticket.

Hiring and configuring agents is an admin (or granted-manager) job — that's the
[admin guide](./admin-agents.md). This page is about working with the ones that exist.

## Where you meet them

| Place | What it's like |
| :--- | :--- |
| **A channel or relay** | @mention brings the agent into the conversation; it replies right in the feed, streaming as it types |
| **An agent chat** | A private thread, one fresh context per topic — history stays out of context, so sessions stay sharp |
| **A plan** | You talk, the agent drafts; the living document builds itself beside the conversation |
| **A board ticket** | It works the problem one step at a time until done or until it needs a person |
| **Your inbox** | An agent may reach out to you first — a proactive check-in lands as a DM |

## To…

| Do this | How |
| :--- | :--- |
| Bring an agent into a channel | Type `@` and its name in the composer — the mention *is* the invitation. `@Dex:opus` also picks a model tier for that reply |
| Get one reply, not three | The first mention of an agent in a message wins — one reply per agent per message |
| Start a private session | Click the agent in Comms' **Agents** section — a new thread opens with fresh context ("history stays out of context"). Right-click → **New thread** anytime |
| Watch it work | A breathing dot on the agent means it's working on a reply; you watch it type in the feed, and **Stop (Esc)** cancels mid-stream |
| Teach an agent something | [Agent Studio](#agent-studio) — name the work, explain it in plain words, Muse drafts the skill |
| Hand it a credential | The key icon in an agent chat — see [Comms](./comms.md#handing-an-agent-a-credential) |
| Walk away mid-reply | The reply still finds you: the thread grows its unread pill and "`Dex` replied" rings the bell — once per thread, further replies fold in. Watching it type counts as reading; opening clears both |
| Walk away | Idle agent chats distill into a summary document and archive on their own |

## Agents on boards

A board decides which agents may work it (board settings → **Agents**; restrictive by
default). Then the columns carry the rules:

| Column rule | Meaning |
| :--- | :--- |
| **agent start** | Assignment in this column counts as approval — agents only pick up work sitting there |
| **review** | Where agent work lands for sign-off. An agent may not sign off its own work, so it hands finished tickets here for a person |
| Blocked | Always present; an agent stuck here is waiting on something |

### Reviewing an agent's ticket

When a ticket hits the review column you get, in order:

1. **The QA judge's verdict** — `Pass` / `Revise` / `Escalate` with its reasoning (advisory
   unless your admin set it enforcing).
2. **The work itself** — Outcome, Resolution, and Error blocks from the agent's report; time
   spent; tokens and cost per model.
3. **Workbench jobs, if any** — "…plans standard-effort work on *repo*; approve to build"
   (**Approve plan** / **Reject**), and "…opened a PR from *branch*" (**Merge** /
   **View PR**).
4. **The gate** — "Ready for review. Approve to complete." → **Approve** or
   **Request changes**.

### Tickets from a conversation

The clipboard icon on a channel or plan runs **Draft tickets**: an agent reads the
conversation and drafts tickets for the board you pick — you walk each proposal
(*Review · 1 of N*), drop or edit what you don't want, then **Create all**. They land in the
board's inbox, **never pre-assigned** — assignment is a person's approval.

## The rails

| An agent can | An agent cannot |
| :--- | :--- |
| Read what it's granted (the ticket, the doc, the thread it's in) | Assign itself work |
| Spend a credential handed to it — once, within the hour | Read a secret's value, or spend one twice |
| Open a pull request against a repo it's granted | Merge its own PR — you press that button |
| Reach out to you first, within the outreach cap | Sign off its own ticket |

## What an agent makes is someone's

Everything an agent creates — a knowledge doc, a document or file, a saved image, a research
report, a workbench plan — carries a **human owner**, not the agent: the person it was
answering in a chat, its owner (for a personal assistant), or the admin who hired it. That
person controls access: share it, make it private, change who can edit — the ordinary owner's
controls, in the item's **Manage access**.

What doesn't change is who can *read* the org's agents' work: an org agent's output stays
visible to the workspace, and its research stays in the org's shared index. Ownership adds
control, not secrecy — and a personal assistant's output stays private to its owner, as
always.

## Agent Studio

**Manage → Agent Studio** is where agents learn. Pick who you're building for on the left
(or **Every agent**), and **Teach** walks you through four steps — *the work, when it
applies, how it's done, who does it* — with Muse drafting the skill from your plain-words
explanation. Two more sections fill in over time: **asked for help with** (work an agent hit
and couldn't do — **Build it** turns the gap into a skill) and **Work routed to**
(workflows that ride along when matching tickets dispatch).

A term you'll meet around agents: a **roll** is a zero-downtime replacement — a fresh
container comes up beside the old one and takes over only once healthy, so the old one
finishes the replies it's holding.
