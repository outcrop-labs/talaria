# Talaria toolkit

description: How and when to use the talaria MCP tools — the workspace playbook every agent should follow.

Talaria IS the company workspace: tickets, knowledge, documents, channels, research, email. The `talaria` MCP tools are how you touch it, and they are your FIRST reach for anything workspace-shaped. This skill is the playbook; the tool list itself is in your MCP handshake.

## The reflexes

**Start of any task: search first.** Someone has probably touched this before. `search_knowledge` sweeps everything anyone said, decided, or documented — decisions, tickets, docs, channel history. One call before you plan saves an hour of rediscovery. If a knowledge space looks relevant, `list_kb_docs` + `read_kb_doc` the specifics.

**Working a ticket: keep the ticket alive.** `get_ticket` before you start (comments and activity carry context the title doesn't — and check its `attachments`: files and linked docs someone left for you). `comment` when you learn something or hit a fork. `report_outcome` when done — outcome is WHAT changed, resolution is HOW; it moves the ticket to quality review, a human signs off. `log_usage` with your token counts, `add_time` with real minutes. Never claim an outcome you didn't verify.

**Before you report an outcome: review your own work first.** A QA judge and a human review after you — don't let them be your first reviewers. Before `report_outcome`, re-read the ticket's requirements (and its template sections, if it has them) and check what you built against each one. For code changes, run the `requesting-code-review` skill — independent reviewer subagent, fix what it finds, then commit. For multi-task implementation work, execute with `subagent-driven-development` — fresh subagent per task with two-stage review, so problems get caught per task instead of at the end. What you catch yourself never becomes a revision cycle.

**Reading attached files.** Tickets and chats carry an `attachments` array. Entries with a `refType` are knowledge docs or artifacts — read those with `read_kb_doc` / `get_document`. Plain entries are uploaded files — `fetch_attachment` with the id: text comes back as text, images you can see directly, other binary formats report metadata only (say plainly what you couldn't read; never guess at contents).

**Producing anything durable: it goes in Talaria.** Deliverables are artifacts (`create_document`, `update_document`); reusable knowledge is a KB doc (`create_kb_doc` in the right space — your drafts stay unofficial until a human promotes them). Work that lives only in your reply or your container is work the company loses. Images you generate: `save_image_artifact`.

**Questions you can't answer from knowledge: `research`.** It runs cited web research (recon for quick, brief for standard, expedition for deep) — never improvise your own scraping pipeline. Poll `research_status`; cite what it found.

**Team communication.** `read_channel` before posting into an ongoing conversation. `post_to_channel` for updates that concern the room; DMs and mentions come to you. Email and calendar go through drafts (`draft_email`, `draft_calendar_event`) — a human approves every send; never promise a teammate something "was sent", say it awaits approval.

## The hard rules

- The company has NO Notion, Obsidian, Airtable, or local note vaults. Never hunt for them, never grep the filesystem for company knowledge. Talaria is the system of record.
- Never fabricate: no invented ticket ids, no claimed tool results you didn't get, no "I archived / sent / deployed" unless the tool call succeeded and you saw it.
- When something BREAKS — a tool errors, credentials missing, connection refused — call `report_problem` with the technical details (it alerts the admin and files a Helpdesk ticket). To the teammate: one plain sentence that something went wrong on your side and the admin is notified. No endpoints, ports, stack traces, or credentials in chat.
- Ticket state is shared truth: don't set `done` (quality review + a human does that), don't assign work to others; triage what you're told to triage.

## Quick map

| You need | Reach for |
|---|---|
| What does the company know about X? | `search_knowledge`, then `read_kb_doc` |
| My assigned work, full context | `get_ticket` (+ `fetch_attachment` for files) |
| Record progress / finish | `comment`, `report_outcome`, `log_usage` |
| About to report done | self-review vs requirements; `requesting-code-review` for code |
| Write something durable | `create_document` / `create_kb_doc` |
| Answer needs the live web | `research` |
| Tell the team | `post_to_channel` (after `read_channel`) |
| Reach outside (mail/calendar) | `draft_email` / `draft_calendar_event` |
| Something is broken | `report_problem` |
