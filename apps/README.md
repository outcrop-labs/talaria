# Talaria apps

> The quick in-repo guide. Full docs: [`docs/APPS.md`](../docs/APPS.md) (anatomy, lifecycle,
> marketplace, publishing) and [`docs/SDK.md`](../docs/SDK.md) (the complete `@talaria/sdk`
> reference).

Apps are self-contained codebases that compile **into** the Talaria deployment and render as
**native platform surfaces** — same design system, same router, same session. Not iframes, not
webhooks: your code becomes part of the product, and integrates with anything the signed-in user
could already do.

Apps are **Svelte 5** (runes) with `@tanstack/svelte-query` for data, like the host. Shared
dependencies (svelte, svelte-query, zod, lucide icons) resolve from the host — apps install
nothing, there is exactly one copy of each in the deployment.

Each directory here is one app. Drop a codebase in (or install one from **Manage → Apps**, which
git-clones it here), reload the dev server (or rebuild in production), then enable it in
**Manage → Apps**.

## Anatomy

```
apps/<slug>/
  talaria.json   manifest — name, icon, version, description, surfaces
  app.ts         UI surfaces — default-exports defineApp({...})
  *.svelte       one component per surface (plus whatever else you need)
  server.ts      optional API — default-exports defineAppServer({...})
  mcp.ts         optional MCP tools for AGENTS — default-exports defineAppMcp({...})
  ...            anything else your app needs (components, lib, assets)
```

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

| surface    | where it appears                      | access                                       |
|------------|----------------------------------------|----------------------------------------------|
| `work`     | Work section of the nav → `/x/<slug>`  | admins; members need an explicit grant       |
| `manage`   | Manage section → `/x/<slug>/manage`    | admins; members need an explicit grant       |
| `settings` | a Settings tab (for people with access)| follows the work-view grant                  |

Apps are **explicit-grant**: enabling one gives members nothing until an admin adds its views per
person in **Admin → People** — the same checklist that governs core Manage views. The app API
gateway enforces the grant server-side.

### app.ts + a surface

```svelte
<!-- app.ts -->
import { defineApp } from '@talaria/sdk'
import Work from './Work.svelte'
export default defineApp({ work: Work })
```

```svelte
<!-- Work.svelte -->
<script lang="ts">
  import { Button, useAppQuery } from '@talaria/sdk'
  const things = useAppQuery('my-app', 'things')
</script>

{#each things.data ?? [] as t (t.id)}
  <div>{t.data.name}</div>
{/each}
<Button>Add one</Button>
```

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
    // ctx.user   the signed-in user (id, name, email, role) — already authenticated
    // ctx.path   the part after /api/apps/<slug>/
    // ctx.store  a namespaced document store (Postgres JSON, no migrations)
    if (ctx.path === 'things' && request.method === 'POST') {
      const body = await parseBody(request, Body)
      if (body instanceof Response) return body // the standard 400
      return json({ thing: await ctx.store.insert('things', body) })
    }
    if (ctx.path === 'things' && request.method === 'GET') {
      return json({ things: await ctx.store.list('things') })
    }
    return json({ error: 'not found' }, { status: 404 })
  },
})
```

The host authenticates every request, verifies the app is enabled, and checks the user may reach
this app **before** your handler runs. Role/permission decisions beyond that are yours
(`ctx.user.role`, or call platform APIs). On the client, `appApi('<slug>')` /
`useAppQuery('<slug>', 'things')` are pre-bound to these routes.

### mcp.ts — tools for agents

```ts
import { defineAppMcp } from '@talaria/sdk/server'

export default defineAppMcp({
  tools: [
    {
      name: 'things_search',
      description: 'Search things by text.',
      inputSchema: { type: 'object', properties: { query: { type: 'string' } } },
      async handler(args, ctx) {
        // ctx.agent  the calling agent's name (gateway-authenticated)
        // ctx.store  the same per-app document store server.ts uses
        return ctx.store.list('things')
      },
    },
  ],
})
```

When the app is enabled, its tools register as an MCP server (badged **app**) in **Manage → MCP**
and inherit the platform's full granular governance: assign it to specific agents (or all), narrow
per-agent tool subsets, and gate per-person access — all enforced by the same gateway as every
other MCP server. Calls dispatch in-process, no network hop. Disabling the app retires the server
and rolls any agents that carried it.

### Data

`ctx.store` gives every app its own collections of JSON documents
(`list / get / insert / update / remove / count / wipe`), namespaced by app slug — no migrations,
no schema, invisible to other apps. For platform data (boards, knowledge, comms, agents…), call the
regular APIs from the client: they run under the user's session, so **every permission and ACL
applies exactly as it would in the core UI** — an app can never do more than the person using it.

## Security model

Apps are compiled into the deployment and run **fully trusted**, like platform code. The safety
boundary is the *user session*, not a sandbox: server handlers receive an authenticated user and the
client calls APIs as that user. Admins should install only apps they trust — the marketplace labels
apps maintained by Outcrop Labs as **official**.

## Publishing

Push your app to a public https git repository with `talaria.json` at the root (repo name
`talaria-app-<slug>` is the convention — the prefix is stripped on install). Anyone can then install
it from **Manage → Apps → Discover → Install from Git**. To be listed in the marketplace, submit it
to the catalog index (`outcrop-labs/talaria-apps`).

The `contacts/` app in this directory is the working reference — three surfaces, an app server,
MCP tools, and the store, in a handful of small files.
