# Server — the app API

`apps/<slug>/server.ts` default-exports one fetch handler, mounted at `/api/apps/<slug>/*`.
Every path under that prefix is yours: the host's own routes about an app (rebuild) live under
`/api/app-host/<slug>/`. The host authenticates the session, checks the app is enabled, and
checks the caller holds at least one of the app's surfaces **before** your handler runs. Which
surfaces and permissions they hold arrive on the context; decisions beyond "may reach the app"
are yours.

```ts
import { defineAppServer, json, parseBody, z } from '@talaria/sdk/server'

const Body = z.object({ name: z.string().min(1) })

export default defineAppServer({
  async fetch(request, ctx) {
    const [head, id, ...rest] = ctx.segments
    if (head === 'things' && id === undefined && request.method === 'POST') {
      const body = await parseBody(request, Body)
      if (body instanceof Response) return body        // the standard 400
      return json({ thing: await ctx.store.insert('things', body) })
    }
    if (head === 'things' && id === undefined && request.method === 'GET') {
      return json({ things: await ctx.store.list('things') })
    }
    if (head === 'things' && id !== undefined && rest.length === 0 && request.method === 'GET') {
      const thing = await ctx.store.get('things', id)  // null for an id that isn't one
      return thing ? json({ thing }) : json({ error: 'not found' }, { status: 404 })
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
| `ctx.segments` | The sub-path after `/api/apps/<slug>/` as decoded segments — `['items', '123']` (below) |
| `ctx.path` | The same sub-path joined with `/` — `items/123` |
| `ctx.url` | The parsed URL (query params) |
| `ctx.origin` | The origin people reach this instance at, for absolute links (below) |
| `ctx.store` | Your document store (below) |
| `ctx.api(path, init?)` | The platform API, called as the caller (below) |
| `ctx.secret(name)` | A workspace secret's value, when an admin gave it to this app (below) |
| `ctx.signal` | Aborts when the caller disconnects (below) |

## The path

The sub-path is decoded exactly once. Each segment of `ctx.segments` is one segment of the URL:
`files/100%25` arrives as `['files', '100%']`. A segment never holds a `/` — a path with an
encoded slash (`%2F`) is refused with a 400 before your handler runs, and so is a malformed
escape. Send a value that can hold a slash in the query string (`ctx.url.searchParams`).
`ctx.path` is the segments joined with `/`, so route and authorize on `ctx.segments`.

## Grants

Each surface is its own grant, set per person in Admin → People or per team in Manage → Teams:
the work surface is `/x/<slug>`, the manage surface `/x/<slug>/manage`, and the settings tab
follows the manage grant. An app that ships settings without a manage surface is still granted
through `/x/<slug>/manage`. The host admits a caller who holds **any** surface, so a member granted
only your work surface can still call the routes your Manage surface uses. Check `ctx.access`
on those routes, against the first segment:

```ts
if (ctx.segments[0] === 'admin' && !ctx.access.manage) {
  return json({ error: 'forbidden' }, { status: 403 })
}
```

Check the segment, not a prefix of `ctx.path`. `ctx.path.startsWith('admin/')` lets
`/api/apps/<slug>/admin` through, because the bare path has no trailing slash, while a router that
splits the path still sends that request to the admin routes.

`ctx.access.admin` is true for an instance admin, who holds every surface the app ships. A
surface your app doesn't ship is always `false`. An app with no surfaces has nothing to grant,
so only admins reach its server. The grants come from the platform's session read with the
caller's cookie, which lists the app views the caller holds; when the host cannot read it, or
cannot read which apps are enabled, the request gets a 503 and your handler doesn't run. The
compiled client (`/app-builds/<slug>/…`) is served under the same rule, and only `app.js`,
`app.css` and `assets/**` of the app's current build are served; an older build's key answers
404. The compiled `server.js` and `mcp.js`, and any chunk they split off with a dynamic
`import()`, are built outside `assets/` and never leave the host.

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

An `AppDoc` is `{ id, data, createdAt, updatedAt }`. Ids are UUIDs the store mints on `insert`.
An id that isn't one names no document: `get` and `update` return `null` and `remove` returns
`false`, without an error, so an `items/123` route answers your own 404.

## Platform data

For boards, knowledge, comms, agents, call the regular APIs. From the **client**
([client.md](./client.md)) they run under the user's session. From the server, `ctx.api` does the
same thing for you:

```ts
const { boards } = await ctx.api<{ boards: Array<{ id: string; name: string }> }>('/api/boards')
await ctx.api(`/api/boards/${boards[0]!.id}/tasks`, { method: 'POST', body: { title: 'Follow up' } })
```

`ctx.api(path, { method?, body?, headers?, signal?, timeoutMs? })` calls the platform API with
the caller's session cookie and nothing else of theirs. Every permission and ACL applies exactly
as in the core UI, so an app can never do more than the person using it. A `body` that isn't
already a string, `FormData`, `Blob` or bytes is sent as JSON. It resolves to the parsed JSON
(`null` for an empty body, the text when the answer isn't JSON). A non-2xx answer throws an
`HttpError` whose `status` is the code and whose `message` is the server's own sentence:

```ts
import { HttpError } from '@talaria/sdk/server'

try {
  await ctx.api(`/api/boards/${id}/tasks`)
} catch (e) {
  if (e instanceof HttpError && e.status === 403) return json({ error: e.message }, { status: 403 })
  throw e
}
```

The path must start with `/api/`. It can't name an app's own server (`/api/apps/…`), the host's
app routes (`/api/app-host/…`), the app-MCP gateway or the service-to-service prefix
(`/api/internal`); those throw a `TypeError`. A read has a
30-second deadline to the answer's headers, which `timeoutMs` changes; a write has none. Every
call follows `ctx.signal`, and any `signal` you pass as well.

## Secrets

`ctx.secret(name)` reads a workspace secret: `ctx.secret('crm')` for a single-entry secret,
`ctx.secret('crm.api_key')` for one entry of a bundle. It resolves to the value, or to `null`. An
app reads only the secrets an admin gave it:

- the secret's **allowed hosts** list `talaria-app://<slug>` exactly. This is the same entry that
  lets an agent's `«secret:…»` handle reach this app's MCP tools ([mcp.md](./mcp.md)). A secret
  with no allowed hosts isn't readable by app code, because for agents a per-agent grant gates
  it, and a server has no agent;
- it is a **workspace** secret (Admin → Secrets), not a person's own working secret;
- it is a **vault** secret, not a one-shot relay, and it hasn't expired.

A use limit is honoured: each read spends one use. Every read writes a `secrets.spend` audit row
with actor `app:<slug>` and the id of the person whose request it was. When a secret doesn't
resolve, the reason goes to the host log, never to the app. Use the value on the server, for
the call it's for. Never return it to the browser, store it, or log it.

## Origin

`ctx.origin` is where people reach this instance, for absolute links in emails, webhooks and
redirects. It is `AUTH_PUBLIC_URL` when the operator set it. Otherwise it is the proto and host
the reverse proxy stated (`x-forwarded-proto`, `x-forwarded-host`), and otherwise the request's
own origin. `ctx.url` isn't a substitute: behind TLS termination it is plain `http`. Without
`AUTH_PUBLIC_URL`, the forwarded headers are whatever reached the host, so set it on a public
instance.

## Cancellation

`ctx.signal` aborts when the caller disconnects before your answer is done, such as a closed tab
or an aborted fetch. Pass it to long work, `runHarness` and your own `fetch` calls, so that work
stops instead of spending for nobody. `ctx.api` already follows it. A handler that throws because
the caller left isn't logged as a crash.

## An app with no UI

`app.ts` is required only when `talaria.json` declares a surface. A server-only or MCP-only app
declares none, ships `server.ts` or `mcp.ts`, and builds, enables and runs without `app.ts`:

```json
{ "name": "Lead scorer", "icon": "⬡", "version": "0.1.0", "description": "Scores leads for agents" }
```

With no surfaces there is nothing to grant, so only admins reach its server. Agents reach its
MCP tools through the usual MCP governance.
