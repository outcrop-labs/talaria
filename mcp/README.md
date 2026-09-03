# talaria-mcp

The agent-facing MCP server for Talaria's PM suite. Point any MCP-capable agent at it
and the agent can work Talaria boards — with the guardrails held by construction:
there is **no assign tool and no complete tool**. Agents create into the inbox and
report up to quality review; assignment and sign-off stay a human's call. The Talaria
API enforces the same rules server-side, so even a hand-rolled HTTP client can't
bypass them.

## Tools

| Tool | What it does |
| --- | --- |
| `whoami` | Who this agent is and what it may touch: identity (personal? whose? elevated?), boards with **why** each is visible (`policy` / `owner` / `elevated`), channels, servers, guardrails, pending access requests. The cheap read-only probe — call it before probing verbs |
| `list_boards` | Boards this agent is allowed on (per-board agent policy) |
| `list_tickets` | A board's tickets |
| `get_ticket` | Full ticket: fields, comments, activity, watchers, reviews, dependencies |
| `create_ticket` | Create a ticket → lands in **inbox**, unassigned |
| `triage_ticket` | Priority, effort, labels, description, due date, and **forward-only** status moves → `in_progress` / `blocked` / `quality_review` (see below) |
| `comment` | Comment on a ticket — the one write that survives **sign-off** (a closed ticket still takes comments). Not a way around archival |
| `report_outcome` | Record outcome/resolution and hand the ticket to **quality review**. The agent's last status move on that ticket |
| `add_time` | Add seconds to the auto-accumulated time-spent. Live tickets only |
| `log_usage` | Report LLM tokens burned on a ticket (prompt/completion, optional model tier) — feeds the ticket's cost rollup and the fleet ledger. Live tickets only |
| `add_dependency` | Mark a ticket blocked by another on the same board. Both ends must be live |
| `report_gap` | Report a capability gap. `taskId` is optional and gated — without it the gap still lands |

Beyond the PM tools above, the toolkit also carries **artifacts**
(create/update/list/get + `export_to_google_doc`), **knowledge base**
(`list_kb_spaces` / `list_kb_docs` / `read_kb_doc` / `create_kb_space` /
`edit_kb_space` — the landing page shown when someone opens the space itself —
`create_kb_doc` / `edit_kb_doc` / `move_kb_doc` / `delete_kb_doc` — edit only
where granted, delete only what the agent itself created), **channels**
(`list_channels` / `read_channel` /
`post_to_channel` — a personal assistant sees its owner's channels and DMs,
any other agent sees its own memberships; posts stay agent-attributed and
don't trigger other agents), **Google** (per-user calendar/mail read +
confirm-send drafts), and `search_knowledge` (RAG). Same auth model throughout.

**Self-service board access** rounds out the `whoami` answer with its actions:
`join_board` adds the agent to a board's allow-list when its owner can already
read the board (one step, no human in the loop — read is the inherited-
permissions promise), `leave_board` removes only its own row, and
`request_board_access` files with the board's editors for boards the owner
cannot see (a `board_access` approval in their queue; re-filing while open is a
no-op). Nothing here touches assignment or sign-off — the grant is read +
draft reach, and the destructive guardrails above are unchanged.

## Setup

```bash
cd mcp && bun install && bun run build
```

### `mcp/dist` is the thing that runs, and it is gitignored

`bun run build` (tsc) compiles `src/index.ts` → `dist/index.js`. That file — not `src` —
is what the Rust api spawns (`api/src/mcp/service.rs`) and what an agent's stdio
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

`triage_ticket` accepts three statuses, but not from anywhere to anywhere. The
whole rule lives in `agent_safe_patch` (`api/src/tasks.rs`) — **not here** —
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
the Rust tasks engine (`api/src/tasks.rs`) — `agent_ticket_refusal` — answers
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
| `log_usage`, `add_dependency` (both ends), `report_gap`'s `taskId` | their own routes — they never reach `updateTask` |
| `comment` | its route, with intent `comment` — see below |
| the workbench MCP's ticket audit lines, **including `finish_job --abandon`** | `logTicket` → `authorizeTicket` (`api/src/workbench/mcp.rs`) |
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
anyway is caught at startup by `auditRefusalProse`, which logs the offending
tool names to stderr (loud, never fatal — a bad sentence is not a reason to take
the fleet's toolkit down).

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
itself (`api/src/mcp/service.rs`) and injects the connection into every
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
| `api/src/agent_auth.rs` (`agent_caller`) | the matching warning on the resolution the probe exercises |
| `api/src/routes/agents/agent_whoami.rs` | the oracle note in the route's header — the one file a narrowing edit is guaranteed to open |
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
