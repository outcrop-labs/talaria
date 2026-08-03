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
| `list_boards` | Boards this agent is allowed on (per-board agent policy) |
| `list_tickets` | A board's tickets |
| `get_ticket` | Full ticket: fields, comments, activity, watchers, reviews, dependencies |
| `create_ticket` | Create a ticket → lands in **inbox**, unassigned |
| `triage_ticket` | Priority, effort, labels, description, due date, and **forward-only** status moves → `in_progress` / `blocked` / `quality_review` (see below) |
| `comment` | Comment on a ticket — the one write that stays open on a ticket the agent can no longer edit |
| `report_outcome` | Record outcome/resolution and hand the ticket to **quality review**. The agent's last status move on that ticket |
| `add_time` | Add seconds to the auto-accumulated time-spent. Open tickets only |
| `log_usage` | Report LLM tokens burned on a ticket (prompt/completion, optional model tier) — feeds the ticket's cost rollup and the fleet ledger |
| `add_dependency` | Mark a ticket blocked by another on the same board |

Beyond the PM tools above, the toolkit also carries **artifacts**
(create/update/list/get + `export_to_google_doc`), **knowledge base**
(`list_kb_spaces` / `list_kb_docs` / `read_kb_doc` / `edit_kb_doc` — edit only
where granted), **channels** (`list_channels` / `read_channel` /
`post_to_channel` — membership-gated; agent posts don't trigger other agents),
**Google** (per-user calendar/mail read + confirm-send drafts), and
`search_knowledge` (RAG). Same auth model throughout.

## Setup

```bash
cd mcp && npm install && npm run build
```

### `mcp/dist` is the thing that runs, and it is gitignored

`npm run build` compiles `src/index.ts` → `dist/index.js`. That file — not `src` —
is what the app spawns (`ui/src/server/mcp-service.ts`) and what an agent's stdio
config points at. `dist/` is in `.gitignore`, so **a commit cannot carry it**: a
fresh clone has no toolkit at all until it is built, and an edit to `src` changes
nothing at runtime until it is rebuilt. A stale `dist` fails silently — the fleet
keeps working, against last month's tool descriptions and last month's auth.

Three things keep it current, and you should not need to run the build by hand:

- `scripts/setup.sh` installs `mcp/`'s dependencies and builds it (fresh install).
- `scripts/dev.sh` rebuilds whenever anything in `mcp/src` is newer than
  `mcp/dist/index.js`, and refuses to start the stack if that build fails
  (`TALARIA_SKIP_MCP_BUILD=1` overrides, and says that dist may be stale).
- CI (`.github/workflows/ci.yml`, job `mcp`) typechecks and builds this package on
  every PR. It does **not** publish `dist`; it only proves the build works.

Configure your agent's MCP client (stdio transport):

```json
{
  "mcpServers": {
    "talaria": {
      "command": "node",
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
whole rule lives in `agentSafePatch` (`ui/src/server/tasks.ts`) — **not here** —
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

Two consequences worth knowing before you burn a turn on them:

- **The review column belongs to the board, not to this server.** `report_outcome`
  does not send a literal `quality_review`; the API redirects the terminal move to
  whatever review column that board has. If the board has **none** — or every
  review column is also flagged agent-start, which would loop the work straight
  back into the pickup queue — the write is **refused**, not guessed at, and the
  error names the board and the setting to fix. That is an admin's problem, not a
  misbehaving agent.
- **`comment` is the exception and the escape hatch:** it stays open on a ticket
  the agent can no longer edit.


## Fleet HTTP mode

Set `MCP_HTTP_PORT` and talaria-mcp serves the WHOLE fleet over stateless
streamable HTTP instead of stdio: each request is handled by a fresh server
bound to the calling agent's identity. Auth is PASS-THROUGH: this process has
no identity of its own, so it forwards each caller's `X-Api-Key` (and
`X-Agent-Name`) to Talaria, which is the only thing that can validate them.
Talaria runs this mode
itself (`ui/src/server/mcp-service.ts`) and injects the connection into every
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

It verifies by issuing an authenticated `GET /api/users` and reading the status
code. **That makes `/api/users` an authentication oracle for the entire toolkit,**
and the coupling is the fragile part of this design:

> Narrow `/api/users` — admin-only, session-only, renamed, moved — and every
> agent's `initialize`/`tools/list` starts failing. The fleet toolkit goes dark
> **fleet-wide**, and nothing in the resulting errors points at the route change.

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
instruction, so the failure is at least diagnosable. `ui/src/server/agent-auth.ts`
carries the matching warning at the Talaria end.
