# API reference — agents

> **Generated** by `bun run docs:api` from `ui/src/routes/api/**` — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
> The **Returns** column is the first success-shaped `json({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.

14 routes.

| Route | Method | Auth |
| :--- | :--- | :--- |
| [`/api/agent-role-templates`](#apiagent-role-templates) | GET | `session` + `perm:agents.manage` |
| [`/api/agent-role-templates`](#apiagent-role-templates) | PUT | `admin` |
| [`/api/agent-role-templates`](#apiagent-role-templates) | DELETE | `admin` |
| [`/api/agent/gap`](#apiagentgap) | POST | `agent` |
| [`/api/agent/message-user`](#apiagentmessage-user) | POST | `agent` |
| [`/api/agent/problem`](#apiagentproblem) | POST | `agent` |
| [`/api/agents`](#apiagents) | GET | `session` |
| [`/api/agents/{id}/heartbeat`](#apiagentsidheartbeat) | GET | `fleet` |
| [`/api/agents/register`](#apiagentsregister) | POST | `fleet` |
| [`/api/gaps`](#apigaps) | GET | `session` |
| [`/api/gaps/{id}`](#apigapsid) | PUT | `session` + `perm:agents.manage` |
| [`/api/muse`](#apimuse) | POST | `session` |
| [`/api/runs/{id}/events`](#apirunsidevents) | GET | `session` |
| [`/api/skills`](#apiskills) | GET | `session` |
| [`/api/skills/{owner}/{name}`](#apiskillsownername) | GET | `session` |
| [`/api/skills/{owner}/{name}`](#apiskillsownername) | PUT | `session` |
| [`/api/skills/{owner}/{name}`](#apiskillsownername) | POST | `session` |
| [`/api/skills/{owner}/{name}`](#apiskillsownername) | DELETE | `session` |
| [`/api/vision/describe`](#apivisiondescribe) | POST | `dual` |

## `/api/agent-role-templates`

Source: [`ui/src/routes/api/agent-role-templates.ts`](../../ui/src/routes/api/agent-role-templates.ts)

> Agent role templates — the business roles a new agent can start from.
> GET   → built-ins + the org's own (anyone who may create an agent needs it).
> PUT   → create or update an ORG template (admin; it seeds every future agent).
> DELETE → remove an org template; a shadowed built-in reappears.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` + `perm:agents.manage` | — | `{templates}` | 200 | — |
| PUT | `admin` | [body](#put-apiagent-role-templates-body) | `…` | 200 | audit |
| DELETE | `admin` | — | `{ok}` | 200, 400, 404 | audit |

### PUT `/api/agent-role-templates` body

| field | schema | notes |
| :--- | :--- | :--- |
| `slug` | `SLUG` |  |
| `name` | `z.string().min(1).max(80)` |  |
| `role` | `z.string().min(1).max(80)` |  |
| `department` | `DEPT` |  |
| `description` | `z.string().max(300).default('')` |  |
| `soul` | `z.string().min(1).max(20_000)` |  |

## `/api/agent/gap`

Source: [`ui/src/routes/api/agent.gap.ts`](../../ui/src/routes/api/agent.gap.ts)

> POST — an agent reports a capability gap (the honesty loop). Deduped by
> work-shape server-side: repeats bump seen_count, never re-notify. Lands in
> the Studio's Suggested queue; the ticket (if given) gets an audit line.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `agent` | [body](#post-apiagentgap-body) | `{error, message}` | 200, 403 | — |

### POST `/api/agent/gap` body

| field | schema | notes |
| :--- | :--- | :--- |
| `kind` | `z.string().min(2).max(80)` |  |
| `missing` | `z.string().min(5).max(300)` |  |
| `needs` | `z.string().max(5000).optional()` |  |
| `taskId` | `Uuid.optional()` |  |

## `/api/agent/message-user`

Source: [`ui/src/routes/api/agent.message-user.ts`](../../ui/src/routes/api/agent.message-user.ts)

> POST (agent key) → an agent starts or continues a direct conversation with
> a human teammate. The message lands as a normal turn in their chat with
> this agent plus an inbox notification. Personal assistants reach only
> their owner; every agent↔user pair is rate-capped per day.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `agent` | [body](#post-apiagentmessage-user-body) | `{ok, conversationId}` | 200, 400 | — |

### POST `/api/agent/message-user` body

| field | schema | notes |
| :--- | :--- | :--- |
| `to` | `z.string().min(1).max(200)` |  |
| `message` | `z.string().min(1).max(4000)` |  |

## `/api/agent/problem`

Source: [`ui/src/routes/api/agent.problem.ts`](../../ui/src/routes/api/agent.problem.ts)

> POST (agent key) → an agent hit something broken it shouldn't explain to a
> normal person. Talaria elevates it: the admins who may hear it get an alert
> notification, a Helpdesk ticket carries the technical details (board
> find-or-created), and the agent gets plain-language confirmation to relay.
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `agent` | [body](#post-apiagentproblem-body) | `{error, message}` | 200, 403 | — |

### POST `/api/agent/problem` body

| field | schema | notes |
| :--- | :--- | :--- |
| `summary` | `z.string().min(5).max(300)` |  |
| `details` | `z.string().max(20_000).optional()` |  |
| `context` | `z.string().max(500).optional().describe('what the agent was trying to do')` |  |
| `taskId` | `Uuid.optional().describe('the ticket the agent was working when it broke')` |  |

## `/api/agents`

Source: [`ui/src/routes/api/agents.ts`](../../ui/src/routes/api/agents.ts)

> GET /api/agents → the fleet the current user may use (definition-backed
> agents with their model tiers, filtered by per-agent access). Auth-gated.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{agents}` | 200 | — |

## `/api/agents/{id}/heartbeat`

Source: [`ui/src/routes/api/agents.$id.heartbeat.ts`](../../ui/src/routes/api/agents.$id.heartbeat.ts)

> GET /api/agents/:id/heartbeat — refresh last_seen and return the agent's
> assigned work (tasks assigned to it, across boards). MC-compatible.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `fleet` | — | `{work_items}` | 200, 401, 403, 404 | — |

## `/api/agents/register`

Source: [`ui/src/routes/api/agents.register.ts`](../../ui/src/routes/api/agents.register.ts)

> POST /api/agents/register — an agent registers with Talaria (MC-compatible
> contract, so the existing plugin works repointed). Agent-key auth.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `fleet` | [body](#post-apiagentsregister-body) | `{registered}` | 200, 401 | — |

### POST `/api/agents/register` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `z.string().min(1).max(200)` |  |
| `role` | `z.string().max(80).optional()` |  |
| `capabilities` | `z.array(z.string()).optional()` |  |
| `framework` | `z.string().max(80).optional()` |  |

## `/api/gaps`

Source: [`ui/src/routes/api/gaps.ts`](../../ui/src/routes/api/gaps.ts)

> The Studio's Suggested queue: capability gaps agents have reported, ranked
> by how often the work-shape recurs. Any member reads (the queue is what
> invites people to tailor); status changes live on /api/gaps/$id.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{gaps}` | 200 | — |

## `/api/gaps/{id}`

Source: [`ui/src/routes/api/gaps.$id.ts`](../../ui/src/routes/api/gaps.$id.ts)

> One capability gap: PUT status (open | dismissed | resolved) — agents.manage.
> Dismissed shapes that keep recurring reopen automatically; resolved sticks.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| PUT | `session` + `perm:agents.manage` | [body](#put-apigapsid-body) | `{ok}` | 200 + varies | — |

### PUT `/api/gaps/{id}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `status` | `z.enum(['open', 'dismissed', 'resolved'])` |  |

## `/api/muse`

Source: [`ui/src/routes/api/muse.ts`](../../ui/src/routes/api/muse.ts)

> POST → a validated JSON draft (cron / agent / ticket / skillForm /
> templateForm) or a streamed document.
> Runs on the caller's muse model, metered as `platform:muse:<user>`. Any
> signed-in user; what they can DO with the draft is still governed by the save
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` | [body](#post-apimuse-body) | `{value, model}` | 200, 400, 502 | — |

### POST `/api/muse` body

| field | schema | notes |
| :--- | :--- | :--- |
| `kind` | `z.enum(['soul', 'personality', 'skill', 'memory', 'cron', 'agent', 'document', 'template', 'ticket', 'skillForm', 'templateForm'])` |  |
| `instruction` | `z.string().trim().min(1).max(8_000)` |  |
| `current` | `z.string().max(300_000).optional()` |  |
| `context` | `z.string().max(2_000).optional()` |  |
| `chat` | `z.array(z.object({ role: z.enum(['user', 'assistant']), content: z.string().max(300_000) })).max(24).optional()` |  |

## `/api/runs/{id}/events`

Source: [`ui/src/routes/api/runs.$id.events.ts`](../../ui/src/routes/api/runs.$id.events.ts)

> GET /api/runs/:id/events → SSE stream of one run's live transitions (state,
> phase, terminal error). Auth-gated by the run's read ACL. This is what makes
> a long action attachable: a tab that was closed, a view that was navigated
> away from, or a second device can re-attach to the SAME server-owned record
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200, 403 | SSE |

## `/api/skills`

Source: [`ui/src/routes/api/skills.ts`](../../ui/src/routes/api/skills.ts)

> Skills across the fleet: shared + per-agent, straight from the mounts the
> agents actually read. Any member reads (the library grounds the Studio and
> what agents will be told); each owner carries canEdit for THIS user —
> admins/agents.manage everywhere, explicit user_agent_access grants (or a
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{owners}` | 200 | — |

## `/api/skills/{owner}/{name}`

Source: [`ui/src/routes/api/skills.$owner.$name.ts`](../../ui/src/routes/api/skills.$owner.$name.ts)

> One skill's SKILL.md. GET → content + file list (any member — the library
> is org work material). PUT → save (creates the skill if new). DELETE →
> remove the whole skill dir. Writes go through canEditSkills: admin /
> agents.manage everywhere; personal-assistant owners and explicit
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200, 404 | — |
| PUT | `session` | [body](#put-apiskillsownername-body) | `{ok}` | 200, 400, 403 | — |
| POST | `session` | [body](#post-apiskillsownername-body) | `{ok}` | 200, 400, 403 | — |
| DELETE | `session` | — | `{ok}` | 200, 400, 403 | — |

### PUT `/api/skills/{owner}/{name}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `content` | `z.string().max(500_000)` |  |

### POST `/api/skills/{owner}/{name}` body — variant 1

| field | schema | notes |
| :--- | :--- | :--- |
| `op` | `z.literal('rename')` |  |
| `toName` | `NAME` |  |

### POST `/api/skills/{owner}/{name}` body — variant 2

| field | schema | notes |
| :--- | :--- | :--- |
| `op` | `z.literal('copy')` |  |
| `toOwner` | `z.string().min(1).max(80)` |  |
| `toName` | `NAME.optional()` |  |

### POST `/api/skills/{owner}/{name}` body — variant 3

| field | schema | notes |
| :--- | :--- | :--- |
| `op` | `z.literal('move')` |  |
| `toOwner` | `z.string().min(1).max(80)` |  |
| `toName` | `NAME.optional()` |  |

## `/api/vision/describe`

Source: [`ui/src/routes/api/vision.describe.ts`](../../ui/src/routes/api/vision.describe.ts)

> READ AN IMAGE ON BEHALF OF A MODEL THAT CANNOT — the endpoint behind the
> `describe_image` tool.
>
> THE ACCESS CHECK IS THE WHOLE SECURITY STORY HERE, and it is not this file's
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `dual` | [body](#post-apivisiondescribe-body) | `{description, model}` | 200, 400, 404, 503 | — |

### POST `/api/vision/describe` body

| field | schema | notes |
| :--- | :--- | :--- |
| `uploadId` | `z.string().min(1).max(200)` |  |
| `question` | `z.string().min(3).max(500)` |  |

