# API conventions — one dialect for every route

The contract that keeps 214 routes predictable. Swept across the whole tree (2026-07); new routes
follow it from day one. Routes live in `ui/src/routes/api/`. A route's path is the string in its
`defineApi('…')` call — the filename is convention that mirrors it (`admin.model-fitness.ts`
declares `defineApi('/api/admin/model-fitness')`). Server logic in `ui/src/server/`.

File placement is also why `vitest.config.ts` excludes `src/routes/**`: every module under
`routes/api/` that exports `Route` goes live — `mcp.test.ts` really serves `/api/mcp/test`, the
glob has no test exclusion. **Nothing under `routes/` can be unit tested**, so a
route parses the request, calls ONE function in `src/server/*`, and serializes the result. A decision
that lives in a route is a decision with no test — `routes/api/admin.model-fitness.ts` carried ~920
lines of them until they moved to `server/fitness/surface.ts`.

The full per-route reference — path, method, auth class, body fields, statuses —
is generated from the route sources: [`api/`](./api/README.md). Nothing here
repeats it; this page is the dialect, that one is the dictionary.

The dialect lives in one runtime: the Rust crate (`api/`) serves every
`/api/*` route except the four permanent TS residents (healthz,
`/api/admin/update`, the `/api/apps/` dispatch subtree, the app-MCP gateway),
and it holds itself to this same dialect byte-for-byte — the 370-pair parity
battery proved it against the TS oracle before the TS API was deleted (the
Rust-side authoring rules and the recorded divergences are
[`RUST-MIGRATION.md`](./RUST-MIGRATION.md)). A new route lands as a Rust route
file, in this dialect, with its tests.

## Guards — `server/api-guard.ts`

Every session route opens with exactly one of these; each returns the user or a ready 401/403
`Response`:

```ts
const user = await requireUser(request)         // signed-in
const user = await requireAdmin(request)        // role admin
const user = await requirePerm(request, 'agents.manage')  // permission catalog
const user = await requireView(request, '/observability') // admin OR granted the view
if (user instanceof Response) return user
```

Resource ACLs (board membership, KB perms, ownership) stay inline after the guard — guards answer
"who are you / what class of thing may you do", ACLs answer "may you do it to THIS".

Non-session auth: agent routes resolve the caller from its own `tak_` credential via
`server/agent-auth.ts` — never from a header. Same `instanceof Response` shape as the guards:

```ts
const agent = await requireAgent(request)   // agent-only routes: caller or 401
if (agent instanceof Response) return agent

const agent = await agentCaller(request)    // dual-auth: null → fall through to session auth
if (agent instanceof Response) return agent // a credential WAS presented and rejected
if (agent) { … }                            // agent.id · agent.model · agent.legacy
```

`x-agent-name` is a cross-check that can narrow access, never grant it; a name contradicting the
credential is a 403. `agent.legacy` marks a caller authenticated by the org-wide `TALARIA_AGENT_KEY`
— the pre-per-agent-key path; identity asserted, not proven, so anything granting privilege must
refuse it.

Fleet-plane endpoints carry their subject in the URL or body, and they take one of two guards — not
the same one:

```ts
if (!(await checkFleetKey(request))) return …   // agents/register: ANY fleet credential.
                                                // An agent registers before it has its own.

const caller = await fleetCaller(request)       // agents/$id/heartbeat: same validation as
if (caller instanceof Response) return caller   // agentCaller, but an UNNAMED legacy caller
if (!caller) return …                           // resolves to { model: null } instead of a 400.
if (caller.model && caller.model !== name)      // A caller we CAN name must match the subject —
  return json({ error: … }, { status: 403 })    // that's what stops A reading B's work queue.
```

`checkFleetKey` answers "does this credential belong to the fleet" and nothing else, so it is only
correct where the response carries no per-agent data. Public routes say so in a comment; `llm.v1.*`
follows the OpenAI wire contract and is exempt from all of this.

A helper that takes an agent but isn't threaded everywhere yet accepts `AgentSubject`
(`AgentCaller | string`) and asks `subjectProven(subject)` / `subjectModel(subject)` rather than
reaching for `.legacy` on something that might be a bare model string.

## Agent writes that touch a ticket — import the predicate, never re-derive it

Every agent patch that goes through `updateTask` inherits the HITL invariant automatically:
`agentSafePatch` (`server/tasks.ts`) strips assignment/planning/archival, redirects terminal moves to
the board's review column, and refuses the rest. **A route that goes through `updateTask` needs
nothing else.**

A route that writes something *attached* to a ticket without going through `updateTask` — a usage
row, a dependency edge, a gap report, a workbench plan comment or PR title — does not inherit it, and
must ask the one exported predicate:

```ts
import { agentTicketRefusal } from '@/server/tasks'

const shut = await agentTicketRefusal(task, caller, 'write')  // reason string, or null when it may
if (shut) return json({ error: shut }, { status: 403 })
```

It returns the **reason** rather than a boolean so the refusal reads the same wherever the write
arrived, and it covers **four** conditions, not one: the board's **agent policy** (revoked, never
granted, board gone), archived ticket, archived **board**, and — for `'write'` only — closed status
(a `done` column on that board, or the off-board `failed` / `cancelled`). The agent is a **required**
argument: `closedToAgents`, its predecessor, took only the ticket and so could not ask the policy
half, which is how a board owner revoking a grant 403'd every write route while the heartbeat kept
serving the same ticket forever.

The third argument is the intent. `'comment'` skips the closed-status clause and nothing else —
commenting stays open on work an agent can no longer edit, which is deliberate — but archival stops
it at **both** levels, because archival withdraws the work rather than closing it, and a channel
nobody is watching is not a channel. Reads are not this question: an agent that passes
`boardAllowsAgent` may read the ticket, because reading changes nothing.

Pass a `boardFacts()` as the optional fourth argument when you are in a **loop** — it is a per-pass
cache (board archival, agent policy, `statusMeta`), never an answer, so it cannot be for the wrong
board. Creating one per request is free; not passing one in a loop is an N+1.

The same predicate answers the *pull* and *session* sides too — `maybeDispatchTicket`, `assignedWork`
and the work-session loop all ask it before handing (or continuing to hand) an agent work — so the
queue, the live session and the write routes agree by construction rather than by three people
remembering.

> **Do not copy it into your route.** Four routes each hand-rolled their own copy, because parallel
> work split file ownership and `server/tasks.ts` did not export one. All four checked only the
> closed-status third and silently missed both archival clauses the original grew later — so an agent
> could keep writing to a ticket a person had archived through any of them. Duplicating an invariant
> is how the sixth laundering path in a series of six got created; a fifth copy is never the fix.
> Comments are deliberately outside this gate: commenting stays open on work an agent can no longer
> edit.

### CI fails a second definition — `scripts/check-invariants.mjs`

The paragraph above was true and documented before four routes hand-rolled the predicate anyway, so
it is no longer only a convention. `node scripts/check-invariants.mjs` runs as the first job on every
PR (`.github/workflows/ci.yml`, no install step, seconds) and **fails the build** on:

| pattern | what to write instead |
| --- | --- |
| a second `function agentTicketRefusal` / `const agentTicketRefusal` anywhere outside `server/tasks.ts` | `import { agentTicketRefusal } from '@/server/tasks'` |
| `s.category === 'active'` outside `server/statuses.ts` — an active-column lookup re-derived from `listStatuses` | `meta.activeKey` (where a ticket goes while it is worked) or `meta.workingKeys` (is it still in play?) — both picked from the one `placeable` list, so neither can be a terminal column |
| `doneKeys.includes(k) \|\| OFF_BOARD_STATUSES.includes(k)` outside `server/statuses.ts` | `meta.terminal(k)` — `statusMeta()` returns it as a **function** precisely so it cannot be half-copied |
| a bare `doneKeys.includes(...)` outside the two places on the census | `meta.terminal(k)`, unless you truly mean the narrower "is this a done-**category** column", in which case say so on the census |
| `'failed'` next to `'cancelled'` as a literal or a type union, outside the two declarations | `import { OFF_BOARD_STATUSES } from '@/lib/task-const'` |
| `if (!r.ok) return []` / `null` / `{}`, `r.ok ? … : []`, or `.catch(() => [])` **inside a `queryFn`** | let it throw — a resolved-on-failure query reports SUCCESS carrying emptiness, and the UI renders "nothing here" for "we could not ask" |

Every failure names the file, the line and the fix. The last row is not an API rule but it is the
same disease on the read side, so it lives in the same check.

Two mechanisms back the table. **Rules** are absolute: legal only in the file that defines the thing.
**Censuses** are exact counts for patterns that are legitimate in a named few places — a file that
gains an occurrence fails, *and a file that loses its last one also fails*, so the list shrinks as
debt is paid and can never settle into a standing amnesty. If a match is genuinely wrong, argue it in
the PR; widening the pattern to go green is how the previous six rounds happened.

**Adding a rule is cheap and expected.** When a review finds an invariant that was re-derived by
hand, centralize it *and* add the pattern — the structural fix alone rests entirely on the next
reviewer noticing, which is exactly the assumption that has failed seven times.

### The one off-board status list

