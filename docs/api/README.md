# API reference

> **Generated** by `bun run docs:api` — do not edit by hand. One file per resource
> group, one row per (path, method), extracted from the Rust router
> (`api/src/routes/mod.rs`) and handler modules (`api/src/routes/**`).
> Requests/responses follow the house envelope and conventions:
> [API-CONVENTIONS.md](../API-CONVENTIONS.md).

224 routes across 23 groups.

| Group | Covers | Routes |
| :--- | :--- | :--- |
| [`account`](./account.md) | Sign-in, session, profile, members. | 13 |
| [`activity`](./activity.md) | What happened, what it cost, what needs you. | 8 |
| [`admin`](./admin.md) | Instance administration (admin session required). | 24 |
| [`agents`](./agents.md) | Agent CRUD, registration, heartbeats, skills, runs. | 15 |
| [`apps`](./apps.md) | The app platform surface and the app-server gateway. | 2 |
| [`boards`](./boards.md) | Kanban boards, members, statuses, labels, views. | 11 |
| [`brief`](./brief.md) | The personal brief: items, replies, delegation. | 5 |
| [`comms`](./comms.md) | Channels, DMs, threads, chat streaming. | 16 |
| [`files`](./files.md) | Uploads, artifacts, shares, downloads. | 13 |
| [`fleet`](./fleet.md) | The agent fleet: defs, containers, crons, federation. | 20 |
| [`inbox`](./inbox.md) | The focus inbox and its command surface. | 7 |
| [`integrations`](./integrations.md) | Connected accounts — Google Workspace and the rest. | 21 |
| [`knowledge`](./knowledge.md) | Knowledge base, RAG collections, org templates, search. | 19 |
| [`llm`](./llm.md) | The OpenAI-compatible wire (llm.v1.*). | 2 |
| [`mcp`](./mcp.md) | MCP servers, governance, gateway, OAuth. | 9 |
| [`models`](./models.md) | Model providers, gateway API keys, local backends. | 5 |
| [`plans`](./plans.md) | Living plans: draft doc, members. | 3 |
| [`research`](./research.md) | Cited research reports and their conversations. | 5 |
| [`secrets`](./secrets.md) | The sealed-secrets vault: folders, shares, reveal, relay. | 6 |
| [`system`](./system.md) | Health and instance discovery endpoints. | 2 |
| [`tasks`](./tasks.md) | Tickets, comments, dependencies, watchers, workflows. | 9 |
| [`teams`](./teams.md) | Teams and their members. | 3 |
| [`workbench`](./workbench.md) | The developer workbench: repos, jobs, harnesses, flows. | 7 |

**Auth vocabulary** (the route's guard class — resource-level ACLs like board
membership or ownership apply on top; see [API-CONVENTIONS.md](../API-CONVENTIONS.md)):

| class | meaning |
| :--- | :--- |
| `public` | no authentication |
| `session` | any signed-in member (`require_user`) |
| `session` + `perm:x` | signed-in member holding permission `x` (`require_perm`) |
| `session` + `view:p` | signed-in member granted view `p` (`require_view`) |
| `admin` | an admin session (`require_admin`) |
| `agent` | an agent credential (`tak_` key, `require_agent`/`agent_caller`) |
| `dual` | session path and agent path both reach the handler |
| `fleet` | internal fleet key (`fleet_caller`/`check_fleet_key`) |
| `bearer-key` | a personal LLM-gateway API key (`authenticate_key`) |

---

The **Returns** column everywhere in this reference is a heuristic: the top-level
keys of the first success-shaped `json!({…})` literal in the handler. `…` means the
shape is computed, not literal. Where a row matters to you, the source link at the
top of its section is the truth.
