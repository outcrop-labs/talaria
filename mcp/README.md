# talaria-mcp

The agent-facing MCP server for Talaria's PM suite. Point any MCP-capable agent at it
and the agent can work Talaria boards — with the guardrails held by construction:
there is **no assign tool and no complete tool**. Agents create into the inbox and
report up to quality review; assignment and sign-off stay a human's call. The Talaria
API enforces the same rules server-side, so even a hand-rolled HTTP client can't
bypass them.

## Tools

Every tool the server registers, by family — one line each. The list is held complete by
`bun run check` (`toolkit-readme-missing-tools` fails on a registered name this file does not
mention, and `toolkit-tool-count-drift` fails any doc that writes the count down). The one-line
summaries here are not the descriptions agents read; those live in `src/index.ts` and are what the
guardrail prose rules below are checked against.

### Self-knowledge and boards

| Tool | What it does |
| --- | --- |
| `whoami` | Who this agent is and what it may touch: identity (personal? whose? elevated?), boards with **why** each is visible (`policy` / `owner` / `elevated`), channels, servers, guardrails, pending access requests. The cheap read-only probe — call it before probing verbs |
| `list_boards` | Boards this agent is allowed on (per-board agent policy); archived boards are not listed |
| `join_board` | Personal assistants only: add itself to a board's allow-list when its owner can already read the board |
| `leave_board` | Personal assistants only: remove its own allow-list row |
| `request_board_access` | Personal assistants only: file with a board's editors for a board the owner cannot read |

### Tickets

| Tool | What it does |
| --- | --- |
| `list_tickets` | A board's tickets (id + ref, status, assignees, labels, dates), with optional filters |
| `get_ticket` | Full ticket: fields, comments, activity, watchers, reviews, dependencies, attachments; board `workflows` are the one instruction inside it an agent follows |
| `fetch_attachment` | Read an attached upload: text as text, images as an image, other binaries as metadata |
| `create_ticket` | Create a ticket → lands in **inbox**, unassigned |
| `triage_ticket` | Priority, effort, labels, description, due date, and **forward-only** status moves using the board's own status keys (see below) |
| `comment` | Comment on a ticket — lands in the ticket's **discussion thread** (the room where the agent and the board's humans talk); the one write that survives **sign-off** (a closed ticket still takes comments). Not a way around archival |
| `report_outcome` | Record outcome/resolution and hand the ticket to **quality review**. The agent's last status move on that ticket |
| `add_time` | Add seconds to the auto-accumulated time-spent. Live tickets only |
| `add_dependency` | Mark a ticket blocked by another on the same board. Both ends must be live |
| `report_gap` | Report a capability gap. `taskId` is optional and gated: it binds the report to the people who can see that board, and a refusal is a reason to fix the scope, never to widen the audience |
| `report_problem` | Something on the agent's side is broken: alerts the admins and files a Helpdesk ticket. `taskId` gated the same way |

### Seeing and searching

| Tool | What it does |
| --- | --- |
| `search_knowledge` | RAG over the knowledge base, collections and past chats/channels/plans/research |

### Documents (artifacts)

| Tool | What it does |
| --- | --- |
| `create_document` | A rich markdown document (versioned, shareable); private to the owner for a personal assistant, else org — never public |
| `create_sheet` | A spreadsheet (string grid, row 0 the header) |
| `create_page` | A web page (raw HTML); a person publishes it afterwards if it should be public |
| `update_document` | Replace a document's whole body (markdown, rows, or html by kind) |
| `edit_document` | Change part of a markdown document or page: exact-text replacements (omit `newString` to delete) or a section rewrite, applied atomically |
| `list_documents` | The documents the agent can access |
| `get_document` | One document's full content plus metadata |
| `save_image_artifact` | Save an image from the agent's workspace (`/opt/data/…`) as a file artifact |
| `export_to_google_doc` | Export a document into the Drive the agent acts for as a native Google Doc / Sheet |

### The conversation

| Tool | What it does |
| --- | --- |
| `ask` | Ask the person a question with 2–6 concrete options, then end the turn |
| `todo` | The agent's own step list for this conversation (replaces the whole list each call) |
| `organize_session` | Rename, pin, group or archive THIS conversation — only when asked |
| `expose_tools` | Show platform tools as chips the person can open and run themselves |

### Channels

