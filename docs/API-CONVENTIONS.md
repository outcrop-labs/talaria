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

Non-session auth: agent routes use `checkAgentKey` + `agentName` (fleet key); public routes say so
in a comment; `llm.v1.*` follows the OpenAI wire contract and is exempt from all of this.

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
