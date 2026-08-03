# API conventions — one dialect for every route

The contract that keeps 160+ routes predictable. Swept across the whole tree (2026-07); new routes
follow it from day one. Routes live in `ui/src/routes/api/` (TanStack Start file routes); server
logic in `ui/src/server/`.

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
during the migration window — identity asserted, not proven, so anything granting privilege must
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
import { closedToAgents } from '@/server/tasks'

const shut = await closedToAgents(task)          // reason string, or null when the agent may write
if (isAgent && shut) return json({ error: shut }, { status: 403 })
```

It returns the **reason** rather than a boolean so the refusal reads the same wherever the write
arrived, and it covers three conditions, not one: archived ticket, archived **board**, closed status
(a `done` column on that board, or the off-board `failed` / `cancelled`). Pass an already-resolved
`statusMeta` as the second argument to save the round trip. The same predicate answers the *pull*
side too — `maybeDispatchTicket` and `assignedWork` ask it before handing an agent work — so the
queue and the write routes agree by construction rather than by two people remembering.

> **Do not copy it into your route.** Four routes each hand-rolled their own copy, because parallel
> work split file ownership and `server/tasks.ts` did not export one. All four checked only the
> closed-status third and silently missed both archival clauses the original grew later — so an agent
> could keep writing to a ticket a person had archived through any of them. Duplicating an invariant
> is how the sixth laundering path in a series of six got created; a fifth copy is never the fix.
> Comments are deliberately outside this gate: commenting stays open on work an agent can no longer
> edit.

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
view-checks before dispatch ([SDK.md](./SDK.md)); inside handlers, follow the shapes above.