| Tool | What it does |
| --- | --- |
| `list_channels` | Channels the agent can see: a personal assistant sees its owner's channels and DMs, any other agent its own memberships |
| `read_channel` | Recent messages (or one thread) in a channel; `sinceSeq` for only what is new |
| `post_to_channel` | Post, agent-attributed; `threadId` keeps a reply in an existing thread |
| `react_to_message` | Toggle an emoji reaction under the agent's own identity |

### Knowledge base

| Tool | What it does |
| --- | --- |
| `list_kb_spaces` | The spaces (shelves) the agent can access |
| `list_kb_docs` | The docs in a space, with tree position |
| `read_kb_doc` | One doc's markdown body + metadata |
| `create_kb_space` | Find-or-create a top-level space |
| `create_kb_doc` | A new doc in a space — a draft until a human marks it official |
| `edit_kb_doc` | Replace the title/body of a doc the agent holds Editor on |
| `edit_kb_space` | Name, description, icon and landing-page markdown of a space it created or edits |
| `move_kb_doc` | Re-file a doc within its space's tree |
| `delete_kb_doc` | Delete a doc the agent itself created — only those |

### Google Calendar

| Tool | What it does |
| --- | --- |
| `read_calendar` | Upcoming events on the calendar the agent acts for |
| `draft_calendar_event` | Queue an event for a human to approve (confirm-sends) |
| `update_google_event` | Queue a time/title change to an event |
| `cancel_google_event` | Queue a cancellation |
| `create_google_meeting` | Queue a meeting with attendees and a Meet link (agenda becomes a Doc after approval) |

### Gmail

| Tool | What it does |
| --- | --- |
| `read_recent_email` | Recent mail (metadata + snippets), optional Gmail `q` |
| `read_email` | One full message by id |
| `draft_email` | Queue an email for a human to approve; never "sent" |
| `list_labels` | The account's labels (Gmail's folders) |
| `create_label` | Find-or-create a label |
| `organize_emails` | Apply/remove labels by id: file, archive (remove INBOX), mark read; nothing is ever deleted |
| `list_pending_sends` | The confirm-sends queue the approver sees — the only proof a draft is waiting |
| `read_pending_send` | One queued draft by id |

### Google Drive, Docs, Sheets and Slides

| Tool | What it does |
| --- | --- |
| `search_drive` | Find files by name in the Drive the agent acts for (read-only) |
| `find_google_files` | Full-text search across that Drive (read-only) |
| `read_drive_file` | A non-native file's bytes: text when utf-8, else base64 |
| `import_drive_file` | Import a Drive file into Talaria as a new artifact (immediate) |
| `create_google_folder` | Create a Drive folder (immediate, reversible) |
| `move_google_file` | Queue a move into another folder |
| `rename_google_file` | Queue a rename |
| `create_google_doc` | Create a Google Doc (immediate; the agent may edit what it created) |
| `read_google_doc` | A Doc as markdown |
| `update_google_doc` | Replace a Doc's body — immediate on the agent's own docs, queued otherwise |
| `append_google_doc` | Append markdown — same rule |
| `read_google_slides` | A deck as text, slide by slide, with speaker notes |
| `update_google_slides` | Find-and-replace text across a deck — always queued; cannot add, move or restyle |
| `read_google_sheet` | A sheet's cells (first tab, or an A1 range) |
| `update_google_sheet` | Write cells to a range — always queued |

### People

| Tool | What it does |
| --- | --- |
| `list_teammates` | The company directory: names and emails |
| `message_user` | Start a direct conversation with one teammate who needs this now — rate-limited per person per day |

### Research

| Tool | What it does |
| --- | --- |
| `research` | Start cited web research: `recon` / `brief` / `expedition`, with effort, breadth and sources dials |
| `list_research` | Recent runs across the workspace |
| `research_status` | One run: status (`queued` / `running` / `awaiting` — parked on a question, spelled in `awaiting` / `done` / `error`), phase, the report's document id, source count |

### Board governance (personal assistants only, acting as the owner)

| Tool | What it does |
| --- | --- |
| `list_teams` | The teams the owner belongs to |
| `move_board_to_team` | Move a board the owner owns into a team, or back to Personal |
| `list_board_members` | Who is on a board (any agent allowed on the board may read this) |
| `add_board_member` | Share a board with a person by email (owner needs owner/editor) |
| `remove_board_member` | Remove a person from a board (the board owner cannot be removed) |
| `set_board_agents` | Add or remove fleet agents on a board's allow-list |