`OFF_BOARD_STATUSES` (`failed`, `cancelled`) is declared in **`ui/src/lib/task-const.ts`**, the file
both halves may import: the client cannot reach `server/`, but `server/` already imports from
`@/lib`. `TaskStatus` derives from it, so the type union cannot drift from the list.

`server/statuses.ts` still carries its own copy next to the resolvers that exclude it; CI fails if
the two ever disagree. The fix is one line — make that file import the shared constant — and the
cross-check then finds nothing to compare and retires itself.

On the client the matching question — "is this ticket finished **on this board**?" — is
`isClosedStatus(key, statuses)` in `components/board/field-pills.ts`. It is board-aware, so it is
the only correct answer once custom statuses exist: a board whose done column is `shipped` has no
`done` key at all, and `t.status === 'done'` is then permanently false. Import it rather than
comparing to a literal.

### Columns you could not read are not columns you may invent

Every board view resolves `useBoardStatuses(boardId)` and falls back to `TASK_STATUSES` when the
result is empty. That fallback cannot tell "this board has no custom statuses" from "the read
failed", and the two have opposite correct behaviours: the first is a normal board, the second is a
**made-up workflow**. A ticket whose status is not in the invented set renders in no column at all —
work that has silently disappeared from the board, with no error anywhere on screen.

So a surface that draws tickets *into* columns must refuse rather than guess:

```ts
const statusesQuery = useBoardStatuses(board.id)
const statusesFailed = statusesQuery.isError && statusesQuery.data === undefined
if (statusesFailed) return <QueryError title="Could not load this board’s columns" … />
```

The `data === undefined` half is what makes it a refusal and not a regression: a *cached* set that
failed to refresh is stale, not absent, and stale columns beat no board at all — that case gets the
inline banner instead. `kanban.tsx` and `board-list.tsx` do this. `gantt.tsx` does **not** (see the
note in `ci.yml`); it warns and draws the chart anyway.

### Wire numerics — `numeric` and `bigint` arrive as strings

postgres.js hands `numeric` and `int8` back as **strings**, and there is no `types` override on the
pool, so `estimated_hours` and `time_spent_seconds` are strings by the time they reach a component
that has them typed `number`. Declare them `PgNumeric` (`@/lib/task-const`) and read them through
`pgNum` / `pgNumOr` / `taskTimeSpent`.

This is not pedantry, and it is mostly invisible to `tsc` — implicit coercion means the arithmetic
compiles and silently misbehaves:

- `patch.estimatedHours !== cur.estimatedHours` in `updateTask` compares a number from the request
  body against a string from the row, so re-saving an unchanged estimate logs a spurious activity
  line every time.
- `sum + t.estimatedHours` concatenates instead of adding — `"04.5"`.
- A comparator returning the raw value sorts lexically: `"100"` before `"99"`.
- `if (!seconds) return '—'` never fires for zero, because `"0"` is truthy.

The permanent fix is a postgres.js `types` override in `server/db/pg.ts` (audit task 21), after which
`PgNumeric` collapses back to `number` and the coercions become no-ops.

## Bodies — `parseBody`

```ts
const body = await parseBody(request, Schema)   // zod
if (body instanceof Response) return body
```

Invalid input 400s with the FIRST zod issue as `error` — never a bare "bad request". Schemas cap
string lengths and array sizes.

## Shapes

- Errors: `json({ error: string }, { status })`. Machine-friendly strings for switchable cases
  (`unauthorized`, `forbidden`, `not found`); human sentences where a person reads them.
- Reads: a single named wrapper — `{ agents }`, `{ doc }`, `{ members }`. Never a bare array.
- Mutations: `{ ok: true }`, or the created/updated object (`{ folder }`).
- Never leak `(e as Error).message` with a 500: `console.error('[route]', e)` server-side, return a
  generic line. 400-level messages may carry the business reason.
- Secrets: sealed at rest, **never echoed** — GETs return set-flags (`passSet`) or masked keys.
  An empty string on a round-tripped masked field means "keep", not "clear".

## Methods

- **GET** reads. **POST** creates and *actions* (verify, test-send, install, rotate).
- **PUT/PATCH** config and edits — config writes are PUT, not POST.
- **DELETE** takes ids in the path, small selectors in a JSON body (matching the PUT transport).
- Authorization comes BEFORE body parsing unless the decision depends on the body (say so in a
  comment).

## Audit

Governance-relevant mutations call `logAudit` with `actor: actorOf(user)` (or `ActingUser.label`
for assistant-proxied writes): lifecycle, access grants, config, credentials (names only — never
values), creates of durable org content. Chatter (messages, reactions, presence) does not audit.

## App servers

Third-party app routes get the same treatment for free: the host gateway authenticates and
view-checks before dispatch ([docs/sdk](./sdk/server.md)); inside handlers, follow the shapes above.
