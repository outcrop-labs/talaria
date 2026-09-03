# API conventions — one dialect for every route

The contract that keeps every `/api/*` route predictable, on both sides of the origin. The runtime of
record is the Rust crate (`api/`): it serves every route except the four permanent TS residents
(`healthz`, `/api/admin/update`, the `/api/apps/` dispatch subtree, the app-MCP gateway), and a new
route lands as a Rust file under `api/src/routes/<group>/`, in this dialect, with its tests (the
Rust-side authoring rules and the recorded divergences are
[`RUST-MIGRATION.md`](./RUST-MIGRATION.md)).

The TS route dialect below still governs everything that writes a route the TS way — the four
residents, and app servers through the host gateway. A resident route lives in `ui/src/routes/api/`;
its path is the string in its `defineApi('…')` call, the filename is convention that mirrors it, and
its server logic lives in `ui/src/server/`. Every module under `routes/api/` that exports `Route`
goes live — the glob has no test exclusion and nothing under `routes/` can be unit tested, so a
route parses the request, calls ONE function in `src/server/*`, and serializes the result. The api
keeps the same division: its route files are thin, and the decisions live one dir deeper, where the
unit tests reach them.

The full per-route reference — path, method, auth class, body fields, statuses — is
[`api/`](./api/README.md), generated from the Rust routes (`bun run docs:api`; drift fails
`bun run check`). Nothing here repeats it; this page is the dialect, that one is the dictionary.

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

Every agent patch that goes through `update_task` inherits the HITL invariant automatically:
`agent_safe_patch` strips assignment/planning/archival, redirects terminal moves to
the board's review column, and refuses the rest. The predicate lives **once**, in
`api/src/tasks.rs` — the TS routes that carried a twin spelling were deleted at the cutover, the
resident tier never writes tickets, and the MCP server's tools ride the api — so there is no second
definition anywhere to keep in step. **A route that goes through `update_task` needs nothing else.**

A route that writes something *attached* to a ticket without going through `update_task` — a usage
row, a dependency edge, a gap report, a workbench plan comment or PR title — does not inherit it, and
must ask the one exported predicate:

```rust
use crate::tasks::{agent_ticket_refusal, AgentIntent, AgentWriteTarget};

let target = AgentWriteTarget::from(&task);   // board, status, archived_at — all it asks
match agent_ticket_refusal(&state.pg, &target, &agent, AgentIntent::Write).await {
    Ok(None) => { /* it may */ }
    Ok(Some(shut)) => return Err(house_error(StatusCode::FORBIDDEN, &shut)),
    Err(e) => {
        tracing::error!("[route] agent authority failed: {e}");
        return Err(thrown_internal_error());
    }
}
```

It returns the **reason** rather than a boolean so the refusal reads the same wherever the write
arrived — the reason *is* the 403 body — and it asks **four** stop conditions, in the order a person
would: the board does not allow this agent (revoked, never granted, or the board is archived/gone),
the ticket is archived, the intent is `Comment` — not a stop; the exemption — and, for `Write` only,
the ticket is closed (a done column on that board, or the off-board `failed` / `cancelled`).

`AgentIntent::Comment` skips the closed-status clause and nothing else — commenting stays open on
work an agent can no longer edit, which is deliberate — but archival stops it at **both** levels,
because archival withdraws the work rather than closing it, and a channel nobody is watching is not a
channel. Reads are not this question: an agent that passes `board_allows_agent` may read the ticket,
because reading changes nothing.

The same predicate answers the *pull* side too — `assigned_work` asks it before serving an agent
its queue — so the heartbeat and the write routes agree by construction rather than by two people
remembering.

> **Do not copy it into your route.** Four TS routes once hand-rolled their own copies, because
> parallel work split file ownership and nothing exported one. All four checked only the
> closed-status third and silently missed both archival clauses the original grew later — so an agent
> could keep writing to a ticket a person had archived through any of them. Duplicating an invariant
> is how the sixth laundering path in a series of six got created; a new copy is never the fix.
> Comments are deliberately outside this gate: commenting stays open on work an agent can no longer
> edit.

### CI fails these — `scripts/check-invariants.mjs`

The conventions above were all true and documented before someone re-derived them by hand anyway, so
they are no longer only conventions. `node scripts/check-invariants.mjs` runs as the first job on
every PR (`.github/workflows/ci.yml`, no install step, seconds) and **fails the build** on:

