# Server — the app API

`apps/<slug>/server.ts` default-exports one fetch handler, mounted at `/api/apps/<slug>/*`.
The host authenticates the session, checks the app is enabled, and checks the caller holds at
least one of the app's surfaces **before** your handler runs. Which surfaces and permissions
they hold arrive on the context; decisions beyond "may reach the app" are yours.

```ts
import { defineAppServer, json, parseBody, z } from '@talaria/sdk/server'

const Body = z.object({ name: z.string().min(1) })

export default defineAppServer({
  async fetch(request, ctx) {
    if (ctx.path === 'things' && request.method === 'POST') {
      const body = await parseBody(request, Body)
      if (body instanceof Response) return body        // the standard 400
      return json({ thing: await ctx.store.insert('things', body) })
    }
    if (ctx.path === 'things' && request.method === 'GET') {
      return json({ things: await ctx.store.list('things') })
    }
    return json({ error: 'not found' }, { status: 404 })
  },
})
```

## The context

| field | what it is |
| :--- | :--- |
| `ctx.user` | The signed-in user (id, name, email, role) — already authenticated |
| `ctx.access` | `{ admin, work, manage, settings }`: the caller's grants on this app (below) |
| `ctx.perms` | The caller's effective platform permissions (`boards.create`, `kb.edit`, …) |
| `ctx.can(perm)` | Whether the caller holds `perm`; always true for an admin |
| `ctx.app` | Your slug |
| `ctx.path` | The part after `/api/apps/<slug>/` — e.g. `items/123` |
| `ctx.url` | The parsed URL (query params) |
| `ctx.store` | Your document store (below) |

## Grants

Each surface is its own grant, set per person in Admin → People or per team in Manage → Teams:
the work surface is `/x/<slug>`, the manage surface `/x/<slug>/manage`, and the settings tab
follows the manage grant. An app that ships settings without a manage surface is still granted
through `/x/<slug>/manage`. The host admits a caller who holds **any** surface, so a member granted
only your work surface can still call the routes your Manage surface uses. Check `ctx.access`
on those routes:

```ts
if (ctx.path.startsWith('admin/') && !ctx.access.manage) {
  return json({ error: 'forbidden' }, { status: 403 })
}
```

`ctx.access.admin` is true for an instance admin, who holds every surface the app ships. A
surface your app doesn't ship is always `false`. An app with no surfaces has nothing to grant,
so only admins reach its server. The grants come from the platform's session read with the
caller's cookie, which lists the app views the caller holds; when the host cannot read it, or
cannot read which apps are enabled, the request gets a 503 and your handler doesn't run. The
compiled client (`/app-builds/<slug>/…`) is served under the same rule, and only `app.js`,
`app.css` and `assets/**` are served. The compiled `server.js` and `mcp.js`, and any chunk they
split off with a dynamic `import()`, are built outside `assets/` and never leave the host.

`parseBody` is the same validation door the host's own routes use: a zod schema in, the
validated data or the standard 400 (first issue message) out. `z` rides along because apps
can't install zod themselves. Follow the platform's [API conventions](../API-CONVENTIONS.md):
errors are `json({ error }, { status })`; mutations return `{ ok: true }` or the created
object.

## The document store

Namespaced collections of JSON documents in Postgres — no migrations, invisible to other apps.

| call | returns |
| :--- | :--- |
| `store.list(collection, { limit?, offset?, newestFirst? })` | `AppDoc[]` |
| `store.get(collection, id)` | `AppDoc` \| null |
| `store.insert(collection, data)` | the created `AppDoc` |
| `store.update(collection, id, patch)` | the updated `AppDoc` \| null (shallow merge) |
| `store.remove(collection, id)` | boolean |
| `store.count(collection)` | number |
| `store.wipe()` | deletes everything this app stored |

An `AppDoc` is `{ id, data, createdAt, updatedAt }`.

## Platform data

For boards, knowledge, comms, agents — call the regular APIs from the **client**
([client.md](./client.md)): they run under the user's session, so every permission and ACL
applies exactly as it would in the core UI. An app can never do more than the person using it.
