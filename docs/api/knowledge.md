# API reference — knowledge

> **Frozen at the cutover (2026-09-01)** — generated from the TS route tree the Rust port
> replaced. Source links point at the Rust modules (`api/src/routes/**`; each module’s
> header names the TS file it ported) or the permanent TS residents still serving.
> Regeneration returns with the Rust extractor (#293); until then, maintained by hand.
> The **Returns** column is the first success-shaped `json({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.

19 routes.

| Route | Method | Auth |
| :--- | :--- | :--- |
| [`/api/kb/comments/{id}`](#apikbcommentsid) | PATCH | `session` |
| [`/api/kb/comments/{id}`](#apikbcommentsid) | DELETE | `session` |
| [`/api/kb/docs/{id}`](#apikbdocsid) | GET | `dual` |
| [`/api/kb/docs/{id}`](#apikbdocsid) | PUT | `dual` |
| [`/api/kb/docs/{id}`](#apikbdocsid) | DELETE | `session` |
| [`/api/kb/docs/{id}/backlinks`](#apikbdocsidbacklinks) | GET | `session` |
| [`/api/kb/docs/{id}/comments`](#apikbdocsidcomments) | GET | `session` |
| [`/api/kb/docs/{id}/comments`](#apikbdocsidcomments) | POST | `session` |
| [`/api/kb/docs/{id}/live`](#apikbdocsidlive) | PUT | `session` |
| [`/api/kb/docs/{id}/live`](#apikbdocsidlive) | GET | `session` |
| [`/api/kb/docs/{id}/move`](#apikbdocsidmove) | POST | `session` + `perm:kb.edit` |
| [`/api/kb/public/{slug}`](#apikbpublicslug) | GET | `public` |
| [`/api/kb/public/space/{slug}`](#apikbpublicspaceslug) | GET | `public` |
| [`/api/kb/search`](#apikbsearch) | GET | `session` |
| [`/api/kb/spaces`](#apikbspaces) | GET | `dual` |
| [`/api/kb/spaces`](#apikbspaces) | POST | `dual` |
| [`/api/kb/spaces/{id}`](#apikbspacesid) | GET | `session` |
| [`/api/kb/spaces/{id}`](#apikbspacesid) | PUT | `session` |
| [`/api/kb/spaces/{id}`](#apikbspacesid) | DELETE | `session` |
| [`/api/kb/spaces/{id}/docs`](#apikbspacesiddocs) | GET | `dual` |
| [`/api/kb/spaces/{id}/docs`](#apikbspacesiddocs) | POST | `dual` |
| [`/api/memory/{id}`](#apimemoryid) | GET | `session` |
| [`/api/memory/{id}`](#apimemoryid) | PUT | `session` |
| [`/api/rag/collections`](#apiragcollections) | GET | `session` |
| [`/api/rag/collections`](#apiragcollections) | POST | `admin` |
| [`/api/rag/collections/{id}`](#apiragcollectionsid) | PUT | `admin` |
| [`/api/rag/collections/{id}`](#apiragcollectionsid) | DELETE | `admin` |
| [`/api/rag/search`](#apiragsearch) | POST | `dual` |
| [`/api/search`](#apisearch) | POST | `dual` |
| [`/api/templates`](#apitemplates) | GET | `session` |
| [`/api/templates`](#apitemplates) | POST | `session` + `perm:templates.manage` |
| [`/api/templates/{id}`](#apitemplatesid) | PUT | `session` + `perm:templates.manage` |
| [`/api/templates/{id}`](#apitemplatesid) | DELETE | `session` + `perm:templates.manage` |

## `/api/kb/comments/{id}`

Source: [`api/src/routes/kb_comments_id.rs`](../../api/src/routes/kb_comments_id.rs)

> One comment. PATCH { resolved } → resolve/unresolve its thread (author,
> thread starter, or doc owner). DELETE → remove your own comment.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| PATCH | `session` | [body](#patch-apikbcommentsid-body) | `{ok}` | 200, 403 | — |
| DELETE | `session` | — | `{ok}` | 200, 403 | — |

### PATCH `/api/kb/comments/{id}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `resolved` | `z.boolean()` |  |

## `/api/kb/docs/{id}`

Source: [`api/src/routes/kb_docs_id.rs`](../../api/src/routes/kb_docs_id.rs)

> One KB doc. Read/edit are gated by the doc's EFFECTIVE audience — inherited
> from its folder unless the doc has been customized. Sharing changes are
> owner-only; agents (by key) only edit content when granted the Editor role.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | — | `{doc, editors}` | 200, 403, 404 | — |
| PUT | `dual` | [body](#put-apikbdocsid-body) | `{doc, editors}` | 200, 400, 403, 404 | audit |
| DELETE | `session` | — | `{ok}` | 200, 403, 404 | — |

### PUT `/api/kb/docs/{id}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `title` | `z.string().max(200).optional()` |  |
| `body` | `z.string().max(500_000).optional()` |  |
| `icon` | `z.string().max(16).nullish()` |  |
| `visibility` | `z.enum(['private', 'org', 'public']).optional()` |  |
| `editPolicy` | `z.enum(['owner', 'org', 'restricted']).optional()` |  |
| `editors` | `z.array(Editor).max(200).optional()` |  |
| `permsInherited` | `z.boolean().optional()` |  |
| `parentId` | `Uuid.nullish()` |  |
| `official` | `z.boolean().optional()` |  |
| `regenerateOkf` | `z.boolean().optional()` |  |
| `ragRouting` | `z.string().max(60).optional()` |  |

## `/api/kb/docs/{id}/backlinks`

Source: [`api/src/routes/kb_docs_id_backlinks.rs`](../../api/src/routes/kb_docs_id_backlinks.rs)

> Docs that link to this one ("linked from"). Editor links point at
> /knowledge/<id>, so backlinks fall out of a substring match. Gated by the
> SAME per-doc ACL as reading the doc — backlink titles leak content.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{backlinks}` | 200, 403, 404 | — |

## `/api/kb/docs/{id}/comments`

Source: [`api/src/routes/kb_docs_id_comments.rs`](../../api/src/routes/kb_docs_id_comments.rs)

> Doc comment threads. GET → all comments (client assembles threads).
> POST { content, parentId?, quote? } → comment/reply. Read access to the doc
> is the gate for both — discussion is part of the document.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{comments}` | 200, 404 | — |
| POST | `session` | [body](#post-apikbdocsidcomments-body) | `…` | 200, 404 | — |

### POST `/api/kb/docs/{id}/comments` body

| field | schema | notes |
| :--- | :--- | :--- |
| `content` | `z.string().trim().min(1).max(8_000)` |  |
| `parentId` | `Uuid.nullish()` |  |
| `quote` | `z.string().trim().max(500).nullish()` |  |

## `/api/kb/docs/{id}/live`

Source: [`api/src/routes/kb_docs_id_live.rs`](../../api/src/routes/kb_docs_id_live.rs)

> Doc presence (the multiplayer layer's heartbeat). PUT { mode } → I'm here,
> viewing or editing. GET → who's here right now, with their mode — the doc
> header renders the avatar stack and the concurrent-edit warning from this.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| PUT | `session` | [body](#put-apikbdocsidlive-body) | `{ok}` | 200, 404 | — |
| GET | `session` | — | `{active}` | 200, 404 | — |

### PUT `/api/kb/docs/{id}/live` body

| field | schema | notes |
| :--- | :--- | :--- |
| `mode` | `z.enum(['view', 'edit'])` |  |

## `/api/kb/docs/{id}/move`

Source: [`api/src/routes/kb_docs_id_move.rs`](../../api/src/routes/kb_docs_id_move.rs)

> Reparent / reorder a doc in the sidebar tree. Rejects cycles server-side.
> Moving a doc is an edit of it, so it takes the same gate the PUT does —
> otherwise any signed-in member could reparent a private doc out of a folder
> they can't even read. `moveDoc` itself only detects cycles and always has.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` + `perm:kb.edit` | [body](#post-apikbdocsidmove-body) | `…` | 200, 400, 403, 404 | audit |

### POST `/api/kb/docs/{id}/move` body

| field | schema | notes |
| :--- | :--- | :--- |
| `parentId` | `Uuid.nullable()` |  |
| `sort` | `z.number().int().default(0)` |  |

## `/api/kb/public/{slug}`

Source: [`api/src/routes/kb_public.rs`](../../api/src/routes/kb_public.rs)

> Public doc read — no auth. Only docs with visibility 'public' resolve.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `public` | — | `{doc}` | 200, 404 | — |

## `/api/kb/public/space/{slug}`

Source: [`api/src/routes/kb_public_space.rs`](../../api/src/routes/kb_public_space.rs)

> Public folder read — no auth. Only spaces with visibility 'public' resolve;
> returns the folder's name + overview (its body), like a public doc.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `public` | — | `{space}` | 200, 404 | — |

## `/api/kb/search`

Source: [`api/src/routes/kb_search.rs`](../../api/src/routes/kb_search.rs)

> Full-text search across the knowledgebase (docs the caller can read).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{hits}` | 200 | — |

## `/api/kb/spaces`

Source: [`api/src/routes/kb_spaces.rs`](../../api/src/routes/kb_spaces.rs)

> KB spaces (any member). GET → all. POST → create.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | — | `…` | 200 | — |
| POST | `dual` | [body](#post-apikbspaces-body) | `{space}` | 200 | audit |

### POST `/api/kb/spaces` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `z.string().min(1).max(80)` |  |
| `description` | `z.string().max(400).optional()` |  |
| `icon` | `z.string().max(8).optional()` |  |

## `/api/kb/spaces/{id}`

Source: [`api/src/routes/kb_spaces_id.rs`](../../api/src/routes/kb_spaces_id.rs)

> One KB folder (space). Same permission model as docs: read gated by
> visibility, writes by the edit policy + editor grants, sharing owner-only.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200, 403, 404 | — |
| PUT | `session` | [body](#put-apikbspacesid-body) | `{space, editors}` | 200, 403, 404 | — |
| DELETE | `session` | — | `{ok}` | 200, 403, 404 | audit |

### PUT `/api/kb/spaces/{id}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `z.string().min(1).max(80).optional()` |  |
| `description` | `z.string().max(400).nullish()` |  |
| `icon` | `z.string().max(16).nullish()` |  |
| `body` | `z.string().max(500_000).optional()` |  |
| `visibility` | `z.enum(['private', 'org', 'public']).optional()` |  |
| `editPolicy` | `z.enum(['owner', 'org', 'restricted']).optional()` |  |
| `editors` | `z.array(Editor).max(200).optional()` |  |

## `/api/kb/spaces/{id}/docs`

Source: [`api/src/routes/kb_spaces_id_docs.rs`](../../api/src/routes/kb_spaces_id_docs.rs)

> A space's docs (tree). GET → doc metadata list. POST → new doc.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | — | `{docs}` | 200 | — |
| POST | `dual` | [body](#post-apikbspacesiddocs-body) | `{doc}` | 200, 403, 404 | audit |

### POST `/api/kb/spaces/{id}/docs` body

| field | schema | notes |
| :--- | :--- | :--- |
| `title` | `z.string().max(200).optional()` |  |
| `parentId` | `Uuid.nullish()` |  |
| `kind` | `z.enum(['human', 'agent']).optional()` |  |
| `body` | `z.string().max(500_000).optional()` |  |

## `/api/memory/{id}`

Source: [`api/src/routes/memory_id.rs`](../../api/src/routes/memory_id.rs)

> One managed agent's MEMORY.md, read/written through its running container.
> Writes: admin, or the owner of a personal assistant for its own memory.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `…` | 200, 400, 403 | — |
| PUT | `session` | [body](#put-apimemoryid-body) | `{ok}` | 200, 400, 403 | — |

### PUT `/api/memory/{id}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `content` | `z.string().max(2_000_000)` |  |

## `/api/rag/collections`

Source: [`api/src/routes/rag_collections.rs`](../../api/src/routes/rag_collections.rs)

> The RAG collection registry (admin). GET → all collections + bindings (the two
> auto ones are ensured first). POST → spin up a new custom collection.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{collections}` | 200 | — |
| POST | `admin` | [body](#post-apiragcollections-body) | `{collection}` | 200, 400 | audit |

### POST `/api/rag/collections` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `z.string().min(2).max(80)` |  |
| `description` | `z.string().max(500).optional()` |  |
| `bindings` | `z.array(Binding).max(200).optional()` |  |

## `/api/rag/collections/{id}`

Source: [`api/src/routes/rag_collections_id.rs`](../../api/src/routes/rag_collections_id.rs)

> One collection (admin). PUT → set its access bindings. DELETE → drop it (auto
> collections are protected).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| PUT | `admin` | [body](#put-apiragcollectionsid-body) | `{ok}` | 200, 404 | audit |
| DELETE | `admin` | — | `{ok}` | 200, 400 | audit |

### PUT `/api/rag/collections/{id}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `bindings` | `z.array(z.object({ principalType: z.enum(['all', 'user', 'agent', 'team']), principalId: z.string().max(200).nullish() })).max(200)` |  |

## `/api/rag/search`

Source: [`api/src/routes/rag_search.rs`](../../api/src/routes/rag_search.rs)

> Ranked retrieval across the caller's accessible collections. Works for a
> signed-in user (their bindings) OR a fleet agent (agent-key + x-agent-name →
> that agent's bindings). This is the endpoint the search_knowledge MCP tool
> calls — retrieval as function-calling.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `dual` | [body](#post-apiragsearch-body) | `…` | 200, 502 | — |

### POST `/api/rag/search` body

| field | schema | notes |
| :--- | :--- | :--- |
| `query` | `z.string().min(1).max(2000)` |  |
| `limit` | `z.number().int().min(1).max(20).optional()` |  |
| `collectionIds` | `z.array(Uuid).max(20).optional()` |  |

## `/api/search`

Source: [`api/src/routes/search.rs`](../../api/src/routes/search.rs)

> LIVE WEB SEARCH — the endpoint behind the `web_search` MCP tool, and the one
> place an agent or a signed-in user reaches this deployment's search.
>
> IT ASKS `searchTheWeb`, NOT SEARXNG. This used to call the SearXNG client
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `dual` | [body](#post-apisearch-body) | `{query, results, via}` | 200, 503 | — |

### POST `/api/search` body

| field | schema | notes |
| :--- | :--- | :--- |
| `query` | `z.string().min(2).max(400)` |  |
| `limit` | `z.number().int().min(1).max(25).optional()` |  |

## `/api/templates`

Source: [`api/src/routes/templates.rs`](../../api/src/routes/templates.rs)

> The org's template library (ticket + plan formats). GET → all (any member —
> the library grounds pickers everywhere). POST → create (any member, like
> boards/channels; the skeletons are org-shared working material, not policy).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{templates}` | 200 | — |
| POST | `session` + `perm:templates.manage` | [body](#post-apitemplates-body) | `…` | 200 | — |

### POST `/api/templates` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `z.string().trim().min(1).max(120)` |  |
| `kind` | `z.enum(['ticket', 'plan'])` |  |
| `body` | `z.string().max(50_000).optional()` |  |
| `guidance` | `z.string().max(10_000).optional()` |  |

## `/api/templates/{id}`

Source: [`api/src/routes/templates_id.rs`](../../api/src/routes/templates_id.rs)

> One template: PUT → edit (kind is immutable — retire and recreate instead),
> DELETE → remove (bindings cascade/null out; consumers fall through the chain).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| PUT | `session` + `perm:templates.manage` | [body](#put-apitemplatesid-body) | `…` | 200, 404 | — |
| DELETE | `session` + `perm:templates.manage` | — | `{ok}` | 200, 404 | — |

### PUT `/api/templates/{id}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `z.string().trim().min(1).max(120).optional()` |  |
| `body` | `z.string().max(50_000).optional()` |  |
| `guidance` | `z.string().max(10_000).optional()` |  |