| pattern | what to write instead |
| --- | --- |
| a `queryFn` that resolves `[]` / `null` / `{}` when the request failed (`if (!r.ok) return []`, `r.ok ? … : []`, `.catch(() => [])`) | let it throw — a resolved-on-failure query reports SUCCESS carrying emptiness, and the UI renders "nothing here" for "we could not ask" |
| a query result flattened to a default on the line that created it (`const { data: x = [] } = useThing()`) | `listQuery()` from `@/components/ui/query-state` (or `<QueryState>`) — rows AND the sentence that says they are missing; flattening makes `isError` unreachable from that component forever |
| a `listQuery()` whose `notice` is taken and never rendered (or never taken) | render it — the inline variant for quiet spots; silence is the failure mode this rule exists for |
| a portal/fixed panel carrying its own document-level outside listener | the shells: `<Popover>`, `<DropdownMenu>`, `<ContextMenu>` — they own outside-click, Escape, scroll, and stacking; the allowlist is three shells, one documented exception (`DocLinkPopover`), one input primitive (`Combobox`) |
| `'failed'` next to `'cancelled'` spelled out as a literal or type union | `import { OFF_BOARD_STATUSES } from '@/lib/task-const'` — the census holds the list to its one TS definition |
| `allow_all_agents` in SQL | the board agent policy is the Rust boards engine's (`api/src/boards.rs`); the one counted site is the migration DDL |
| a hand-rolled client `fetch(`, or a `credentials: 'same-origin'` stanza outside the door | a verb from `@/lib/fetch-json` (`getJson` / `getList` / `postJson` / `postStream` …) — one HTTP door, and doors drift |

Migration SQL and a few structural checks (server values imported into browser modules, upload
bytes served outside `serveUpload`) get their own passes in the same script. Every failure names the
file, the line and the fix.

Two mechanisms back the table. **Rules** are absolute: legal only in the file that defines the thing.
**Censuses** are exact counts for patterns that are legitimate in a named few places — a file that
gains an occurrence fails, *and a file that loses its last one also fails*, so the list shrinks as
debt is paid and can never settle into a standing amnesty. If a match is genuinely wrong, argue it in
the PR; widening the pattern to go green is how the previous rounds happened.

**Adding a rule is cheap and expected.** When a review finds an invariant that was re-derived by
hand, centralize it *and* add the pattern — the structural fix alone rests entirely on the next
reviewer noticing, which is exactly the assumption that keeps failing.

### The one off-board status list

`OFF_BOARD_STATUSES` (`failed`, `cancelled`) is declared in **`ui/src/lib/task-const.ts`** — the
client's wire vocabulary. The workflow-column engine that owns the set server-side is the Rust
statuses engine (`api/src/statuses.rs`), which declares its own; a TS copy must import, not
re-spell, and the `off-board-status-literal` census holds it to that one definition. (`server/statuses.ts`
carried a second copy until the cutover deleted the file, and the cross-check that held the two
identical retired with it.) `TaskStatus` derives from the list, so the type union cannot drift
from it.

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

```svelte
const statusesQuery = useBoardStatuses(() => board.id)
const statusesFailed = $derived(statusesQuery.isError && statusesQuery.data === undefined)
```

…then a `<QueryError>` region in the template when `statusesFailed`. The `data === undefined` half
is what makes it a refusal and not a regression: a *cached* set that failed to refresh is stale, not
absent, and stale columns beat no board at all — that case gets the inline banner instead.
`Kanban.svelte` and `BoardList.svelte` do exactly this ("same test, same word, same consequence,"
as the list's own comment puts it). `Gantt.svelte` deliberately does not replace the chart — the
tickets loaded, so the timeline stays and the failed workflow read is an inline `<QueryError>`
banner whose title says bar colours and overdue marks are guesses.

### Wire numerics — `numeric` and `bigint` arrive as strings

postgres.js hands `numeric` and `int8` back as **strings**, and there is no `types` override on the
pool, so `estimated_hours` and `time_spent_seconds` are strings by the time they reach a component
that has them typed `number`. Declare them `PgNumeric` (`@/lib/task-const`) and read them through
`pgNum` / `pgNumOr` / `taskTimeSpent`.

This is not pedantry, and it is mostly invisible to `tsc` — implicit coercion means the arithmetic
compiles and silently misbehaves:

- A number off the wire (an edit form's estimate) compared against a string from the row reads as
  different every time, so re-saving an unchanged estimate looks like a change.
- `sum + t.estimatedHours` concatenates instead of adding — `"04.5"`.
- A comparator returning the raw value sorts lexically: `"100"` before `"99"`.
- `if (!seconds) return '—'` never fires for zero, because `"0"` is truthy.

The permanent fix is a postgres.js `types` override in `server/db/pg.ts`, after which
`PgNumeric` collapses back to `number` and the coercions become no-ops.

## Bodies — `parseBody`

```ts
const body = await parseBody(request, Schema)   // zod
if (body instanceof Response) return body
```

Invalid input 400s with the FIRST zod issue as `error` — never a bare "bad request". Schemas cap
string lengths and array sizes. The api's `body.rs` answers with the same first-issue sentences —
they are wire contract, pinned by its tests.

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
