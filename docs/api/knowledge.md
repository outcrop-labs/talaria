# API reference — knowledge

> **Generated** by `bun run docs:api` from the Rust router table (`api/src/routes/mod.rs`)
> and the handler modules under `api/src/routes/**` (the TS residents still serving
> `healthz`, `admin/update` and the app dispatch excepted) — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
> The **Returns** column is the first success-shaped `json!({…})` literal and is heuristic —
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
| [`/api/kb/docs/{id}/live`](#apikbdocsidlive) | GET | `session` |
| [`/api/kb/docs/{id}/live`](#apikbdocsidlive) | PUT | `session` |
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

Source: [`api/src/routes/knowledge/kb_comments_id.rs`](../../api/src/routes/knowledge/kb_comments_id.rs)

> /api/kb/comments/{id}. One comment. PATCH { resolved } → resolve/unresolve
> its thread (author, thread starter, or doc owner). DELETE → remove your own
> comment.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| PATCH | `session` | [body](#patch-apikbcommentsid-body) | `{ok}` | 200, 400, 403 | — |
| DELETE | `session` | — | `{ok}` | 200, 403 | — |

### PATCH `/api/kb/comments/{id}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `resolved` | `bool` |  |

## `/api/kb/docs/{id}`

Source: [`api/src/routes/knowledge/kb_docs_id.rs`](../../api/src/routes/knowledge/kb_docs_id.rs)

> /api/kb/docs/{id}. One KB doc. Read/edit gated by the doc's EFFECTIVE
> audience — inherited from its folder unless customized. Sharing changes are
> owner-only; routing owner-only;
> officializing needs kb.official. Agents (by key) only edit content when
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | — | `{doc, editors}` | 200, 403, 404 | — |
| PUT | `dual` | [body](#put-apikbdocsid-body) | `{doc, editors}` | 200, 400, 403, 404 | audit |
| DELETE | `session` | — | `{ok}` | 200, 403, 404 | — |

### PUT `/api/kb/docs/{id}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `title` | `string?(200)` |  |
| `body` | `string?(500000)` |  |
| `icon` | `string? nullable(16)` |  |
| `visibility` | `enum(private|org|public)?` |  |
| `editPolicy` | `enum(owner|org|restricted)?` |  |
| `permsInherited` | `bool?` |  |
| `parentId` | `uuid? nullable` |  |
| `official` | `bool?` |  |
| `regenerateOkf` | `bool?` |  |
| `ragRouting` | `string?(60)` |  |

## `/api/kb/docs/{id}/backlinks`

Source: [`api/src/routes/knowledge/kb_docs_id_backlinks.rs`](../../api/src/routes/knowledge/kb_docs_id_backlinks.rs)

> /api/kb/docs/{id}/backlinks. Docs that link to this one ("linked from").
> Editor links point at /knowledge/<id>, so backlinks fall out of a substring
> match. Gated by the SAME per-doc ACL as reading the doc — backlink titles
> leak content.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{backlinks}` | 200, 403, 404 | — |

## `/api/kb/docs/{id}/comments`

Source: [`api/src/routes/knowledge/kb_docs_id_comments.rs`](../../api/src/routes/knowledge/kb_docs_id_comments.rs)

> /api/kb/docs/{id}/comments. Doc comment threads. GET → all comments (client
> assembles threads). POST { content, parentId?, quote? } → comment/reply.
> Read access to the doc is the gate for both — discussion is part of the
> document. 404-as-ACL: a doc you can't discuss doesn't exist as far as this
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{comments}` | 200, 404 | — |
| POST | `session` | [body](#post-apikbdocsidcomments-body) | `{comment}` | 200, 400, 404 | — |

### POST `/api/kb/docs/{id}/comments` body

| field | schema | notes |
| :--- | :--- | :--- |
| `content` | `string trimmed(1, 8000)` | content/quote are trim-then-validate members — the length bounds apply to the TRIMMED value, which is also what gets stored. |
| `parentId` | `uuid?` |  |
| `quote` | `string trimmed(0, 500)` |  |

## `/api/kb/docs/{id}/live`

Source: [`api/src/routes/knowledge/kb_docs_id_live.rs`](../../api/src/routes/knowledge/kb_docs_id_live.rs)

> /api/kb/docs/{id}/live. Doc presence (the multiplayer layer's heartbeat).
> PUT { mode } → I'm here, viewing or editing. GET → who's here right now,
> with their mode — the doc header renders the avatar stack and the
> concurrent-edit warning from this. Redis keys kb:presence:<docId>:<userId>
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{active}` | 200, 404 | — |
| PUT | `session` | [body](#put-apikbdocsidlive-body) | `{ok}` | 200, 400, 404 | — |

### PUT `/api/kb/docs/{id}/live` body

| field | schema | notes |
| :--- | :--- | :--- |
| `mode` | `enum(view|edit)` |  |

## `/api/kb/docs/{id}/move`

Source: [`api/src/routes/knowledge/kb_docs_id_move.rs`](../../api/src/routes/knowledge/kb_docs_id_move.rs)

> /api/kb/docs/{id}/move. Reparent / reorder a doc in the sidebar tree.
> Rejects cycles server-side. Moving a doc is an edit of it, so it takes the
> same gate the PUT does — otherwise any signed-in member could reparent a
> private doc out of a folder they can't even read. `moveDoc` itself only
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `session` + `perm:kb.edit` | [body](#post-apikbdocsidmove-body) | `{doc}` | 200, 400, 403, 404 | audit |

### POST `/api/kb/docs/{id}/move` body

| field | schema | notes |
| :--- | :--- | :--- |
| `parentId` | `uuid? nullable` |  |
| `sort` | `number()` |  |

## `/api/kb/public/{slug}`

Source: [`api/src/routes/knowledge/kb_public.rs`](../../api/src/routes/knowledge/kb_public.rs)

> /api/kb/public/{slug}. Public doc read — no auth. Only docs with visibility
> 'public' resolve; the response body is title/body/updatedAt only (routing
> and every other internal column stay off the public wire).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `public` | — | `{doc}` | 200, 404 | — |

## `/api/kb/public/space/{slug}`

Source: [`api/src/routes/knowledge/kb_public_space.rs`](../../api/src/routes/knowledge/kb_public_space.rs)

> /api/kb/public/space/{slug}. Public folder read — no auth. Only spaces with
> visibility 'public' resolve; returns the folder's name + overview (its
> body), like a public doc.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `public` | — | `{space}` | 200, 404 | — |

## `/api/kb/search`

Source: [`api/src/routes/knowledge/kb_search.rs`](../../api/src/routes/knowledge/kb_search.rs)

> /api/kb/search. Full-text search across the knowledgebase (docs the caller
> can read). The engine (ranked union of docs + space overviews,
> effective-visibility ACL filter, sentinel highlighting) lives in
> kb::search_docs.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{hits}` | 200 | — |

## `/api/kb/spaces`

Source: [`api/src/routes/knowledge/kb_spaces.rs`](../../api/src/routes/knowledge/kb_spaces.rs)

> /api/kb/spaces. KB spaces (any member). GET → all the caller can read
> (agents over MCP see org/public + granted; humans see visibility-read +
> granted). POST → create (agents find-or-create by name; humans need
> kb.official).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | — | `{spaces}` | 200 | — |
| POST | `dual` | [body](#post-apikbspaces-body) | `{space}` | 200, 400 | audit |

### POST `/api/kb/spaces` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `string(1, 80)` |  |
| `description` | `string?(400)` |  |
| `icon` | `string?(8)` |  |

## `/api/kb/spaces/{id}`

Source: [`api/src/routes/knowledge/kb_spaces_id.rs`](../../api/src/routes/knowledge/kb_spaces_id.rs)

> /api/kb/spaces/{id}. One KB folder. Same permission model as docs: read
> gated by visibility, writes by the edit policy + editor grants, sharing
> owner-only (can_govern).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{space, editors}` | 200, 403, 404 | — |
| PUT | `session` | [body](#put-apikbspacesid-body) | `{space, editors}` | 200, 400, 403, 404 | — |
| DELETE | `session` | — | `{ok}` | 200, 403, 404 | audit |

### PUT `/api/kb/spaces/{id}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `string(1, 80)` |  |
| `description` | `string? nullable(400)` |  |
| `icon` | `string? nullable(16)` |  |
| `body` | `string?(500000)` |  |
| `visibility` | `enum(private|org|public)?` |  |
| `editPolicy` | `enum(owner|org|restricted)?` |  |

## `/api/kb/spaces/{id}/docs`

Source: [`api/src/routes/knowledge/kb_spaces_id_docs.rs`](../../api/src/routes/knowledge/kb_spaces_id_docs.rs)

> /api/kb/spaces/{id}/docs. A space's doc tree. GET → doc metadata list
> (agents gate on agent space-access, then per-doc audience; humans gate on
> the folder, then inherited docs show and customized ones filter). POST →
> new doc (agent docs are drafts owned by the assistant's principal; humans
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `dual` | — | `{docs}` | 200 | — |
| POST | `dual` | [body](#post-apikbspacesiddocs-body) | `{doc}` | 200, 400, 403, 404 | audit |

### POST `/api/kb/spaces/{id}/docs` body

| field | schema | notes |
| :--- | :--- | :--- |
| `title` | `string?(200)` |  |
| `parentId` | `uuid?` |  |
| `kind` | `enum(human|agent)?` |  |
| `body` | `string?(500000)` | Initial markdown body (the MCP create_kb_doc path sets it in one shot). |

## `/api/memory/{id}`

Source: [`api/src/routes/knowledge/memory_id.rs`](../../api/src/routes/knowledge/memory_id.rs)

> /api/memory/{id}. One managed agent's MEMORY.md, read/written through its
> running container. Writes: admin, or the owner of a personal assistant for
> its own memory.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{content, container}` | 200, 400, 403 | — |
| PUT | `session` | [body](#put-apimemoryid-body) | `{ok}` | 200, 400, 403 | — |

### PUT `/api/memory/{id}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `content` | `string(0, 2000)` | content — required, max 2M; the empty string is legal (min 0: clearing a memory is a write). |

## `/api/rag/collections`

Source: [`api/src/routes/knowledge/rag_collections.rs`](../../api/src/routes/knowledge/rag_collections.rs)

> /api/rag/collections. The RAG collection registry. GET → every collection +
> its access bindings (the two auto ones ensured first; members get the
> picker shape with the binding matrix blanked — that matrix is admin
> governance). POST → spin up a custom collection. The write is admin-only;
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{collections}` | 200 | — |
| POST | `admin` | [body](#post-apiragcollections-body) | `{collection}` | 200, 400 | audit |

### POST `/api/rag/collections` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `string(2, 80)` |  |
| `description` | `string?(500)` |  |

## `/api/rag/collections/{id}`

Source: [`api/src/routes/knowledge/rag_collections_id.rs`](../../api/src/routes/knowledge/rag_collections_id.rs)

> /api/rag/collections/{id}. One collection, admin. PUT → replace its access
> bindings wholesale; an unknown (but well-formed) id 404s. DELETE → drop it
> (the two auto collections are protected); a missing id is a no-op delete —
> it still answers ok, it still audits.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| PUT | `admin` | [body](#put-apiragcollectionsid-body) | `{ok}` | 200, 400, 404 | audit |
| DELETE | `admin` | — | `{ok}` | 200, 400 | audit |

### PUT `/api/rag/collections/{id}` body

Body is validated imperatively (`obj.get` dispatch / element-wise walks), not
through the `crate::body` member vocabulary — the field set lives in the route
source.

## `/api/rag/search`

Source: [`api/src/routes/knowledge/rag_search.rs`](../../api/src/routes/knowledge/rag_search.rs)

> /api/rag/search. Ranked retrieval across the caller's accessible
> collections, for EITHER caller shape: a signed-in user (their bindings) or
> a fleet agent (agent key + x-agent-name → that agent's bindings). This is
> the endpoint the search_knowledge MCP tool calls — retrieval as
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `dual` | [body](#post-apiragsearch-body) | `{hits}` | 200, 400, 502 | — |

### POST `/api/rag/search` body

| field | schema | notes |
| :--- | :--- | :--- |
| `query` | `string(1, 2000)` |  |
| `limit` | `number?(1, 20)` |  |
| `collectionIds` | `uuid[]?(20)` |  |

## `/api/search`

Source: [`api/src/routes/knowledge/search.rs`](../../api/src/routes/knowledge/search.rs)

> /api/search.
>
> LIVE WEB SEARCH — the endpoint behind the `web_search` MCP tool, and the one
> place an agent or a signed-in user reaches this deployment's search.
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| POST | `dual` | [body](#post-apisearch-body) | `{query, results, via}` | 200, 400, 503 | — |

### POST `/api/search` body

| field | schema | notes |
| :--- | :--- | :--- |
| `query` | `string(2, 400)` |  |

## `/api/templates`

Source: [`api/src/routes/knowledge/templates.rs`](../../api/src/routes/knowledge/templates.rs)

> /api/templates. The org's template library (ticket + plan formats). GET →
> all (any member — the library grounds pickers everywhere). POST → create
> (needs templates.manage — the skeletons are org-wide starting points).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `session` | — | `{templates}` | 200 | — |
| POST | `session` + `perm:templates.manage` | [body](#post-apitemplates-body) | `{template}` | 200, 400 | — |

### POST `/api/templates` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `string trimmed(1, 120)` |  |
| `kind` | `enum(ticket|plan)` |  |
| `body` | `string?(50000)` |  |
| `guidance` | `string?(10000)` |  |

## `/api/templates/{id}`

Source: [`api/src/routes/knowledge/templates_id.rs`](../../api/src/routes/knowledge/templates_id.rs)

> /api/templates/{id}. One template: PUT → edit (kind is immutable — retire
> and recreate instead), DELETE → remove (bindings cascade/null out;
> consumers fall through the chain).

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| PUT | `session` + `perm:templates.manage` | [body](#put-apitemplatesid-body) | `{template}` | 200, 400, 404 | — |
| DELETE | `session` + `perm:templates.manage` | — | `{ok}` | 200, 404 | — |

### PUT `/api/templates/{id}` body

| field | schema | notes |
| :--- | :--- | :--- |
| `name` | `optional_trimmed` |  |
| `body` | `string?(50000)` |  |
| `guidance` | `string?(10000)` |  |

