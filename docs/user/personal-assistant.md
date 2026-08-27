# Your assistant

Your assistant is a real agent that's just yours: its own memory, skills, and tools, running
on a model nobody else picked — because it reads your private work, it never runs on one
somebody else chose. It writes your daily brief, drafts your replies, and works the Inbox
beside you. Nothing that leaves the building — an email, an invite, a posted reply you
delegated — happens without your approval.

Not the same as an org agent: those the company hires on the [Agents page](./admin-agents.md);
this one lives in your sidebar, above the nav, and works only for you.

## Set it up

**Set up your assistant** — the card at the top of your sidebar, or **Settings → Assistant**.
Three steps:

1. **Name** — what you'll call it, plus its **@handle** (how agents and integrations refer
   to it; **can't be changed later**).
2. **Personality** — a starting point (**Warm & proactive** · **Concise & professional** ·
   **Playful & curious**) or write your own. Skip it and you get the default:
   *warm, direct, and useful*.
3. **Launch** — creating it starts a private workspace; this can take a minute. Then
   **Say hello**.

Your first daily brief opens tomorrow, two hours before your workday starts — it's waiting
for you rather than the other way around.

## Tune it

**Settings → Assistant** — everything about it, no admin involved:

| Setting | What it does |
| :--- | :--- |
| **Name** / **Handle** | Chats, memory, and access move with a renamed handle; mentions pick up the new one |
| **Model** | Tier chips — which brain it runs on |
| **Personality** | How it comes across: tone, priorities, pet peeves. Every save is versioned and applies right away — your assistant restarts with it |
| **Start / Stop** | The container beneath it |
| **Open chat →** | A straight line to it |

Below the settings, its working parts: **Schedules** (recurring jobs it runs on its own
scheduler — they keep firing even when Talaria is down; describe one like *"every weekday
morning, summarize my inbox into a brief"* and it's drafted for you), **Skills**, and
**Memory** (add *"remember something: one fact the agent should keep"*; it's MEMORY.md, with
version history).

## What it does for you

| Thing | How it works |
| :--- | :--- |
| **The daily brief** | Written by your assistant and nothing else — see [Your day](./your-day.md) |
| **Drafted replies** | For conversations where someone's waiting on you: a ready reply with **Send it** / **Discard**. Marked `OUT OF DATE` if the other person spoke since — a fresh one comes on the next pass |
| **Inbox actions** | In the drawer, it proposes actions — **Confirm exact action** — one at a time |
| **Reaching out** | It may DM you first; personal assistants reach only their owner, rate-capped per day |

## Letting it speak for you

Two switches, both honest about who's talking:

- **Google Workspace** (**Settings → Assistant** or **Settings → Connections**): connect
  your account and your assistant reads your mail and calendar live, finds Drive files, and
  drafts emails and events for you. **Every send waits for your approval in the Inbox** —
  reads and drafts are free; anything that leaves the building is a card you press.
- **Per-conversation delegation**: on a drafted reply, **Let my assistant reply here** — it
  then answers that conversation for you (**Assistant handles this (stop)** to revoke). A
  delegated reply is posted *in the assistant's own name*, never impersonating you, and the
  grant is kept on record — who was allowed to speak for whom, and when it stopped.

**Tool accounts** (Settings → Connections) extend the same idea to your org's MCP servers
that run in per-user mode: connect your own account and your assistant uses the server as
you; disconnect any time.

## Its rules

Your assistant's working rules are in its soul, and the platform holds the line: it creates
and triages tickets for you, but never assigns or closes them; it prefers the local model
tier for routine work and escalates deliberately.

Admins can mark their own assistant **elevated** (their People row in Admin) to widen its
view org-wide.

## Words your assistant uses

| Term | Meaning |
| :--- | :--- |
| **Brief** | The morning document it writes — see [Your day](./your-day.md) |
| **Digest** | The separate one-email-a-day summary (Settings → Notifications) |
| **Delegation** | Letting the assistant answer one conversation for you, in its own name |
| **Tool account** | An MCP server connected as you, for your assistant to use |
| **Memory** | MEMORY.md — the facts it keeps, versioned |
