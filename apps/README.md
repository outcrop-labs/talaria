# Talaria apps

> The quick in-repo guide. Full docs: [`docs/APPS.md`](../docs/APPS.md) (anatomy, lifecycle,
> marketplace, publishing) and [`docs/sdk/`](../docs/sdk/README.md) (the complete `@talaria/sdk`
> docset).

Apps are self-contained TypeScript codebases this instance compiles and renders as
**native platform surfaces** — same design system, same router, same session. Not iframes, not
webhooks: your code becomes part of the product, and integrates with anything the signed-in user
could already do. Authors import `@talaria/sdk` (+ svelte); they never write Rust.

Apps are **Svelte 5** (runes) with `@tanstack/svelte-query` for data, like the host. Shared
dependencies (svelte, svelte-query, zod, lucide icons) resolve from the host — apps install
nothing, there is exactly one copy of each in the deployment.

Each directory here is one app. Drop a codebase in (or install one from **Manage → Apps**, which
git-clones it here). The instance compiles it and starts its database; then enable it in
**Manage → Apps**.

## Anatomy

```
apps/<slug>/
  talaria.json   manifest — name, icon, version, description, surfaces
  app.ts         UI surfaces — default-exports defineApp({...}); only when talaria.json declares one
  *.svelte       one component per surface (plus whatever else you need)
  server.ts      optional API — default-exports defineAppServer({...}) → /api/apps/<slug>/*
  mcp.ts         optional MCP tools for AGENTS — default-exports defineAppMcp({...})
  harnesses/     optional activity harnesses — one defineHarness per file, scored in Models → Fitness
  ...            anything else your app needs (components, lib, assets)
```

An app with no UI (a server or agent tools only) declares no surfaces and ships no `app.ts`; with
nothing to grant, only admins reach its server, and agents reach its tools through MCP governance.

### talaria.json

```json
{
  "name": "Contacts",
  "icon": "☏",
  "version": "0.1.0",
  "description": "Lightweight CRM — people, companies, stages, notes.",
  "surfaces": { "work": "Contacts", "manage": "Contacts data", "settings": "Contacts" }
}
```

Surfaces are opt-in; ship any combination:

| surface    | where it appears                                                    | access                                 |
|------------|---------------------------------------------------------------------|----------------------------------------|
| `work`     | a tile in the top dock's Apps group → `/x/<slug>`, and every route below it | admins; members need an explicit grant |
| `manage`   | the manage area behind the dock's gear → `/x/<slug>/manage`, and every route below it | admins; members need an explicit grant |
| `settings` | a Settings tab (for people with access); one page, no routes below it | follows the manage grant             |

Apps are **explicit-grant**: enabling one gives members nothing until an admin adds its views per
person in **Admin → People**, or per team in **Manage → Teams** — the same checklist that governs
core Manage views. The work and manage surfaces are separate grants, and each covers the routes
below it: a member with only the work grant reaches `/x/<slug>/items/42` and never
`/x/<slug>/manage/…`. An app that ships a settings tab without a manage surface is still granted
through `/x/<slug>/manage`. The app API gateway admits anyone holding **any** of the app's
surfaces and tells your server which in `ctx.access`, so a route only your manage surface uses
must check `ctx.access.manage` itself (see server.ts below).

### app.ts + a surface

```ts
// apps/<slug>/app.ts
import { defineApp } from '@talaria/sdk'
import Work from './Work.svelte'
export default defineApp({ work: Work })
```

```svelte
<!-- apps/<slug>/Work.svelte -->
<script lang="ts">
  import { Button, useAppQuery } from '@talaria/sdk'
  // The server below answers { things: [...] } — read the list out of it.
  const things = useAppQuery<{ things: Array<{ id: string; data: { name: string } }> }>('things')
</script>

{#each things.data?.things ?? [] as t (t.id)}
  <div>{t.data.name}</div>
{/each}
<Button>Add one</Button>
```

A surface never spells its own slug: the hooks bind to the app the surface belongs to, under the
slug THIS instance installed it as (install strips `talaria-app-` from the repo name, so it need
not be the name you scaffolded). `useApp()` returns `{ slug, surface, manifest, rest, query }`
when you need it. For a real list, hand the query to `QueryState`, so a failed read shows the
error and a Retry instead of an empty list ([client.md](../docs/sdk/client.md)).

### Routes below a surface

