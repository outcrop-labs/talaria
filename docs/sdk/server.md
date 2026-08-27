# Server — the app API

`apps/<slug>/server.ts` default-exports one fetch handler, mounted at `/api/apps/<slug>/*`.
The host authenticates the session, checks the app is enabled, and checks the caller may reach
this app **before** your handler runs. Role/permission decisions beyond that are yours.

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
| `ctx.app` | Your slug |
| `ctx.path` | The part after `/api/apps/<slug>/` — e.g. `contacts/123` |
| `ctx.url` | The parsed URL (query params) |
| `ctx.store` | Your document store (below) |

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
