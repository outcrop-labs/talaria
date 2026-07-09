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
| `triage_ticket` | Priority, effort, labels, description, due date, status → `in_progress` / `blocked` / `quality_review` |
| `comment` | Comment on a ticket |
| `report_outcome` | Record outcome/resolution and hand the ticket to **quality review** |
| `add_time` | Add seconds to the auto-accumulated time-spent |
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

Configure your agent's MCP client (stdio transport):

```json
{
  "mcpServers": {
    "talaria": {
      "command": "node",
      "args": ["/path/to/talaria/mcp/dist/index.js"],
      "env": {
        "TALARIA_URL": "http://localhost:5273",
        "TALARIA_AGENT_KEY": "<TALARIA_AGENT_KEY from ui/.env>",
        "TALARIA_AGENT_NAME": "sam"
      }
    }
  }
}
```

- `TALARIA_AGENT_KEY` authenticates the fleet (same key the register/heartbeat
  endpoints use).
- `TALARIA_AGENT_NAME` is the agent's fleet model name. It scopes the per-board
  agent policy (an agent only sees/touches boards that allow it) and attributes
  every ticket, comment, and activity entry to the agent.

## How it holds the guardrails

Board access is restrictive by default: an agent sees a board only if the board
allows all agents or lists the agent by name (Board settings → Agents). On the
API side, agent-created tickets are forced to `inbox` with no assignees, `status:
assigned` is rejected, and `status: done` is coerced to `quality_review`. This MCP
server additionally never offers the unsafe inputs, so a well-behaved agent never
even sees them.


## Fleet HTTP mode

Set `MCP_HTTP_PORT` and talaria-mcp serves the WHOLE fleet over stateless
streamable HTTP instead of stdio: each request is handled by a fresh server
bound to the calling agent's identity (`X-Agent-Name` header) and must carry
the fleet key (`X-Api-Key` = `TALARIA_AGENT_KEY`). Talaria runs this mode
itself (`ui/src/server/mcp-service.ts`) and injects the connection into every
rendered agent config — you never start it by hand. Stdio mode (one agent via
`TALARIA_AGENT_NAME`) remains for external clients.
