# API reference — agents

> **Generated** by `bun run docs:api` from the Rust router table (`api/src/routes/mod.rs`)
> and the handler modules under `api/src/routes/**` (the TS residents still serving
> `healthz` and the app dispatch excepted) — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
> The **Returns** column is the first success-shaped `json!({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.

20 routes.

| Route | Method | Auth |
| :--- | :--- | :--- |
| [`/api/agent-role-templates`](#apiagent-role-templates) | GET | `session` + `perm:agents.manage` |
| [`/api/agent-role-templates`](#apiagent-role-templates) | PUT | `admin` |
| [`/api/agent-role-templates`](#apiagent-role-templates) | DELETE | `admin` |
| [`/api/agent/gap`](#apiagentgap) | POST | `agent` |
| [`/api/agent/message-user`](#apiagentmessage-user) | POST | `agent` |
| [`/api/agent/problem`](#apiagentproblem) | POST | `agent` |
| [`/api/agent/whoami`](#apiagentwhoami) | GET | `agent` |
| [`/api/agents`](#apiagents) | GET | `session` |
| [`/api/agents/{id}/heartbeat`](#apiagentsidheartbeat) | GET | `fleet` |
| [`/api/agents/register`](#apiagentsregister) | POST | `fleet` |
| [`/api/agents/tool-events`](#apiagentstool-events) | POST | `agent` |
| [`/api/gaps`](#apigaps) | GET | `session` |
| [`/api/gaps/{id}`](#apigapsid) | PUT | `session` + `perm:agents.manage` |
| [`/api/muse`](#apimuse) | POST | `session` |
| [`/api/runs/{id}/events`](#apirunsidevents) | GET | `session` |
| [`/api/runs/{id}/watch`](#apirunsidwatch) | GET | `session` |
| [`/api/skills`](#apiskills) | GET | `session` |
| [`/api/skills/{owner}/{name}`](#apiskillsownername) | GET | `session` |
| [`/api/skills/{owner}/{name}`](#apiskillsownername) | POST | `session` |
| [`/api/skills/{owner}/{name}`](#apiskillsownername) | PUT | `session` |
| [`/api/skills/{owner}/{name}`](#apiskillsownername) | DELETE | `session` |
| [`/api/skills/marketplace`](#apiskillsmarketplace) | GET | `session` |
| [`/api/skills/marketplace/detail`](#apiskillsmarketplacedetail) | GET | `session` |
| [`/api/skills/marketplace/install`](#apiskillsmarketplaceinstall) | POST | `session` |
| [`/api/vision/describe`](#apivisiondescribe) | POST | `dual` |

## `/api/agent-role-templates`

Source: [`api/crates/talaria-routes-fleet/src/agents/agent_role_templates.rs`](../../api/crates/talaria-routes-fleet/src/agents/agent_role_templates.rs)

> /api/agent-role-templates. The business roles a new agent can start from.
>   GET    → built-ins + the org's own (anyone who may create an agent).
>   PUT    → create or update an ORG template (admin; it seeds every future agent).
>   DELETE → remove an org template; a shadowed built-in reappears.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` + `perm:agents.manage` | — | `{templates}` | 200 | — |
| PUT | `admin` | [body](#put-apiagent-role-templates-body) | `{template}` | 200, 400 | audit |
| DELETE | `admin` | — | `{ok}` | 200, 400, 404 | audit |

### PUT `/api/agent-role-templates` body

| field | schema | notes |
| :--- | :--- | :--- |
| `slug` | `kebab()` |  |
| `name` | `string(1, 80)` |  |
| `role` | `string(1, 80)` |  |
| `department` | `kebab()` |  |
| `description` | `string(0, 300)` |  |
| `soul` | `string(1, 20000)` |  |

## `/api/agent/gap`

Source: [`api/crates/talaria-routes-fleet/src/agents/agent_gap.rs`](../../api/crates/talaria-routes-fleet/src/agents/agent_gap.rs)

> /api/agent/gap. POST — an agent reports a capability gap (the honesty loop). Deduped by
> work-shape server-side: repeats bump seen_count, never re-notify. Lands in
> the Studio's Suggested queue; the ticket (if given) gets an audit line.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `agent` | [body](#post-apiagentgap-body) | `{ok, seenCount, note}` | 200, 400, 403 | — |

### POST `/api/agent/gap` body

| field | schema | notes |
| :--- | :--- | :--- |
| `kind` | `string(2, 80)` |  |
| `missing` | `string(5, 300)` |  |
| `needs` | `string?(5000)` |  |
| `taskId` | `uuid?` |  |

## `/api/agent/message-user`

Source: [`api/crates/talaria-routes-fleet/src/agents/agent_message_user.rs`](../../api/crates/talaria-routes-fleet/src/agents/agent_message_user.rs)

> /api/agent/message-user. POST (agent key) → an agent starts or continues a direct conversation with
> a human teammate. The message lands as a normal turn in their chat with
> this agent plus an inbox notification. Personal assistants reach only
> their owner; every agent↔user pair is rate-capped per day.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `agent` | [body](#post-apiagentmessage-user-body) | `{ok, conversationId}` | 200, 400 | — |

### POST `/api/agent/message-user` body

| field | schema | notes |
| :--- | :--- | :--- |
| `to` | `string(1, 200)` | Teammate's email (preferred) or exact display name. |
| `message` | `string(1, 4000)` |  |

## `/api/agent/problem`

Source: [`api/crates/talaria-routes-fleet/src/agents/agent_problem.rs`](../../api/crates/talaria-routes-fleet/src/agents/agent_problem.rs)

> /api/agent/problem. POST (agent key) → an agent hit something broken it
> shouldn't explain to a normal person. Talaria elevates it: the admins who
> may hear it get an alert notification, a Helpdesk ticket carries the
> technical details (board find-or-created), and the agent gets plain-language
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `agent` | [body](#post-apiagentproblem-body) | `{ok, ticket, relay}` | 200, 400, 403 | — |

### POST `/api/agent/problem` body

| field | schema | notes |
| :--- | :--- | :--- |
| `summary` | `string(5, 300)` |  |
| `details` | `string?(20000)` |  |
| `context` | `string?(500)` | what the agent was trying to do |
| `taskId` | `uuid?` | the ticket the agent was working when it broke |

## `/api/agent/whoami`

Source: [`api/crates/talaria-routes-fleet/src/agents/agent_whoami.rs`](../../api/crates/talaria-routes-fleet/src/agents/agent_whoami.rs)

> /api/agent/whoami. GET (agent key) → who is calling and what it may touch.
>
> THE ROUTE AN AGENT PROBES WITH. Before this, the only way an agent could
> learn its own reach was to try a verb and read the 403 — Gregosaurus's
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `agent` | — | `{boardId, boardName, status, createdAt}` | 200 | — |

## `/api/agents`

Source: [`api/crates/talaria-routes-fleet/src/agents/agents.rs`](../../api/crates/talaria-routes-fleet/src/agents/agents.rs)

> GET /api/agents. The fleet the current user may use (definition-backed
> agents with their model tiers, filtered by per-agent access). Auth-gated.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200 | — |

## `/api/agents/{id}/heartbeat`

Source: [`api/crates/talaria-routes-fleet/src/agents/agents_id_heartbeat.rs`](../../api/crates/talaria-routes-fleet/src/agents/agents_id_heartbeat.rs)

> GET /api/agents/{id}/heartbeat. Refresh last_seen and return the agent's
> assigned work (tasks assigned to it, across boards). MC-compatible.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `fleet` | — | `{work_items}` | 200, 401, 403, 404 | — |

## `/api/agents/register`

Source: [`api/crates/talaria-routes-fleet/src/agents/agents_register.rs`](../../api/crates/talaria-routes-fleet/src/agents/agents_register.rs)

> POST /api/agents/register. An agent registers with Talaria (MC-compatible
> contract, so the existing plugin works repointed). Agent-key auth.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `fleet` | [body](#post-apiagentsregister-body) | `{agent, registered}` | 200, 400, 401 | — |

### POST `/api/agents/register` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `string(1, 200)` |  |
| `role` | `string?(80)` |  |
| `framework` | `string?(80)` |  |

## `/api/agents/tool-events`

Source: [`api/crates/talaria-routes-fleet/src/agents/tool_events.rs`](../../api/crates/talaria-routes-fleet/src/agents/tool_events.rs)

> POST /api/agents/tool-events. The inbound half of the talaria-events
> Hermes plugin: the agent's plugin reports each tool call's lifecycle
> (name, args, RESULT — the thing no other wire carries) and this route
> lands it on the agent's live run's watch stream, where the run-detail
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `agent` | — | `{ok, landed}` | 200, 400 | — |

## `/api/gaps`

Source: [`api/crates/talaria-routes-fleet/src/agents/gaps.rs`](../../api/crates/talaria-routes-fleet/src/agents/gaps.rs)

> /api/gaps. GET → the Studio's Suggested queue: capability gaps agents have reported,
> ranked by how often the work-shape recurs. Any member reads (the queue is
> what invites people to tailor); status changes live on /api/gaps/{id}.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{gaps}` | 200 | — |

## `/api/gaps/{id}`

Source: [`api/crates/talaria-routes-fleet/src/agents/gaps_id.rs`](../../api/crates/talaria-routes-fleet/src/agents/gaps_id.rs)

> /api/gaps/{id}. PUT status (open | dismissed | resolved) — agents.manage. Dismissed shapes
> that keep recurring reopen automatically; resolved sticks.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| PUT | `session` + `perm:agents.manage` | [body](#put-apigapsid-body) | `{ok}` | 200, 400 | — |

### PUT `/api/gaps/{id}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `status` | `string(1, 20)` | status: exactly one of the three literals — anything else is a 400 before the row is touched. |

## `/api/muse`

Source: [`api/crates/talaria-routes-fleet/src/agents/muse.rs`](../../api/crates/talaria-routes-fleet/src/agents/muse.rs)

> /api/muse.
>
> The Muse endpoint. ONE route, TWO answers, because the Muse does two
> genuinely different things and used to pretend they were one:
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apimuse-body) | `…` | 200, 400, 500, 502 | SSE |

### POST `/api/muse` body

| field | schema | notes |
| :--- | :--- | :--- |
| `kind` | `enum(soul|personality|skill|memory|cron|agent|document|template|ticket|skillForm|templateForm)` |  |
| `current` | `string?(300000)` |  |
| `context` | `string?(2000)` |  |

## `/api/runs/{id}/events`

Source: [`api/crates/talaria-routes-fleet/src/agents/runs_events.rs`](../../api/crates/talaria-routes-fleet/src/agents/runs_events.rs)

> GET /api/runs/{id}/events → SSE stream of one run's live transitions (state,
> phase, terminal error). Auth-gated by the run's read ACL. This is what makes
> a long action
> attachable: a tab that was closed, a view that was navigated away from, or a
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200, 403 | SSE |

## `/api/runs/{id}/watch`

Source: [`api/crates/talaria-routes-fleet/src/agents/runs_watch.rs`](../../api/crates/talaria-routes-fleet/src/agents/runs_watch.rs)

> GET /api/runs/{id}/watch → SSE of the run's WORK TERMINAL: the agent's
> own stream events (words as they land, tool calls as they start) as one
> JSON line per `data:` frame — first a bounded replay of the current
> turn's tail, then live frames as they publish. Same ACL as the run's
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200, 403 | SSE |

## `/api/skills`

Source: [`api/crates/talaria-routes-fleet/src/agents/skills.rs`](../../api/crates/talaria-routes-fleet/src/agents/skills.rs)

> /api/skills.
> Skills across the fleet: shared + per-agent, straight from the mounts the
> agents actually read. Any member reads (the library grounds the Studio and
> what agents will be told); each owner carries canEdit for THIS user —
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{owners}` | 200 | — |

## `/api/skills/{owner}/{name}`

Source: [`api/crates/talaria-routes-fleet/src/agents/skills_owner_name.rs`](../../api/crates/talaria-routes-fleet/src/agents/skills_owner_name.rs)

> /api/skills/{owner}/{name}. One skill's SKILL.md. GET → content + file list (any member — the library
> is org work material). PUT → save (creates the skill if new). DELETE →
> remove the whole skill dir. Writes go through canEditSkill(s): admin /
> agents.manage everywhere; personal-assistant owners and explicit
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{content, files}` | 200, 404 | — |
| POST | `session` | [body](#post-apiskillsownername-body) | `{ok}` | 200, 400, 403 | — |
| PUT | `session` | [body](#put-apiskillsownername-body) | `{ok}` | 200, 400, 403 | — |
| DELETE | `session` | — | `{ok}` | 200, 400, 403 | — |

### POST `/api/skills/{owner}/{name}` body

Body is validated imperatively (`obj.get` dispatch / element-wise walks), not
through the `crate::body` member vocabulary — the field set lives in the route
source.

### PUT `/api/skills/{owner}/{name}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `content` | `string(0, 500000)` | content: required, max 500_000 — the empty string is legal. |

## `/api/skills/marketplace`

Source: [`api/crates/talaria-routes-fleet/src/agents/skills_marketplace.rs`](../../api/crates/talaria-routes-fleet/src/agents/skills_marketplace.rs)

> /api/skills/marketplace — the Hermes Atlas skill catalog for the Studio's
> picker: GET ?q= (the ranked list, filtered), GET detail?repo= (one GitHub
> repo's discovered skills), POST install (write them into an owner's skill
> root). Reads are any member's (the Studio is a member surface, like the
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{entries}` | 200, 502 | — |

## `/api/skills/marketplace/detail`

Source: [`api/crates/talaria-routes-fleet/src/agents/skills_marketplace.rs`](../../api/crates/talaria-routes-fleet/src/agents/skills_marketplace.rs)

> /api/skills/marketplace — the Hermes Atlas skill catalog for the Studio's
> picker: GET ?q= (the ranked list, filtered), GET detail?repo= (one GitHub
> repo's discovered skills), POST install (write them into an owner's skill
> root). Reads are any member's (the Studio is a member surface, like the
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{repo, skills}` | 200, 400, 502 | — |

## `/api/skills/marketplace/install`

Source: [`api/crates/talaria-routes-fleet/src/agents/skills_marketplace.rs`](../../api/crates/talaria-routes-fleet/src/agents/skills_marketplace.rs)

> /api/skills/marketplace — the Hermes Atlas skill catalog for the Studio's
> picker: GET ?q= (the ranked list, filtered), GET detail?repo= (one GitHub
> repo's discovered skills), POST install (write them into an owner's skill
> root). Reads are any member's (the Studio is a member surface, like the
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apiskillsmarketplaceinstall-body) | `{ok, owner, repo, results}` | 200, 400, 403, 502 | — |

### POST `/api/skills/marketplace/install` body

| field | schema | notes |
| :--- | :--- | :--- |
| `owner` | `string(1, 80)` |  |
| `repo` | `string(1, 200)` |  |

## `/api/vision/describe`

Source: [`api/crates/talaria-routes-fleet/src/agents/vision_describe.rs`](../../api/crates/talaria-routes-fleet/src/agents/vision_describe.rs)

> /api/vision/describe.
>
> READ AN IMAGE ON BEHALF OF A MODEL THAT CANNOT — the endpoint behind the
> `describe_image` tool.
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `dual` | [body](#post-apivisiondescribe-body) | `{description, model}` | 200, 400, 404, 503 | — |

### POST `/api/vision/describe` body

| field | schema | notes |
| :--- | :--- | :--- |
| `uploadId` | `string(1, 200)` |  |
| `question` | `string(3, 500)` |  |

