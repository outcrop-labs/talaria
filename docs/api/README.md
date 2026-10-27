# API reference

> **Generated** by `bun run docs:api` — do not edit by hand. One file per resource
> group, one row per (path, method), extracted from the Rust router
> (`api/src/routes/mod.rs`) and handler modules (`api/src/routes/**`).
> Requests/responses follow the house envelope and conventions:
> [API-CONVENTIONS.md](../API-CONVENTIONS.md).

342 routes across 24 groups.

| Group | Covers | Routes |
| :--- | :--- | :--- |
| [`account`](./account.md) | Sign-in, session, profile, members. | 18 |
| [`activity`](./activity.md) | What happened, what it cost, what the machine is doing, what needs you. | 11 |
| [`admin`](./admin.md) | Instance administration (admin session required). | 28 |
| [`agents`](./agents.md) | Agent CRUD, skills, runs. | 27 |
| [`apps`](./apps.md) | The app platform surface, the app-server gateway, and the host's own app routes. | 7 |
| [`boards`](./boards.md) | Kanban boards, members, statuses, labels, views, workchains. | 19 |
| [`brief`](./brief.md) | The personal brief: items, replies, delegation. | 5 |
| [`comms`](./comms.md) | Channels, DMs, threads, chat streaming. | 32 |
| [`files`](./files.md) | Uploads, artifacts, shares, downloads. | 19 |
| [`fleet`](./fleet.md) | The agent fleet: defs, containers, crons, federation. | 26 |
| [`inbox`](./inbox.md) | The focus inbox and its command surface. | 7 |
| [`integrations`](./integrations.md) | Connected accounts — Google Workspace and the rest. | 42 |
| [`internal`](./internal.md) | Service-to-service calls from the app host — never a user's or an agent's. | 3 |
| [`knowledge`](./knowledge.md) | Knowledge base, RAG collections, org templates, blueprints, search. | 20 |
| [`llm`](./llm.md) | The OpenAI-compatible wire (llm.v1.*). | 2 |
| [`mcp`](./mcp.md) | MCP servers, governance, gateway, OAuth. | 16 |
| [`models`](./models.md) | Model providers, gateway API keys, local backends. | 5 |
| [`plans`](./plans.md) | Living plans: draft doc, members. | 4 |
| [`research`](./research.md) | Cited research reports and their conversations. | 6 |
| [`secrets`](./secrets.md) | The sealed-secrets vault: folders, shares, reveal, relay. | 7 |
| [`system`](./system.md) | Health and instance discovery endpoints. | 2 |
| [`tasks`](./tasks.md) | Tickets, comments, dependencies, watchers, workflows. | 12 |
| [`teams`](./teams.md) | Teams and their members. | 6 |
| [`workbench`](./workbench.md) | The developer workbench: repos, jobs, harnesses, flows. | 18 |

**Auth vocabulary** (the route's guard class — resource-level ACLs like board
membership or ownership apply on top; see [API-CONVENTIONS.md](../API-CONVENTIONS.md)):

| class | meaning |
| :--- | :--- |
| `public` | no authentication |
| `session` | any signed-in member (`require_user`) |
| `session` + `perm:x` | signed-in member holding permission `x` (`require_perm`) |
| `session` + `view:p` | signed-in member granted view `p` (`require_view`) |
| `session` + `agent-manager` | a manager of the agent in the path, or an admin (`require_agent_manager`) |
| `session` + `agent-reader` | the above, or anyone who reads the whole fleet — `agents.manage` (`require_agent_reader`) |
| `admin` | an admin session (`require_admin`) |
| `agent` | an agent credential (`tak_` key, `require_agent`/`agent_caller`) |
| `dual` | session path and agent path both reach the handler |
| `internal` | the root-derived service credential (`x-talaria-internal`) between the app host and the api — `require_internal` on the api's routes, `requireInternal` (loopback only) on the host's — never a user or agent |
| `bearer-key` | a personal LLM-gateway API key (`authenticate_key`) |

---

The **Returns** column everywhere in this reference is a heuristic: the top-level
keys of the first success-shaped `json!({…})` literal in the handler. `…` means the
shape is computed, not literal. Where a row matters to you, the source link at the
top of its section is the truth.