An app owns every route below its surfaces: `/x/<slug>/<rest>` renders the work surface and
`/x/<slug>/manage/<rest>` the manage surface, so a list, its records and its tabs are real
addresses people can link, reload and step through with Back. `useAppRoute()` says where the
surface is (`rest`, `segments`, `query`, and `match('items/:id')` for a route's params),
`appHref(path)` builds a link relative to the surface, and `appNavigate(path)` goes there from
code. `{ surface: 'manage' }` links into the other surface, which is how a settings tab, one page
with no routes below it, links anywhere deeper. Who may open a route follows its surface's grant.
Details: [client.md](../docs/sdk/client.md#routes-inside-your-app).

`@talaria/sdk` re-exports the Mercury UI kit (Button, Input, Select, Modal, Chip, EmptyState,
Skeleton, confirm/alert, context menus, `cn`, …), session hooks (`useMe`, `useHasPerm`,
`useIsAdmin`), svelte-query primitives (`createQuery`, `createMutation`, `useQueryClient`,
`keepPreviousData`), motion presets (`fade`, `fly`, `QUICK`, …), and the fetch helpers below.
Icons come from `@lucide/svelte`.

### server.ts

```ts
import { defineAppServer, json, parseBody, z } from '@talaria/sdk/server'

const Body = z.object({ name: z.string().min(1) })

export default defineAppServer({
  async fetch(request, ctx) {
    // ctx.user      the signed-in user (id, name, email, role) — already authenticated
    // ctx.access    which of this app's surfaces they hold: { admin, work, manage, settings }
    // ctx.segments  the path after /api/apps/<slug>/, decoded once: ['things', '123']
    // ctx.store     a namespaced document store (Postgres JSON, no migrations)
    const [head, id] = ctx.segments
    if (head === 'things' && id === undefined && request.method === 'POST') {
      const body = await parseBody(request, Body)
      if (body instanceof Response) return body // the standard 400
      return json({ thing: await ctx.store.insert('things', body) })
    }
    if (head === 'things' && id === undefined && request.method === 'GET') {
      return json({ things: await ctx.store.list('things') })
    }
    if (head === 'stats' && request.method === 'GET') {
      // Read by the manage surface only. The host admitted anyone holding ANY surface,
      // so this route checks its own grant.
      if (!ctx.access.manage) return json({ error: 'forbidden' }, { status: 403 })
      return json({ things: await ctx.store.count('things') })
    }
    return json({ error: 'not found' }, { status: 404 })
  },
})
```

The host authenticates every request, verifies the app is enabled, and checks the caller holds
at least one of its surfaces **before** your handler runs. `ctx.access` says which; everything
finer is yours: `ctx.can('boards.create')` for a platform permission, and `ctx.api(path)` to call
the platform as the caller. The context also carries `ctx.path` (the segments joined),
`ctx.perms`, `ctx.url`, `ctx.origin` (for absolute links), `ctx.secret(name)` (a workspace
secret an admin gave this app) and `ctx.signal` (aborts when the caller leaves):
[server.md](../docs/sdk/server.md). Route on `ctx.segments`, never on a prefix of `ctx.path`.
Every path under `/api/apps/<slug>/` is yours; the host's own app routes (rebuild) live under
`/api/app-host/<slug>/`. On the client, `useAppApi()` / `useAppQuery('things')` are pre-bound to
these routes, under the slug the app was installed as.

### mcp.ts — tools for agents

```ts
import { defineAppMcp } from '@talaria/sdk/server'

export default defineAppMcp({
  tools: [
    {
      name: 'things_search',
      description: 'Search things by name. Returns matching { id, name }.',
      inputSchema: { type: 'object', properties: { query: { type: 'string' } } },
      async handler(args, ctx) {
        // ctx.agent  the calling agent's name (gateway-authenticated)
        // ctx.store  the same per-app document store server.ts uses
        const query = String(args.query ?? '').toLowerCase()
        const things = await ctx.store.list<{ name?: string }>('things', { limit: 500 })
        return things
          .filter((t) => (t.data.name ?? '').toLowerCase().includes(query))
          .map((t) => ({ id: t.id, name: t.data.name ?? '' }))
      },
    },
  ],
})
```

When the app is enabled, its tools register as an MCP server (badged **app**) in **Manage → MCP**
and inherit the platform's full granular governance: assign it to specific agents (or all), narrow
per-agent tool subsets, and gate per-person access — all enforced by the same gateway as every
other MCP server. Calls dispatch in-process, no network hop. Each tool is checked when the app
loads; one that fails (a bad name, no description, an oversized schema) is left out and named on
the server's card, and the app's other tools keep working ([mcp.md](../docs/sdk/mcp.md)). Turning
the app off, or a failed build, takes its tools off every agent (they roll) but keeps the server
row and every grant on it, marked **app off**, until the app serves again; only uninstalling
removes the row.

### Data

`ctx.store` gives every app its own collections of JSON documents
(`list / get / insert / update / remove / count / wipe`), namespaced by app slug — no migrations,
no schema, invisible to other apps. For platform data (boards, knowledge, comms, agents…), call the
regular APIs from the client, or from the server with `ctx.api('/api/boards')`, which sends the
caller's session cookie: either way they run under the user's session, so **every permission and
ACL applies exactly as it would in the core UI** — an app can never do more than the person using
it.

## Security model

Apps are compiled into the deployment and run **fully trusted**, like platform code. The safety
boundary is the *user session*, not a sandbox: server handlers receive an authenticated user, and
the client and `ctx.api` call APIs as that user. An app reads a workspace secret only when an admin
lists `talaria-app://<slug>` in its allowed hosts, and every read is audited. Admins should install
only apps they trust — the marketplace labels apps maintained by Outcrop Labs as **official**.

## Publishing

Push your app to a public https git repository with `talaria.json` at the root (repo name
`talaria-app-<slug>` is the convention — the prefix is stripped on install). Anyone can then install
it from **Manage → Apps → Discover → Install from Git**. To be listed in the marketplace, submit it
to the catalog index (`outcrop-labs/talaria-apps`).

Scaffold a new app with `bun talaria app new <slug>`. Core ships none enabled.

