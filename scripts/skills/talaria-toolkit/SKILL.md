# Talaria toolkit

description: How and when to use the talaria MCP tools — the workspace playbook every agent should follow.

Talaria IS the company workspace: tickets, knowledge, documents, channels, research, email. The `talaria` MCP tools are how you touch it, and they are your FIRST reach for anything workspace-shaped. This skill is the playbook. The tools themselves arrive over MCP and may not sit in your visible tool list — your harness defers MCP tools behind its tool-search bridge, and deferred is not missing: `tool_search("talaria")` surfaces the whole toolkit (exact names arrive prefixed, `mcp_talaria_*`), and once you know a name, `tool_describe` it and call it directly. Never conclude a talaria capability doesn't exist without searching first.

## The reflexes

**Start of any task: search first.** Someone has probably touched this before. `search_knowledge` sweeps everything anyone said, decided, or documented — decisions, tickets, docs, channel history. One call before you plan saves an hour of rediscovery. If a knowledge space looks relevant, `list_kb_docs` + `read_kb_doc` the specifics.

**Working a ticket: keep the ticket alive.** Address it by the `id` from `list_tickets` or the ref in the assignment title (`PLAT-118`). Both work on `get_ticket`, `comment`, `triage_ticket`, `report_outcome`, `add_time`, `log_usage`, `add_dependency`, `start_job`, `report_gap`, and `report_problem`. A bare number is not an id. `get_ticket` before you start (comments and activity carry context the title doesn't — and check its `attachments`: files and linked docs someone left for you). `comment` when you learn something or hit a fork. If comment answers 409 that the ticket has no owner to hold its room, a board member is missing — `report_problem` once and stop; retrying will not create the room. `report_outcome` when done — outcome is WHAT changed, resolution is HOW; it moves the ticket to quality review, a human signs off. `log_usage` with your token counts, `add_time` with real minutes. Never claim an outcome you didn't verify.

**Dev work never happens in chat.** You never write, modify, or commit code from a chat thread — not even small changes. Every dev task flows through a ticket and a workbench job: when someone asks for dev work in chat, create or link the ticket and move execution into the workbench; if a human explicitly asks for an out-of-band change, the ticket still comes first. Capture dev-work instructions given in chat as comments on the relevant ticket, so the work context lives where the work happens.

**Before you report an outcome: review your own work first.** A QA judge and a human review after you — don't let them be your first reviewers. Before `report_outcome`, re-read the ticket's requirements (and its template sections, if it has them) and check what you built against each one. For code, read your own diff and run the repo's tests (the **workbench-driving** skill is the discipline when a harness did the writing). For multi-task implementation work, execute with `subagent-driven-development` — fresh subagent per task with two-stage review, so problems get caught per task instead of at the end. What you catch yourself never becomes a revision cycle.

**Reading attached files.** Tickets and chats carry an `attachments` array. Entries with a `refType` are knowledge docs or artifacts — read those with `read_kb_doc` / `get_document`. Plain entries are uploaded files — `fetch_attachment` with the id: text comes back as text, images you can see directly, other binary formats report metadata only (say plainly what you couldn't read; never guess at contents).

**Producing anything durable: it goes in Talaria.** Markdown deliverables are `create_document`; a tracker or comparison grid is `create_sheet` (row 0 is the header — not a markdown table inside a doc); a public HTML page is `create_page`. Reusable knowledge is a KB doc (`create_kb_doc` in the right space — your drafts stay unofficial until a human promotes them). Work that lives only in your reply or your container is work the company loses. Images you generate: `save_image_artifact`.

**Questions you can't answer from knowledge: `research`.** It runs cited web research (recon for quick, brief for standard, expedition for deep) — never improvise your own scraping pipeline. Poll `research_status`; cite what it found.

**Team communication.** `read_channel` before posting into an ongoing conversation. `post_to_channel` for updates that concern the room — pass `threadId` (the root's message id from `read_channel`, never the seq) to reply in a thread rather than forking a new top-level message. Acknowledge with `react_to_message` (a ✅ is not a new post; the message id, never the seq). DMs and mentions come to you. When something genuinely needs one specific person NOW — their work is blocked on you, a decision only they can make, a deadline about to slip — `message_user` starts a real conversation with them (it notifies their inbox). It's rate-limited per person per day: spend those sends on things that matter, never on status updates (that's a ticket comment) or things the room should see (that's a channel post). Email and calendar go through the connected account: drafts wait for a human to approve; never say a message was sent.

**Git and GitHub: push over HTTPS, no setup.** Credentials for GitHub are injected by Talaria at git time — `git clone`, `pull`, and `push` with plain `https://` URLs just work for the repos granted to you. There is no gh CLI, no token in your environment, and no SSH key, and looking for them will (correctly) find nothing: the credential never enters your context, your command output, or your disk. So never diagnose GitHub access by checking for configured auth — diagnose it by doing the git operation. If git says it could not read a username, or the helper says `no credential for github.com` with no repository after the host, git did not name a repo. From a checkout, set origin to `https://github.com/<owner>/<repo>.git` and retry — the helper reads origin when the path is missing. Do not run `git credential fill`; that prints the token into your transcript. A repo you are not granted is `report_problem`, not a token to mint.

**Reaching main is a human's call, not yours.** Push your work to a branch named for the ticket, open a PR, and put the link in your outcome or comment — `main` is protected and will refuse your pushes, which is the design, not an error to work around. Never force-push, never rewrite history on shared branches, and never commit secrets; a person reviews and merges.

## The hard rules

- The company has NO Notion, Obsidian, Airtable, or local note vaults. Never hunt for them, never grep the filesystem for company knowledge. Talaria is the system of record.
- Never set up your own GitHub authentication — no `gh auth login`, no device-flow login, no stored tokens, no SSH keys. A credential you create is standing and unscoped; the injected ones are per-repo and revocable. A repo the injected credential doesn't cover is an ask to a human, not a workaround to build.
- Never fabricate: no invented ticket ids, no claimed tool results you didn't get, no "I archived / sent / deployed" unless the tool call succeeded and you saw it.
- When something BREAKS — a tool errors, credentials missing, connection refused — call `report_problem` with the technical details (it alerts the admin and files a Helpdesk ticket). To the teammate: one plain sentence that something went wrong on your side and the admin is notified. No endpoints, ports, stack traces, or credentials in chat.
- Ticket state is shared truth: don't set `done` (quality review + a human does that), don't assign work to others, don't parent a sub-task (`create_ticket` ignores `parentId` — comment the breakdown and let a human file it). Triage what you're told to triage.

## Quick map

| You need | Reach for |
|---|---|
| What does the company know about X? | `search_knowledge`, then `read_kb_doc` |
| My assigned work, full context | `get_ticket` (+ `fetch_attachment` for files) |
| Record progress / finish | `comment`, `report_outcome`, `log_usage` |
| About to report done | self-review vs requirements; **workbench-driving** when a harness wrote the code |
| Write something durable | `create_document` / `create_sheet` / `create_page` / `create_kb_doc` |
| Answer needs the live web | `research` |
| Tell the team | `post_to_channel` (after `read_channel`; `threadId` to stay in a thread) |
| Acknowledge without posting | `react_to_message` |
| One person needs this now | `message_user` (sparingly — it's rate-limited) |
| Reach outside (mail/calendar) | `draft_email` / `draft_calendar_event` |
| Push to GitHub | plain `git` over `https://` — credentials are injected, none will be visible |
| Build a Talaria app | **talaria-apps** skill — TypeScript, `@talaria/sdk`, never Rust |
| A talaria tool isn't in your tool list | deferred, not missing — `tool_search("talaria")`, then call by exact name |
| Something is broken | `report_problem` |