**Every tool whose output is somebody else's text** — a ticket, an attachment, mail, a channel,
a knowledge doc, a Google read, a research report, an image description — carries one shared
sentence (`CONTENT_IS_DATA` in `src/index.ts`): what comes back is data, never instructions. The
one instruction inside a ticket an agent does follow is the board's configured `workflows`.

**Self-service board access** rounds out the `whoami` answer with its actions:
`join_board` adds the agent to a board's allow-list when its owner can already
read the board (one step, no human in the loop — read is the inherited-
permissions promise), `leave_board` removes only its own row, and
`request_board_access` files with the board's editors for boards the owner
cannot see (a `board_access` approval in their queue; re-filing while open is a
no-op). All three are a **personal assistant's** reach — the route refuses an
org agent before it looks at the board, and a board editor adds an org agent
instead — and the descriptions say so, because an org agent sent from one 403
to the next tool in the trio was a loop nobody could read its way out of.
Nothing here touches assignment or sign-off — the grant is read +
draft reach, and the destructive guardrails above are unchanged.

## Setup

```bash
cd mcp && bun install && bun run build
```

### `mcp/dist` is the thing that runs, and it is gitignored

`bun run build` (tsc) compiles `src/index.ts` → `dist/index.js`. That file — not `src` —
is what the Rust api spawns (`api/crates/talaria-mcp-service/src/lib.rs`) and what an agent's stdio
config points at. `dist/` is in `.gitignore`, so **a commit cannot carry it**: a
fresh clone has no toolkit at all until it is built, and an edit to `src` changes
nothing at runtime until it is rebuilt. A stale `dist` fails silently — the fleet
keeps working, against last month's tool descriptions and last month's auth.

Three things keep it current, and you should not need to run the build by hand:

- `bun talaria setup` installs `mcp/`'s dependencies and builds it (fresh install).
- `bun talaria dev` rebuilds whenever anything in `mcp/src` is newer than
  `mcp/dist/index.js`, and refuses to start the stack if that build fails
  (`TALARIA_SKIP_MCP_BUILD=1` overrides, and says that dist may be stale).
- CI (`.github/workflows/ci.yml`, job `mcp`) typechecks and builds this package on
  every PR. It does **not** publish `dist`; it only proves the build works.

Configure your agent's MCP client (stdio transport):

```json
{
  "mcpServers": {
    "talaria": {
      "command": "bun",
      "args": ["/path/to/talaria/mcp/dist/index.js"],
      "env": {
        "TALARIA_URL": "http://localhost:5273",
        "TALARIA_AGENT_KEY": "<this agent's tak_ credential, from fleet/.env>",
        "TALARIA_AGENT_NAME": "sam"
      }
    }
  }
}
```

- `TALARIA_AGENT_KEY` is the agent's OWN credential (`TALARIA_AGENT_KEY_<SLUG>`
  in `fleet/.env`, minted per agent by the renderer). Talaria resolves identity
  from it, which is what makes board policy and tool allowlists enforceable.
- `TALARIA_AGENT_NAME` is the agent's fleet model name. It is now a CROSS-CHECK:
  Talaria refuses a request whose name contradicts its credential. It still
  attributes every ticket, comment, and activity entry to the agent.

## How it holds the guardrails

Board access is restrictive by default: an agent sees a board only if the board
allows all agents or lists the agent by name (Board settings → Agents). On the
API side an agent that supplies `assignees` on create is refused outright (403,
`agents cannot assign tickets`), so agent-created tickets land unassigned in the
board's intake column; entering an agent-start column later is refused for the
same reason; and a terminal move (`done`, or the off-board `failed` / `cancelled`)
is redirected to the board's own review column. This MCP server additionally never
offers the unsafe inputs, so a well-behaved agent never even sees them.

### The lifecycle is one-way for an agent

`triage_ticket` accepts the board's own status keys (three on a board that kept
the defaults), but not from anywhere to anywhere. The
whole rule lives in `agent_safe_patch` (`api/crates/talaria-tasks/src/lib.rs`) — **not here** —
and every clause raises `HumanApprovalRequired`, which the route turns into a
403. The tool descriptions mirror it, because an agent that doesn't know spends
turns on writes that can't land:

| Move | Agent | Why |
| --- | --- | --- |
| assigned → `in_progress` | yes | working what a person gave it |
| in_progress → `blocked` | yes | parking its own work |
| in_progress → `quality_review` | yes | handing over for sign-off |
| blocked → `in_progress` | **no** | entering a start column is assignment; a person restarts parked work |
| out of the review column | **no** | review is the human sign-off queue — otherwise an agent pulls its own work back off the reviewer's board |
| any write to a **closed** ticket (a `done` column, or `failed` / `cancelled`) | **no** | sign-off is sticky and covers the record, not just the column. Includes `add_time` |
| any write to an **archived** ticket, or any ticket on an **archived board** | **no** | archiving hides work from the people watching it; an agent noticing and writing anyway is that stop failing |
| any move of a ticket **stranded** in a status its board no longer has | **no** | nothing can class it, so a person places it |
| entering an agent-start column from anywhere else | **no** | the destination is the gate, whether the agent named that column or the terminal-move redirect picked it |

**"Live" is one word for three conditions.** A ticket takes agent writes only
while its status is not closed (`done` column / `failed` / `cancelled`), the
ticket is not archived, and its board is not archived. One exported predicate in
the Rust tasks engine (`api/crates/talaria-tasks/src/lib.rs`) — `agent_ticket_refusal` — answers
that, *and* the board's
agent policy, for every door, and returns the *reason*, so the refusal reads the
same sentence wherever the write arrived.

Counting the doors is a losing game (this file has claimed four and five, and
was wrong within a round each time) because the number only ever grew when
someone re-expressed the rule by hand. What matters is that no door expresses it
itself:

| Door | Reaches the predicate via |
| --- | --- |
| the ticket patch (`triage_ticket`, `report_outcome`, `add_time`) | `agent_safe_patch` inside `updateTask` |
| `add_dependency` (both ends), `report_gap`'s `taskId` | their own routes — they never reach `updateTask` |
| `comment` | its route, with intent `comment` — see below |
| the workbench MCP's ticket audit lines, **including `finish_job --abandon`** | `logTicket` → `authorizeTicket` (`api/crates/talaria-workbench-mcp/src/lib.rs`) |
| the ticket a workbench verb names in its arguments | `ticketArg` → `authorizeTicket` — parse and gate are one call, so a verb cannot hold an ungated `taskId` |

A `task_activity` row on a ticket **is** a write to that ticket, which is why
the audit lines are in that list at all.

**The one clause that comes apart: comments.** `agentTicketRefusal` takes an
intent, and `comment` skips exactly ONE of the four stop conditions — the closed
status. A **closed** ticket still takes comments (the agent's channel on work it
can no longer edit); an **archived** ticket, or one on an **archived board**,
does not, because archiving withdraws the work from the people watching it. The
`comment` tool description used to promise the opposite, which is worse than an
under-stated refusal: it taught agents that archival was not a real stop, on the
one tool whose entire job is being the exception.

