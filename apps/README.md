# Talaria apps

Apps are self-contained codebases that compile **into** the Talaria deployment and render as
**native platform surfaces** — same design system, same router, same session. Not iframes, not
webhooks: your code becomes part of the product, and integrates with anything the signed-in user
could already do.

Each directory here is one app. Drop a codebase in (or install one from **Manage → Apps**, which
git-clones it here), reload the dev server (or rebuild in production), then enable it in
**Manage → Apps**.

## Anatomy

```
apps/<slug>/
  talaria.json   manifest — name, icon, version, description, surfaces
  app.tsx        UI surfaces (React) — default-exports defineApp({...})
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
  "surfaces": {
    "work": "Contacts",
    "manage": "Contacts data",
    "settings": "Contacts"
  }
}
```

Surfaces are opt-in; ship any combination:

| surface    | where it appears                          | default access                          |
|------------|-------------------------------------------|-----------------------------------------|
| `work`     | Work section of the nav → `/x/<slug>`      | all members (denialable per person)     |
| `manage`   | Manage section → `/x/<slug>/manage`        | admins; members need an explicit grant  |
| `settings` | a tab on every user's Settings page        | all members                             |

Access is governed in **Admin → People** with the same checklist as core views.

### app.tsx

```tsx
import { defineApp, useMe, Button } from '@talaria/sdk'

function Work() {
  const { data: me } = useMe()
  return <div className="p-8">Hello {me?.name} <Button>Do it</Button></div>
}

export default defineApp({ work: Work })
```

`@talaria/sdk` re-exports the Mercury UI kit (Button, Input, Select, Modal, Chip, EmptyState,
Skeleton, confirm/alert, context menus, `cn`, …), session hooks (`useMe`, `useHasPerm`), react-query
(`useQuery`, `useMutation`, …), and fetch helpers. Import `react` freely — shared dependencies
(react, react-dom, @tanstack/react-query, @tanstack/react-router, lucide-react) resolve from the
host, so there is exactly one copy of each in the deployment.

### server.ts

```ts
import { defineAppServer, json } from '@talaria/sdk/server'

export default defineAppServer({
  async fetch(request, ctx) {
    // ctx.user   the signed-in user (id, name, email, role) — already authenticated
    // ctx.path   the part after /api/apps/<slug>/
    // ctx.store  a namespaced document store (Postgres JSON, no migrations)
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
and the store, in ~2 files.
