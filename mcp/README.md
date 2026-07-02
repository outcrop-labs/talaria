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
        "TALARIA_AGENT_NAME": "sam-support"
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