Every tool description in `src/index.ts` that mentions a refusal must therefore
name all three conditions — and must not promise more than `comment` actually
gets. They no longer spell the sentence out: `LIVE_ONLY`, `COMMENT_INSTEAD` and
`COMMENT_EXEMPTION` are defined once at the top of that file and interpolated,
so widening the rule is one edit. A description that hand-rolls the sentence
anyway is caught twice: at startup by `auditRefusalProse`, which logs the
offending tool names to stderr (loud, never fatal — a bad sentence is not a
reason to take the fleet's toolkit down), and in CI by `bun run check`
(`toolkit-prose-refusal-drift` / `toolkit-prose-teaches-scope-drop` in
`scripts/check-invariants.mjs`), which reads the registrations out of the
source with the file's own regexes and IS fatal. The second rule — no text may
offer a scope argument (`report_gap`'s `taskId`) as the thing to shed when a
call is refused — runs over this README and the toolkit skill too, because that
advice last survived here, in words the tool text had already stopped using.

Three consequences worth knowing before you burn a turn on them:

- **The review column belongs to the board, not to this server.** `report_outcome`
  does not send a literal `quality_review`; the API redirects the terminal move to
  whatever review column that board has. If the board has **none** — or every
  review column is also flagged agent-start, which would loop the work straight
  back into the pickup queue — the write is **refused**, not guessed at, and the
  error names the board and the setting to fix. That is an admin's problem, not a
  misbehaving agent.
- **`comment` is the exception and the escape hatch — but only past sign-off.**
  It stays open on a *closed* ticket the agent can no longer edit. It is refused
  on an archived ticket and on an archived board, like every other write.
- **An archived board is out of service at every door.** `boardAllowsAgent`
  refuses it, so an archived board is not listed, its tickets are not readable,
  and `create_ticket` on one 403s — not just the writes in the table above.


## Fleet HTTP mode

Set `MCP_HTTP_PORT` and talaria-mcp serves the WHOLE fleet over stateless
streamable HTTP instead of stdio: each request is handled by a fresh server
bound to the calling agent's identity. Auth is PASS-THROUGH: this process has
no identity of its own, so it forwards each caller's `X-Api-Key` (and
`X-Agent-Name`) to Talaria, which is the only thing that can validate them.
Talaria runs this mode
itself (`api/crates/talaria-mcp-service/src/lib.rs`) and injects the connection into every
rendered agent config — you never start it by hand. Stdio mode (one agent via
`TALARIA_AGENT_NAME`) remains for external clients.

`MCP_HTTP_HOST` overrides where it listens (comma-separated). The default is
loopback plus the docker bridge addresses the fleet actually arrives on, rescanned
periodically because docker brings a compose network's bridge up only once a
container attaches. Binding `0.0.0.0` publishes the toolkit catalog on every
interface the host has; do that only behind a network policy.

### Authentication — and the route it depends on

Tool *calls* need no local check: each one ends in a request Talaria
authenticates. `initialize` and `tools/list` do not — the MCP SDK answers those
from this process, so without a check the full toolkit catalog (every tool name,
description and JSON schema: a map of the fleet's write surface) is readable by
anyone who can open a socket. So before a server is built for a caller, this
process **verifies the presented credential against Talaria** and caches the
verdict briefly (60s accepted / 5s refused, keyed by name + a hash of the
credential). Unreachable Talaria ⇒ `503`, never a pass.

It verifies by issuing an authenticated `GET /api/agent/whoami` and reading the
status code. That route is the **purpose-built probe** — agent self-introspection
whose other job is exactly this — but it is still an authentication oracle for
the entire toolkit, and the coupling is the fragile part of this design:

> Narrow `/api/agent/whoami` — admin-only, session-only, renamed, moved — and
> every agent's `initialize`/`tools/list` starts failing. The fleet toolkit goes
> dark **fleet-wide**, and nothing in the resulting errors points at the route
> change.

The contract that route has to keep, therefore:

| Requirement | Why |
| --- | --- |
| Authenticates an agent's own `tak_` credential | it is the credential being verified |
| `GET`, cheap, no side effects | issued on every cache miss |
| `401`/`403` on a bad credential | that is the refusal this process forwards |
| Not `404`/`405` | those mean *the probe itself* broke, not the caller |

If you must change it, repoint the probe first: **`TALARIA_MCP_VERIFY_PATH`**
overrides the path with no code change. A `404`/`405` from the probe is treated as
"Talaria unreachable" (`503`, uncached) and logged once per outage with that
instruction, so the failure is at least diagnosable.

**Where the warning has to live.** The person who breaks this is not reading
`mcp/src/index.ts`; they are changing an agent route. So the coupling is stated
at every file that edit passes through:

| File | Carries |
| --- | --- |
| `mcp/src/index.ts` (`verify`) | the ⚠ coupling note and the repoint instruction |
| `api/crates/talaria-agent-auth/src/lib.rs` (`agent_caller`) | the matching warning on the resolution the probe exercises |
| `api/crates/talaria-routes-fleet/src/agents/agent_whoami.rs` | the oracle note in the route's header — the one file a narrowing edit is guaranteed to open |
| the startup log | one line naming the coupling on every boot that uses the default probe, so the sentence is already in an operator's scrollback before the outage |

A warning one module away does not get read; that is why the marker sits in the
route file itself, above the handler.

**History, so nobody borrows a product route again.** The oracle used to live on
`GET /api/users` — chosen because its agent branch is nothing but
`agent_caller()` — which made the people directory load-bearing for fleet auth:
one good product decision to narrow it would have darkened every agent's
toolkit. The purpose-built `whoami` route (auth, negligible payload, no product
reason to ever narrow it) removed that borrow; `/api/users` is free to be a
people picker again.
